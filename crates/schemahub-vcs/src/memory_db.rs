//! In-memory [`ObjectDb`] — for fast tests and `schemahub-core`'s unit tests
//! (crate-structure.md §6). Same content-addressing semantics as the redb impl.

use std::collections::HashMap;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::object_db::{ObjectDb, ObjectDbError, ObjectDbResult, ObjectId, ObjectKind, OpId};

fn hash(kind: ObjectKind, bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update([kind.tag()]);
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

#[derive(Debug, Default)]
struct Inner {
    /// (kind_tag, id) → bytes
    objects: HashMap<(u8, Vec<u8>), Vec<u8>>,
    /// repo → (op_id → op bytes)
    ops: HashMap<String, HashMap<Vec<u8>, Vec<u8>>>,
    /// (repo, name) → ref bytes
    refs: HashMap<(String, String), Vec<u8>>,
}

/// A thread-safe in-memory object store.
#[derive(Debug, Default)]
pub struct MemoryObjectDb {
    inner: Mutex<Inner>,
}

impl MemoryObjectDb {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObjectDb for MemoryObjectDb {
    fn put_object(&self, kind: ObjectKind, bytes: &[u8]) -> ObjectDbResult<ObjectId> {
        let id = ObjectId(hash(kind, bytes));
        let mut inner = self.inner.lock().unwrap();
        inner
            .objects
            .entry((kind.tag(), id.0.clone()))
            .or_insert_with(|| bytes.to_vec());
        Ok(id)
    }

    fn put_object_at(&self, kind: ObjectKind, id: &ObjectId, bytes: &[u8]) -> ObjectDbResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .objects
            .entry((kind.tag(), id.0.clone()))
            .or_insert_with(|| bytes.to_vec());
        Ok(())
    }

    fn get_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<Vec<u8>> {
        let inner = self.inner.lock().unwrap();
        inner
            .objects
            .get(&(kind.tag(), id.0.clone()))
            .cloned()
            .ok_or(ObjectDbError::NotFound)
    }

    fn has_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<bool> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.objects.contains_key(&(kind.tag(), id.0.clone())))
    }

    fn list_objects(&self, kind: ObjectKind) -> ObjectDbResult<Vec<ObjectId>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .objects
            .keys()
            .filter(|(tag, _)| *tag == kind.tag())
            .map(|(_, id)| ObjectId(id.clone()))
            .collect())
    }

    fn delete_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.objects.remove(&(kind.tag(), id.0.clone()));
        Ok(())
    }

    fn put_op(&self, repo: &str, op_bytes: &[u8]) -> ObjectDbResult<OpId> {
        let mut hasher = Sha256::new();
        hasher.update(b"op");
        hasher.update(op_bytes);
        let id = OpId(hasher.finalize().to_vec());
        let mut inner = self.inner.lock().unwrap();
        inner
            .ops
            .entry(repo.to_string())
            .or_default()
            .entry(id.0.clone())
            .or_insert_with(|| op_bytes.to_vec());
        Ok(id)
    }

    fn put_op_at(&self, repo: &str, id: &OpId, op_bytes: &[u8]) -> ObjectDbResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .ops
            .entry(repo.to_string())
            .or_default()
            .entry(id.0.clone())
            .or_insert_with(|| op_bytes.to_vec());
        Ok(())
    }

    fn get_op(&self, repo: &str, id: &OpId) -> ObjectDbResult<Vec<u8>> {
        let inner = self.inner.lock().unwrap();
        inner
            .ops
            .get(repo)
            .and_then(|m| m.get(&id.0))
            .cloned()
            .ok_or(ObjectDbError::NotFound)
    }

    fn list_ops(&self, repo: &str) -> ObjectDbResult<Vec<OpId>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .ops
            .get(repo)
            .map(|m| m.keys().map(|k| OpId(k.clone())).collect())
            .unwrap_or_default())
    }

    fn set_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .refs
            .insert((repo.to_string(), name.to_string()), value.to_vec());
        Ok(())
    }

    fn get_ref(&self, repo: &str, name: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .refs
            .get(&(repo.to_string(), name.to_string()))
            .cloned())
    }
}
