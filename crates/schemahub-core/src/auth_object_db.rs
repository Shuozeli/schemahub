//! redb/PostgreSQL project and membership stores over `ObjectDb` records.

use std::sync::Arc;

use schemahub_jj::{ObjectDb, ObjectDbError};
use schemahub_types::{Identity, Role};
use serde::{Deserialize, Serialize};

use crate::auth_store::{
    AccessStoreError, AccessStoreResult, ProjectMeta, ProjectStore, RoleStore,
};

const PROJECT_COLLECTION: &str = "schemahub.projects.v1";
const ROLE_COLLECTION: &str = "schemahub.project_roles.v1";
const MAX_CAS_RETRIES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RoleRecord {
    project: String,
    identity_id: String,
    role: Role,
    active: bool,
}

#[derive(Debug)]
pub struct ObjectDbProjectStore {
    db: Arc<dyn ObjectDb>,
}

impl ObjectDbProjectStore {
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    fn encode(meta: &ProjectMeta) -> AccessStoreResult<Vec<u8>> {
        serde_json::to_vec(meta)
            .map_err(|error| AccessStoreError::Backend(format!("encode project: {error}")))
    }

    fn decode(bytes: &[u8]) -> AccessStoreResult<ProjectMeta> {
        serde_json::from_slice(bytes)
            .map_err(|error| AccessStoreError::Backend(format!("decode project: {error}")))
    }

    fn upsert(&self, mut meta: ProjectMeta) -> AccessStoreResult<ProjectMeta> {
        for _ in 0..MAX_CAS_RETRIES {
            match self
                .db
                .get_record(PROJECT_COLLECTION, &meta.name)
                .map_err(map_db)?
            {
                None => {
                    if meta.etag.is_empty() {
                        meta.etag = "v1".to_string();
                    }
                    let replacement = Self::encode(&meta)?;
                    if self
                        .db
                        .create_record(PROJECT_COLLECTION, &meta.name, &replacement)
                        .map_err(map_db)?
                    {
                        return Ok(meta);
                    }
                }
                Some(current) => {
                    let current_meta = Self::decode(&current)?;
                    if meta.etag.is_empty() {
                        meta.etag = next_etag(&current_meta.etag)?;
                    }
                    let replacement = Self::encode(&meta)?;
                    if self
                        .db
                        .compare_and_swap_record(
                            PROJECT_COLLECTION,
                            &meta.name,
                            &current,
                            &replacement,
                        )
                        .map_err(map_db)?
                    {
                        return Ok(meta);
                    }
                }
            }
        }
        Err(AccessStoreError::Backend(
            "project changed repeatedly while writing bootstrap metadata".to_string(),
        ))
    }

    /// Atomically import a project and its complete membership set. This is
    /// used by the one-time JSON access-store migration so a crash cannot
    /// leave a project with only a partially copied ACL.
    pub fn create_with_members(
        &self,
        mut meta: ProjectMeta,
        members: &[(String, Role)],
    ) -> AccessStoreResult<ProjectMeta> {
        if !members.iter().any(|(_, role)| *role == Role::Owner) {
            return Err(AccessStoreError::Backend(format!(
                "project '{}' cannot be created without an Owner",
                meta.name
            )));
        }
        meta.etag = "v1".to_string();
        let project_bytes = Self::encode(&meta)?;
        let mut owned_records = vec![(
            PROJECT_COLLECTION.to_string(),
            meta.name.clone(),
            project_bytes,
        )];
        for (identity_id, role) in members {
            let record = RoleRecord {
                project: meta.name.clone(),
                identity_id: identity_id.clone(),
                role: *role,
                active: true,
            };
            owned_records.push((
                ROLE_COLLECTION.to_string(),
                role_key(&record.project, &record.identity_id),
                encode_role(&record)?,
            ));
        }
        let records: Vec<_> = owned_records
            .iter()
            .map(|(collection, key, value)| (collection.as_str(), key.as_str(), value.as_slice()))
            .collect();
        if !self.db.create_records(&records).map_err(map_db)? {
            return Err(AccessStoreError::AlreadyExists(format!(
                "project '{}' or one of its memberships already exists",
                meta.name
            )));
        }
        Ok(meta)
    }
}

impl ProjectStore for ObjectDbProjectStore {
    fn get(&self, project: &str) -> AccessStoreResult<Option<ProjectMeta>> {
        self.db
            .get_record(PROJECT_COLLECTION, project)
            .map_err(map_db)?
            .map(|bytes| Self::decode(&bytes))
            .transpose()
    }

    fn create_with_owner(
        &self,
        meta: ProjectMeta,
        owner_id: &str,
    ) -> AccessStoreResult<ProjectMeta> {
        self.create_with_members(meta, &[(owner_id.to_string(), Role::Owner)])
    }

