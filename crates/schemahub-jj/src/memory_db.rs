//! In-memory [`ObjectDb`] — for fast tests and `schemahub-core`'s unit tests
//! (crate-structure.md §6). Same content-addressing semantics as the redb impl.

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use sha2::{Digest, Sha256};

use crate::object_db::{
    ObjectDb, ObjectDbError, ObjectDbLockGuard, ObjectDbResult, ObjectId, ObjectKind, OpId,
};

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
    /// (collection, stable resource key) → serialized resource bytes
    records: HashMap<(String, String), Vec<u8>>,
}

/// A thread-safe in-memory object store.
#[derive(Debug, Default)]
pub struct MemoryObjectDb {
    inner: Mutex<Inner>,
    maintenance: RwLock<()>,
    publication: Mutex<()>,
}

impl MemoryObjectDb {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObjectDb for MemoryObjectDb {
    fn acquire_mutation_guard(&self) -> ObjectDbResult<Box<dyn ObjectDbLockGuard + '_>> {
        self.maintenance
            .read()
            .map(|guard| Box::new(guard) as Box<dyn ObjectDbLockGuard>)
            .map_err(|error| ObjectDbError::Backend(format!("poisoned maintenance lock: {error}")))
    }

    fn acquire_publication_guard(
        &self,
        _repo: &str,
    ) -> ObjectDbResult<Box<dyn ObjectDbLockGuard + '_>> {
        self.publication
            .lock()
            .map(|guard| Box::new(guard) as Box<dyn ObjectDbLockGuard>)
            .map_err(|error| ObjectDbError::Backend(format!("poisoned publication lock: {error}")))
    }

    fn acquire_gc_guard(&self) -> ObjectDbResult<Box<dyn ObjectDbLockGuard + '_>> {
        self.maintenance
            .write()
            .map(|guard| Box::new(guard) as Box<dyn ObjectDbLockGuard>)
            .map_err(|error| ObjectDbError::Backend(format!("poisoned maintenance lock: {error}")))
    }

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

    fn list_repo_keys(&self) -> ObjectDbResult<Vec<String>> {
        let inner = self.inner.lock().unwrap();
        let mut repos: std::collections::BTreeSet<_> = inner.ops.keys().cloned().collect();
        repos.extend(inner.refs.keys().map(|(repo, _)| repo.clone()));
        Ok(repos.into_iter().collect())
    }

    fn set_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .refs
            .insert((repo.to_string(), name.to_string()), value.to_vec());
        Ok(())
    }

    fn create_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<bool> {
        let mut inner = self.inner.lock().unwrap();
        let key = (repo.to_string(), name.to_string());
        match inner.refs.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value.to_vec());
                Ok(true)
            }
            std::collections::hash_map::Entry::Occupied(_) => Ok(false),
        }
    }

    fn get_ref(&self, repo: &str, name: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .refs
            .get(&(repo.to_string(), name.to_string()))
            .cloned())
    }

    fn create_record(&self, collection: &str, key: &str, value: &[u8]) -> ObjectDbResult<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| ObjectDbError::Backend(format!("poisoned memory db: {error}")))?;
        let record_key = (collection.to_string(), key.to_string());
        if inner.records.contains_key(&record_key) {
            return Ok(false);
        }
        inner.records.insert(record_key, value.to_vec());
        Ok(true)
    }

    fn create_records(&self, records: &[(&str, &str, &[u8])]) -> ObjectDbResult<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| ObjectDbError::Backend(format!("poisoned memory db: {error}")))?;
        let mut keys = std::collections::HashSet::with_capacity(records.len());
        for (collection, key, _) in records {
            let record_key = ((*collection).to_string(), (*key).to_string());
            if !keys.insert(record_key.clone()) || inner.records.contains_key(&record_key) {
                return Ok(false);
            }
        }
        for (collection, key, value) in records {
            inner.records.insert(
                ((*collection).to_string(), (*key).to_string()),
                (*value).to_vec(),
            );
        }
        Ok(true)
    }

    fn get_record(&self, collection: &str, key: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let inner = self
            .inner
            .lock()
            .map_err(|error| ObjectDbError::Backend(format!("poisoned memory db: {error}")))?;
        Ok(inner
            .records
            .get(&(collection.to_string(), key.to_string()))
            .cloned())
    }

    fn list_records(&self, collection: &str) -> ObjectDbResult<Vec<(String, Vec<u8>)>> {
        let inner = self
            .inner
            .lock()
            .map_err(|error| ObjectDbError::Backend(format!("poisoned memory db: {error}")))?;
        Ok(inner
            .records
            .iter()
            .filter(|((record_collection, _), _)| record_collection == collection)
            .map(|((_, key), value)| (key.clone(), value.clone()))
            .collect())
    }

    fn compare_and_swap_record(
        &self,
        collection: &str,
        key: &str,
        expected: &[u8],
        replacement: &[u8],
    ) -> ObjectDbResult<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| ObjectDbError::Backend(format!("poisoned memory db: {error}")))?;
        let record_key = (collection.to_string(), key.to_string());
        let Some(current) = inner.records.get_mut(&record_key) else {
            return Ok(false);
        };
        if current.as_slice() != expected {
            return Ok(false);
        }
        *current = replacement.to_vec();
        Ok(true)
    }

    fn compare_and_delete_record(
        &self,
        collection: &str,
        key: &str,
        expected: &[u8],
    ) -> ObjectDbResult<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| ObjectDbError::Backend(format!("poisoned memory db: {error}")))?;
        let record_key = (collection.to_string(), key.to_string());
        if inner.records.get(&record_key).map(Vec::as_slice) != Some(expected) {
            return Ok(false);
        }
        inner.records.remove(&record_key);
        Ok(true)
    }
}
