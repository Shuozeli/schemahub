//! Legacy JSON `RoleStore` + `ProjectStore` implementations.
//!
//! Production writes use the ObjectDb-backed stores. These implementations are
//! retained for compatibility tests and for the server's one-time import from
//! pre-0.5 installations.
//!
//! Persistence shape (one file per store, both under `<data_dir>`):
//!
//! ```json
//! // roles.json
//! {
//!   "acme":     { "alice": "Owner", "bob": "Writer" },
//!   "payments": { "carol": "Reader" }
//! }
//!
//! // projects.json
//! [
//!   { "name": "acme",     "visibility": "Public",  "creator": "alice" },
//!   { "name": "payments", "visibility": "Private", "creator": "carol" }
//! ]
//! ```
//!
//! Mutations rewrite the whole file atomically (tempfile in the same directory
//! plus `rename`); both stores guard their in-memory state with a single
//! `Mutex`.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use schemahub_types::{Identity, Role};

use crate::auth_store::{
    AccessStoreError, AccessStoreResult, ProjectMeta, ProjectStore, RoleStore,
};

fn access_error(error: io::Error) -> AccessStoreError {
    AccessStoreError::Backend(error.to_string())
}

// ── FileRoleStore ────────────────────────────────────────────────────────────

/// JSON-backed [`RoleStore`] at `<data_dir>/roles.json`.
///
/// Loaded on construction; every mutation rewrites the file atomically.
pub struct FileRoleStore {
    path: PathBuf,
    state: Mutex<HashMap<String, HashMap<String, Role>>>, // project → identity_id → role
}

impl FileRoleStore {
    /// Open or create the store at `path`. A missing file is treated as an
    /// empty map (the file is created on first write).
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let state = if path.exists() {
            let bytes = std::fs::read(&path)?;
            serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    fn persist_locked(
        path: &Path,
        state: &HashMap<String, HashMap<String, Role>>,
    ) -> io::Result<()> {
        atomic_write_json(path, state)
    }
}

impl RoleStore for FileRoleStore {
    fn get(&self, project: &str, identity: &Identity) -> AccessStoreResult<Option<Role>> {
        let Some(id) = identity.id() else {
            return Ok(None);
        };
        let s = self.state.lock().unwrap();
        Ok(s.get(project).and_then(|m| m.get(id)).copied())
    }

    fn set(&self, project: &str, identity_id: &str, role: Role) -> AccessStoreResult<()> {
        let mut s = self.state.lock().unwrap();
        s.entry(project.to_string())
            .or_default()
            .insert(identity_id.to_string(), role);
        Self::persist_locked(&self.path, &s).map_err(access_error)
    }

    fn remove(&self, project: &str, identity_id: &str) -> AccessStoreResult<()> {
        let mut s = self.state.lock().unwrap();
        if let Some(m) = s.get_mut(project) {
            m.remove(identity_id);
            if m.is_empty() {
                s.remove(project);
            }
        }
        Self::persist_locked(&self.path, &s).map_err(access_error)
    }

    fn list_project(&self, project: &str) -> AccessStoreResult<Vec<(String, Role)>> {
        let s = self.state.lock().unwrap();
        Ok(s.get(project)
            .map(|m| m.iter().map(|(id, r)| (id.clone(), *r)).collect())
            .unwrap_or_default())
    }
}

// ── FileProjectStore ─────────────────────────────────────────────────────────

/// JSON-backed [`ProjectStore`] at `<data_dir>/projects.json`.
pub struct FileProjectStore {
    path: PathBuf,
    state: Mutex<HashMap<String, ProjectMeta>>, // name → meta
}

impl FileProjectStore {
    /// Open or create the store at `path`. Missing file → empty registry.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let state = if path.exists() {
            let bytes = std::fs::read(&path)?;
            // The on-disk format is a `Vec<ProjectMeta>` so it round-trips a
            // stable, hand-editable ordering. Re-key by name in memory.
            let list: Vec<ProjectMeta> = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            list.into_iter().map(|m| (m.name.clone(), m)).collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    fn persist_locked(path: &Path, state: &HashMap<String, ProjectMeta>) -> io::Result<()> {
        // Sort by name for a deterministic on-disk shape (the hand-editable
        // bootstrap workflow benefits from stable diffs).
        let mut list: Vec<&ProjectMeta> = state.values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        atomic_write_json(path, &list)
    }
}

impl ProjectStore for FileProjectStore {
    fn get(&self, project: &str) -> AccessStoreResult<Option<ProjectMeta>> {
        Ok(self.state.lock().unwrap().get(project).cloned())
    }