    fn set(&self, meta: ProjectMeta) -> AccessStoreResult<()> {
        self.upsert(meta).map(|_| ())
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut meta: ProjectMeta,
    ) -> AccessStoreResult<ProjectMeta> {
        let current = self
            .db
            .get_record(PROJECT_COLLECTION, &meta.name)
            .map_err(map_db)?
            .ok_or_else(|| AccessStoreError::NotFound(meta.name.clone()))?;
        let current_meta = Self::decode(&current)?;
        if current_meta.etag != expected_etag {
            return Err(AccessStoreError::EtagMismatch {
                name: meta.name,
                expected: expected_etag.to_string(),
                current: current_meta.etag,
            });
        }
        meta.etag = next_etag(&current_meta.etag)?;
        let replacement = Self::encode(&meta)?;
        if self
            .db
            .compare_and_swap_record(PROJECT_COLLECTION, &meta.name, &current, &replacement)
            .map_err(map_db)?
        {
            return Ok(meta);
        }
        let latest = self
            .get(&meta.name)?
            .ok_or_else(|| AccessStoreError::NotFound(meta.name.clone()))?;
        Err(AccessStoreError::EtagMismatch {
            name: meta.name,
            expected: expected_etag.to_string(),
            current: latest.etag,
        })
    }

    fn list(&self) -> AccessStoreResult<Vec<ProjectMeta>> {
        let mut projects = Vec::new();
        for (key, bytes) in self.db.list_records(PROJECT_COLLECTION).map_err(map_db)? {
            let meta = Self::decode(&bytes)?;
            if meta.name != key {
                return Err(AccessStoreError::Backend(format!(
                    "project key/name mismatch: key={key:?}, name={:?}",
                    meta.name
                )));
            }
            projects.push(meta);
        }
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(projects)
    }
}

#[derive(Debug)]
pub struct ObjectDbRoleStore {
    db: Arc<dyn ObjectDb>,
}

impl ObjectDbRoleStore {
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    fn replace(&self, record: RoleRecord) -> AccessStoreResult<()> {
        let key = role_key(&record.project, &record.identity_id);
        let replacement = encode_role(&record)?;
        for _ in 0..MAX_CAS_RETRIES {
            match self.db.get_record(ROLE_COLLECTION, &key).map_err(map_db)? {
                None => {
                    if self
                        .db
                        .create_record(ROLE_COLLECTION, &key, &replacement)
                        .map_err(map_db)?
                    {
                        return Ok(());
                    }
                }
                Some(current) => {
                    if self
                        .db
                        .compare_and_swap_record(ROLE_COLLECTION, &key, &current, &replacement)
                        .map_err(map_db)?
                    {
                        return Ok(());
                    }
                }
            }
        }
        Err(AccessStoreError::Backend(format!(
            "membership {key:?} changed repeatedly"
        )))
    }
}

impl RoleStore for ObjectDbRoleStore {
    fn get(&self, project: &str, identity: &Identity) -> AccessStoreResult<Option<Role>> {
        let Some(identity_id) = identity.id() else {
            return Ok(None);
        };
        self.db
            .get_record(ROLE_COLLECTION, &role_key(project, identity_id))
            .map_err(map_db)?
            .map(|bytes| decode_role(&bytes))
            .transpose()
            .map(|record| {
                record
                    .filter(|record| record.active)
                    .map(|record| record.role)
            })
    }

    fn set(&self, project: &str, identity_id: &str, role: Role) -> AccessStoreResult<()> {
        self.replace(RoleRecord {
            project: project.to_string(),
            identity_id: identity_id.to_string(),
            role,
            active: true,
        })
    }

    fn remove(&self, project: &str, identity_id: &str) -> AccessStoreResult<()> {
        let key = role_key(project, identity_id);
        let Some(bytes) = self.db.get_record(ROLE_COLLECTION, &key).map_err(map_db)? else {
            return Ok(());
        };
        let mut record = decode_role(&bytes)?;
        if !record.active {
            return Ok(());
        }
        record.active = false;
        self.replace(record)
    }

    fn list_project(&self, project: &str) -> AccessStoreResult<Vec<(String, Role)>> {
        let mut roles = Vec::new();
        for (key, bytes) in self.db.list_records(ROLE_COLLECTION).map_err(map_db)? {
            let record = decode_role(&bytes)?;
            if role_key(&record.project, &record.identity_id) != key {
                return Err(AccessStoreError::Backend(format!(
                    "role key/content mismatch for {key:?}"
                )));
            }
            if record.project == project && record.active {
                roles.push((record.identity_id, record.role));
            }
        }
        roles.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(roles)
    }
}

fn role_key(project: &str, identity_id: &str) -> String {
    format!("projects/{project}/members/{}", hex::encode(identity_id))
}

