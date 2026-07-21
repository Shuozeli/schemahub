//! `redb`-backed [`ObjectDb`] — the embedded default (design.md §4.5).
//!
//! Single-file, MVCC, zero-ops. Layout:
//! - one `objects` table keyed by `kind_tag ++ content_hash` → bytes;
//! - one `ops` table keyed by `repo ++ 0x00 ++ op_id` → op record bytes;
//! - one `refs` table keyed by `repo ++ 0x00 ++ name` → ref bytes.
//! - one `records` table keyed by `collection ++ 0x00 ++ stable_key` → bytes.
//!
//! Ids are content hashes (`sha256`), so `put_object` is inherently idempotent
//! and objects dedup globally across repos.

use std::path::Path;
use std::sync::{Mutex, RwLock};

use redb::{Database, ReadableTable, TableDefinition};
use sha2::{Digest, Sha256};

use crate::object_db::{
    ObjectDb, ObjectDbError, ObjectDbLockGuard, ObjectDbResult, ObjectId, ObjectKind, OpId,
};

const OBJECTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("objects");
const OPS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("ops");
const REFS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("refs");
const RECORDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("records");

fn map_db<E: std::fmt::Display>(e: E) -> ObjectDbError {
    ObjectDbError::Backend(e.to_string())
}

/// `sha256(tag ++ bytes)` — kind-tagged content hash.
fn hash(kind: ObjectKind, bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update([kind.tag()]);
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

/// `kind_tag ++ id` — the objects-table key.
fn object_key(kind: ObjectKind, id: &ObjectId) -> Vec<u8> {
    let mut k = Vec::with_capacity(1 + id.0.len());
    k.push(kind.tag());
    k.extend_from_slice(&id.0);
    k
}

/// `repo ++ 0x00 ++ suffix` — a repo-scoped key for the ops/refs tables.
fn scoped_key(repo: &str, suffix: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(repo.len() + 1 + suffix.len());
    k.extend_from_slice(repo.as_bytes());
    k.push(0);
    k.extend_from_slice(suffix);
    k
}

/// Single-file, MVCC, zero-ops object store.
#[derive(Debug)]
pub struct RedbObjectDb {
    db: Database,
    maintenance: RwLock<()>,
    publication: Mutex<()>,
}

impl RedbObjectDb {
    /// Open (or create) the database at `path` and define its tables.
    pub fn open(path: impl AsRef<Path>) -> ObjectDbResult<Self> {
        let db = Database::create(path).map_err(map_db)?;
        // Eagerly create tables so reads on a fresh db do not error.
        let wtx = db.begin_write().map_err(map_db)?;
        {
            wtx.open_table(OBJECTS).map_err(map_db)?;
            wtx.open_table(OPS).map_err(map_db)?;
            wtx.open_table(REFS).map_err(map_db)?;
            wtx.open_table(RECORDS).map_err(map_db)?;
        }
        wtx.commit().map_err(map_db)?;
        Ok(Self {
            db,
            maintenance: RwLock::new(()),
            publication: Mutex::new(()),
        })
    }
}

impl ObjectDb for RedbObjectDb {
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
        let key = object_key(kind, &id);
        let wtx = self.db.begin_write().map_err(map_db)?;
        {
            let mut table = wtx.open_table(OBJECTS).map_err(map_db)?;
            // Idempotent: only write if absent.
            if table.get(key.as_slice()).map_err(map_db)?.is_none() {
                table.insert(key.as_slice(), bytes).map_err(map_db)?;
            }
        }
        wtx.commit().map_err(map_db)?;
        Ok(id)
    }

    fn put_object_at(&self, kind: ObjectKind, id: &ObjectId, bytes: &[u8]) -> ObjectDbResult<()> {
        let key = object_key(kind, id);
        let wtx = self.db.begin_write().map_err(map_db)?;
        {
            let mut table = wtx.open_table(OBJECTS).map_err(map_db)?;
            if table.get(key.as_slice()).map_err(map_db)?.is_none() {
                table.insert(key.as_slice(), bytes).map_err(map_db)?;
            }
        }
        wtx.commit().map_err(map_db)?;
        Ok(())
    }

    fn get_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<Vec<u8>> {
        let key = object_key(kind, id);
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(OBJECTS).map_err(map_db)?;
        match table.get(key.as_slice()).map_err(map_db)? {
            Some(v) => Ok(v.value().to_vec()),
            None => Err(ObjectDbError::NotFound),
        }
    }

    fn has_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<bool> {
        let key = object_key(kind, id);
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(OBJECTS).map_err(map_db)?;
        Ok(table.get(key.as_slice()).map_err(map_db)?.is_some())
    }

    fn list_objects(&self, kind: ObjectKind) -> ObjectDbResult<Vec<ObjectId>> {
        let tag = kind.tag();
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(OBJECTS).map_err(map_db)?;
        let mut out = Vec::new();
        for entry in table.iter().map_err(map_db)? {
            let (k, _) = entry.map_err(map_db)?;
            let key = k.value();
            if key.first() == Some(&tag) {
                out.push(ObjectId(key[1..].to_vec()));
            }
        }
        Ok(out)
    }

    fn delete_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<()> {
        let key = object_key(kind, id);
        let wtx = self.db.begin_write().map_err(map_db)?;
        {
            let mut table = wtx.open_table(OBJECTS).map_err(map_db)?;
            table.remove(key.as_slice()).map_err(map_db)?;
        }
        wtx.commit().map_err(map_db)?;
        Ok(())
    }

    fn put_op(&self, repo: &str, op_bytes: &[u8]) -> ObjectDbResult<OpId> {
        // Op ids are content hashes of the operation record.
        let mut hasher = Sha256::new();
        hasher.update(b"op");
        hasher.update(op_bytes);
        let id = OpId(hasher.finalize().to_vec());
        let key = scoped_key(repo, &id.0);
        let wtx = self.db.begin_write().map_err(map_db)?;
        {
            let mut table = wtx.open_table(OPS).map_err(map_db)?;
            if table.get(key.as_slice()).map_err(map_db)?.is_none() {
                table.insert(key.as_slice(), op_bytes).map_err(map_db)?;
            }
        }
        wtx.commit().map_err(map_db)?;
        Ok(id)
    }

    fn put_op_at(&self, repo: &str, id: &OpId, op_bytes: &[u8]) -> ObjectDbResult<()> {
        let key = scoped_key(repo, &id.0);
        let wtx = self.db.begin_write().map_err(map_db)?;
        {
            let mut table = wtx.open_table(OPS).map_err(map_db)?;
            if table.get(key.as_slice()).map_err(map_db)?.is_none() {
                table.insert(key.as_slice(), op_bytes).map_err(map_db)?;
            }
        }
        wtx.commit().map_err(map_db)?;
        Ok(())
    }

    fn get_op(&self, repo: &str, id: &OpId) -> ObjectDbResult<Vec<u8>> {
        let key = scoped_key(repo, &id.0);
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(OPS).map_err(map_db)?;
        match table.get(key.as_slice()).map_err(map_db)? {
            Some(v) => Ok(v.value().to_vec()),
            None => Err(ObjectDbError::NotFound),
        }
    }

    fn list_ops(&self, repo: &str) -> ObjectDbResult<Vec<OpId>> {
        let prefix = {
            let mut p = repo.as_bytes().to_vec();
            p.push(0);
            p
        };
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(OPS).map_err(map_db)?;
        let mut out = Vec::new();
        for entry in table.iter().map_err(map_db)? {
            let (k, _) = entry.map_err(map_db)?;
            let key = k.value();
            if key.starts_with(&prefix) {
                out.push(OpId(key[prefix.len()..].to_vec()));
            }
        }
        Ok(out)
    }

    fn list_repo_keys(&self) -> ObjectDbResult<Vec<String>> {
        let rtx = self.db.begin_read().map_err(map_db)?;
        let ops = rtx.open_table(OPS).map_err(map_db)?;
        let refs = rtx.open_table(REFS).map_err(map_db)?;
        let mut repos = std::collections::BTreeSet::new();
        for table in [&ops, &refs] {
            for entry in table.iter().map_err(map_db)? {
                let (key, _) = entry.map_err(map_db)?;
                let key = key.value();
                let separator = key.iter().position(|byte| *byte == 0).ok_or_else(|| {
                    ObjectDbError::Backend("malformed repository-scoped key".to_string())
                })?;
                let repo = std::str::from_utf8(&key[..separator]).map_err(map_db)?;
                repos.insert(repo.to_string());
            }
        }
        Ok(repos.into_iter().collect())
    }

    fn set_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<()> {
        let key = scoped_key(repo, name.as_bytes());
        let wtx = self.db.begin_write().map_err(map_db)?;
        {
            let mut table = wtx.open_table(REFS).map_err(map_db)?;
            table.insert(key.as_slice(), value).map_err(map_db)?;
        }
        wtx.commit().map_err(map_db)?;
        Ok(())
    }

    fn create_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<bool> {
        let key = scoped_key(repo, name.as_bytes());
        let wtx = self.db.begin_write().map_err(map_db)?;
        let inserted = {
            let mut table = wtx.open_table(REFS).map_err(map_db)?;
            if table.get(key.as_slice()).map_err(map_db)?.is_some() {
                false
            } else {
                table.insert(key.as_slice(), value).map_err(map_db)?;
                true
            }
        };
        wtx.commit().map_err(map_db)?;
        Ok(inserted)
    }

    fn get_ref(&self, repo: &str, name: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let key = scoped_key(repo, name.as_bytes());
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(REFS).map_err(map_db)?;
        Ok(table
            .get(key.as_slice())
            .map_err(map_db)?
            .map(|v| v.value().to_vec()))
    }

    fn create_record(&self, collection: &str, key: &str, value: &[u8]) -> ObjectDbResult<bool> {
        let record_key = scoped_key(collection, key.as_bytes());
        let wtx = self.db.begin_write().map_err(map_db)?;
        let inserted = {
            let mut table = wtx.open_table(RECORDS).map_err(map_db)?;
            if table.get(record_key.as_slice()).map_err(map_db)?.is_some() {
                false
            } else {
                table.insert(record_key.as_slice(), value).map_err(map_db)?;
                true
            }
        };
        wtx.commit().map_err(map_db)?;
        Ok(inserted)
    }

    fn create_records(&self, records: &[(&str, &str, &[u8])]) -> ObjectDbResult<bool> {
        let keys: Vec<_> = records
            .iter()
            .map(|(collection, key, _)| scoped_key(collection, key.as_bytes()))
            .collect();
        let wtx = self.db.begin_write().map_err(map_db)?;
        let inserted = {
            let mut table = wtx.open_table(RECORDS).map_err(map_db)?;
            let mut unique = std::collections::HashSet::with_capacity(keys.len());
            let mut available = true;
            for key in &keys {
                if !unique.insert(key.clone())
                    || table.get(key.as_slice()).map_err(map_db)?.is_some()
                {
                    available = false;
                    break;
                }
            }
            if available {
                for ((_, _, value), key) in records.iter().zip(&keys) {
                    table.insert(key.as_slice(), *value).map_err(map_db)?;
                }
            }
            available
        };
        wtx.commit().map_err(map_db)?;
        Ok(inserted)
    }

    fn get_record(&self, collection: &str, key: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let record_key = scoped_key(collection, key.as_bytes());
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(RECORDS).map_err(map_db)?;
        Ok(table
            .get(record_key.as_slice())
            .map_err(map_db)?
            .map(|value| value.value().to_vec()))
    }

    fn list_records(&self, collection: &str) -> ObjectDbResult<Vec<(String, Vec<u8>)>> {
        let mut prefix = collection.as_bytes().to_vec();
        prefix.push(0);
        let rtx = self.db.begin_read().map_err(map_db)?;
        let table = rtx.open_table(RECORDS).map_err(map_db)?;
        let mut records = Vec::new();
        for entry in table.iter().map_err(map_db)? {
            let (key, value) = entry.map_err(map_db)?;
            let bytes = key.value();
            if !bytes.starts_with(&prefix) {
                continue;
            }
            let stable_key = String::from_utf8(bytes[prefix.len()..].to_vec())
                .map_err(|error| ObjectDbError::Backend(error.to_string()))?;
            records.push((stable_key, value.value().to_vec()));
        }
        Ok(records)
    }

    fn compare_and_swap_record(
        &self,
        collection: &str,
        key: &str,
        expected: &[u8],
        replacement: &[u8],
    ) -> ObjectDbResult<bool> {
        let record_key = scoped_key(collection, key.as_bytes());
        let wtx = self.db.begin_write().map_err(map_db)?;
        let replaced = {
            let mut table = wtx.open_table(RECORDS).map_err(map_db)?;
            let matches = table
                .get(record_key.as_slice())
                .map_err(map_db)?
                .is_some_and(|value| value.value() == expected);
            if matches {
                table
                    .insert(record_key.as_slice(), replacement)
                    .map_err(map_db)?;
            }
            matches
        };
        wtx.commit().map_err(map_db)?;
        Ok(replaced)
    }

    fn compare_and_delete_record(
        &self,
        collection: &str,
        key: &str,
        expected: &[u8],
    ) -> ObjectDbResult<bool> {
        let record_key = scoped_key(collection, key.as_bytes());
        let wtx = self.db.begin_write().map_err(map_db)?;
        let deleted = {
            let mut table = wtx.open_table(RECORDS).map_err(map_db)?;
            let matches = table
                .get(record_key.as_slice())
                .map_err(map_db)?
                .is_some_and(|value| value.value() == expected);
            if matches {
                table.remove(record_key.as_slice()).map_err(map_db)?;
            }
            matches
        };
        wtx.commit().map_err(map_db)?;
        Ok(deleted)
    }
}