    fn create_with_owner(
        &self,
        _meta: ProjectMeta,
        _owner_id: &str,
    ) -> AccessStoreResult<ProjectMeta> {
        Err(AccessStoreError::Backend(
            "FileProjectStore cannot atomically create a project and owner; use ObjectDbProjectStore"
                .to_string(),
        ))
    }

    fn set(&self, mut meta: ProjectMeta) -> AccessStoreResult<()> {
        let mut s = self.state.lock().unwrap();
        if meta.etag.is_empty() {
            meta.etag = "v1".to_string();
        }
        s.insert(meta.name.clone(), meta);
        Self::persist_locked(&self.path, &s).map_err(access_error)
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut meta: ProjectMeta,
    ) -> AccessStoreResult<ProjectMeta> {
        let mut state = self.state.lock().unwrap();
        let current = state
            .get(&meta.name)
            .ok_or_else(|| AccessStoreError::NotFound(meta.name.clone()))?;
        if current.etag != expected_etag {
            return Err(AccessStoreError::EtagMismatch {
                name: meta.name,
                expected: expected_etag.to_string(),
                current: current.etag.clone(),
            });
        }
        let version = current
            .etag
            .strip_prefix('v')
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                AccessStoreError::Backend(format!("invalid stored project etag: {}", current.etag))
            })?;
        meta.etag = format!("v{}", version + 1);
        state.insert(meta.name.clone(), meta.clone());
        Self::persist_locked(&self.path, &state).map_err(access_error)?;
        Ok(meta)
    }

    fn list(&self) -> AccessStoreResult<Vec<ProjectMeta>> {
        let s = self.state.lock().unwrap();
        let mut list: Vec<ProjectMeta> = s.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Write `value` as pretty JSON to `path` atomically (tempfile in the same dir
/// + `rename`). Creates parent directories as needed.
fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no filename"))?;
    let tmp_name = format!(".{}.tmp", file_name.to_string_lossy());
    let tmp = dir.join(tmp_name);
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemahub_types::Visibility;
    use tempfile::TempDir;

    #[test]
    fn role_store_persists_across_reopen() {
        // Arrange
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("roles.json");
        {
            let s = FileRoleStore::open(&path).unwrap();
            s.set("acme", "alice", Role::Owner).unwrap();
            s.set("acme", "bob", Role::Writer).unwrap();
        }

        // Act
        let s = FileRoleStore::open(&path).unwrap();
        let alice = s.get("acme", &Identity::user("alice")).unwrap();
        let bob = s.get("acme", &Identity::user("bob")).unwrap();

        // Assert
        assert_eq!(alice, Some(Role::Owner));
        assert_eq!(bob, Some(Role::Writer));
    }

    #[test]
    fn role_store_remove_drops_entry() {
        // Arrange
        let dir = TempDir::new().unwrap();
        let s = FileRoleStore::open(dir.path().join("roles.json")).unwrap();
        s.set("acme", "alice", Role::Owner).unwrap();

        // Act
        s.remove("acme", "alice").unwrap();

        // Assert
        assert_eq!(s.get("acme", &Identity::user("alice")).unwrap(), None);
        assert!(s.list_project("acme").unwrap().is_empty());
    }

    #[test]
    fn project_store_persists_across_reopen() {
        // Arrange
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("projects.json");
        {
            let s = FileProjectStore::open(&path).unwrap();
            s.set(ProjectMeta::new("acme", Visibility::Public, "alice", 1_000))
                .unwrap();
        }

        // Act
        let s = FileProjectStore::open(&path).unwrap();
        let meta = s.get("acme").unwrap().unwrap();

        // Assert
        assert_eq!(meta.visibility, Visibility::Public);
        assert_eq!(meta.creator, "alice");
    }
}