fn encode_role(record: &RoleRecord) -> AccessStoreResult<Vec<u8>> {
    serde_json::to_vec(record)
        .map_err(|error| AccessStoreError::Backend(format!("encode role: {error}")))
}

fn decode_role(bytes: &[u8]) -> AccessStoreResult<RoleRecord> {
    serde_json::from_slice(bytes)
        .map_err(|error| AccessStoreError::Backend(format!("decode role: {error}")))
}

fn map_db(error: ObjectDbError) -> AccessStoreError {
    AccessStoreError::Backend(error.to_string())
}

fn next_etag(current: &str) -> AccessStoreResult<String> {
    let version = current
        .strip_prefix('v')
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            AccessStoreError::Backend(format!("invalid stored project etag: {current}"))
        })?;
    Ok(format!("v{}", version + 1))
}

#[cfg(test)]
mod tests {
    use schemahub_jj::{MemoryObjectDb, RedbObjectDb};
    use schemahub_types::Visibility;

    use super::*;

    fn project() -> ProjectMeta {
        ProjectMeta::new("acme", Visibility::Private, "alice", 1_000)
    }

    fn stored_project() -> ProjectMeta {
        let mut project = project();
        project.etag = "v1".to_string();
        project
    }

    #[test]
    fn project_and_initial_owner_are_created_atomically() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db);

        // Act
        let created = projects
            .create_with_owner(project(), "alice")
            .expect("create project and owner");

        // Assert
        assert_eq!(projects.get("acme").unwrap(), Some(created));
        assert_eq!(
            roles.get("acme", &Identity::user("alice")).unwrap(),
            Some(Role::Owner)
        );
    }

    #[test]
    fn owner_conflict_leaves_no_partial_project() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let role = RoleRecord {
            project: "acme".to_string(),
            identity_id: "alice".to_string(),
            role: Role::Reader,
            active: true,
        };
        db.create_record(
            ROLE_COLLECTION,
            &role_key("acme", "alice"),
            &encode_role(&role).unwrap(),
        )
        .unwrap();
        let projects = ObjectDbProjectStore::new(db);

        // Act
        let result = projects.create_with_owner(project(), "alice");

        // Assert
        assert!(matches!(result, Err(AccessStoreError::AlreadyExists(_))));
        assert_eq!(projects.get("acme").unwrap(), None);
    }

    #[test]
    fn owner_conflict_leaves_no_partial_project_in_redb() {
        // Arrange
        let temp = tempfile::tempdir().expect("tempdir");
        let db: Arc<dyn ObjectDb> =
            Arc::new(RedbObjectDb::open(temp.path().join("schemahub.redb")).expect("open redb"));
        let roles = ObjectDbRoleStore::new(db.clone());
        roles
            .set("acme", "alice", Role::Reader)
            .expect("seed conflicting role");
        let projects = ObjectDbProjectStore::new(db);

        // Act
        let result = projects.create_with_owner(project(), "alice");

        // Assert
        assert!(matches!(result, Err(AccessStoreError::AlreadyExists(_))));
        assert_eq!(projects.get("acme").unwrap(), None);
    }

    #[test]
    fn project_and_members_survive_redb_restart() {
        // Arrange
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("schemahub.redb");
        {
            let db: Arc<dyn ObjectDb> =
                Arc::new(RedbObjectDb::open(&path).expect("open redb writer"));
            ObjectDbProjectStore::new(db.clone())
                .create_with_owner(project(), "alice")
                .expect("create project");
            ObjectDbRoleStore::new(db)
                .set("acme", "agent", Role::Writer)
                .expect("add agent");
        }
        let db: Arc<dyn ObjectDb> =
            Arc::new(RedbObjectDb::open(&path).expect("reopen redb reader"));
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db);

        // Act
        let restored_project = projects.get("acme").expect("get project");
        let restored_roles = roles.list_project("acme").expect("list roles");

        // Assert
        assert_eq!(restored_project, Some(stored_project()));
        assert_eq!(
            restored_roles,
            vec![
                ("agent".to_string(), Role::Writer),
                ("alice".to_string(), Role::Owner)
            ]
        );
    }

    #[test]
    fn project_replace_rejects_a_stale_etag_without_losing_the_winner() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db);
        let created = projects
            .create_with_owner(project(), "alice")
            .expect("create project");
        let mut winner = created.clone();
        winner.visibility = Visibility::Public;
        winner.update_time_unix_ms = 2_000;
        let updated = projects
            .replace(&created.etag, winner)
            .expect("replace project");
        let mut stale = created.clone();
        stale.creator = "mallory".to_string();

        // Act
        let result = projects.replace(&created.etag, stale);

        // Assert
        assert!(matches!(
            result,
            Err(AccessStoreError::EtagMismatch {
                expected,
                current,
                ..
            }) if expected == "v1" && current == "v2"
        ));
        assert_eq!(projects.get("acme").unwrap(), Some(updated));
    }
}
