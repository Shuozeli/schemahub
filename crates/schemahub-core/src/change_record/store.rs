use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use schemahub_jj::{ObjectDb, ObjectDbError};
use thiserror::Error;

use super::ChangeRecord;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChangeStoreError {
    #[error("change record already exists: {0}")]
    AlreadyExists(String),
    #[error("change record not found: {0}")]
    NotFound(String),
    #[error("change record etag mismatch for {name}: expected {expected}, current {current}")]
    EtagMismatch {
        name: String,
        expected: String,
        current: String,
    },
    #[error("change record store error: {0}")]
    Backend(String),
}

/// Transactional persistence boundary for change records.
///
/// `replace` is a compare-and-set operation: durable implementations must read
/// the current ETag and write the replacement in one database transaction.
pub trait ChangeRecordStore: Send + Sync + 'static {
    fn create(&self, record: ChangeRecord) -> Result<ChangeRecord, ChangeStoreError>;
    fn get(&self, name: &str) -> Result<Option<ChangeRecord>, ChangeStoreError>;
    fn list(&self, project: &str, repo: &str) -> Result<Vec<ChangeRecord>, ChangeStoreError>;
    fn replace(
        &self,
        expected_etag: &str,
        record: ChangeRecord,
    ) -> Result<ChangeRecord, ChangeStoreError>;
}

/// In-memory transactional fake used by core public-behavior tests.
#[derive(Debug, Default)]
pub struct MemoryChangeRecordStore {
    records: Mutex<BTreeMap<String, ChangeRecord>>,
}

impl MemoryChangeRecordStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, ChangeRecord>>, ChangeStoreError> {
        self.records
            .lock()
            .map_err(|error| ChangeStoreError::Backend(format!("poisoned memory store: {error}")))
    }
}

impl ChangeRecordStore for MemoryChangeRecordStore {
    fn create(&self, mut record: ChangeRecord) -> Result<ChangeRecord, ChangeStoreError> {
        let mut records = self.lock()?;
        if records.contains_key(&record.name) {
            return Err(ChangeStoreError::AlreadyExists(record.name));
        }
        record.etag = "v1".to_string();
        records.insert(record.name.clone(), record.clone());
        Ok(record)
    }

    fn get(&self, name: &str) -> Result<Option<ChangeRecord>, ChangeStoreError> {
        let records = self.lock()?;
        Ok(records.get(name).cloned())
    }

    fn list(&self, project: &str, repo: &str) -> Result<Vec<ChangeRecord>, ChangeStoreError> {
        let records = self.lock()?;
        let mut found: Vec<_> = records
            .values()
            .filter(|record| record.project == project && record.repo == repo)
            .cloned()
            .collect();
        found.sort_by(|left, right| {
            left.create_time_unix_ms
                .cmp(&right.create_time_unix_ms)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(found)
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut record: ChangeRecord,
    ) -> Result<ChangeRecord, ChangeStoreError> {
        let mut records = self.lock()?;
        let current = records
            .get(&record.name)
            .ok_or_else(|| ChangeStoreError::NotFound(record.name.clone()))?;
        if current.etag != expected_etag {
            return Err(ChangeStoreError::EtagMismatch {
                name: record.name,
                expected: expected_etag.to_string(),
                current: current.etag.clone(),
            });
        }
        let version = current
            .etag
            .strip_prefix('v')
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                ChangeStoreError::Backend(format!("invalid stored etag: {}", current.etag))
            })?;
        record.etag = format!("v{}", version + 1);
        records.insert(record.name.clone(), record.clone());
        Ok(record)
    }
}

const CHANGE_RECORD_COLLECTION: &str = "schemahub.change_records.v1";

/// Durable change-record store over the same [`ObjectDb`] used by the JJ
/// layer. Redb and PostgreSQL therefore share one deployment/persistence
/// selection while keeping mutable records outside JJ's content-addressed
/// object namespace.
#[derive(Debug)]
pub struct ObjectDbChangeRecordStore {
    db: Arc<dyn ObjectDb>,
}

impl ObjectDbChangeRecordStore {
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    fn encode(record: &ChangeRecord) -> Result<Vec<u8>, ChangeStoreError> {
        serde_json::to_vec(record)
            .map_err(|error| ChangeStoreError::Backend(format!("encode change record: {error}")))
    }

    fn decode(bytes: &[u8]) -> Result<ChangeRecord, ChangeStoreError> {
        serde_json::from_slice(bytes)
            .map_err(|error| ChangeStoreError::Backend(format!("decode change record: {error}")))
    }

    fn map_db(error: ObjectDbError) -> ChangeStoreError {
        ChangeStoreError::Backend(error.to_string())
    }
}

impl ChangeRecordStore for ObjectDbChangeRecordStore {
    fn create(&self, mut record: ChangeRecord) -> Result<ChangeRecord, ChangeStoreError> {
        record.etag = "v1".to_string();
        let bytes = Self::encode(&record)?;
        let inserted = self
            .db
            .create_record(CHANGE_RECORD_COLLECTION, &record.name, &bytes)
            .map_err(Self::map_db)?;
        if !inserted {
            return Err(ChangeStoreError::AlreadyExists(record.name));
        }
        Ok(record)
    }

    fn get(&self, name: &str) -> Result<Option<ChangeRecord>, ChangeStoreError> {
        self.db
            .get_record(CHANGE_RECORD_COLLECTION, name)
            .map_err(Self::map_db)?
            .map(|bytes| Self::decode(&bytes))
            .transpose()
    }

    fn list(&self, project: &str, repo: &str) -> Result<Vec<ChangeRecord>, ChangeStoreError> {
        let mut found = Vec::new();
        for (key, bytes) in self
            .db
            .list_records(CHANGE_RECORD_COLLECTION)
            .map_err(Self::map_db)?
        {
            let record = Self::decode(&bytes)?;
            if record.name != key {
                return Err(ChangeStoreError::Backend(format!(
                    "change record key/name mismatch: key={key:?}, name={:?}",
                    record.name
                )));
            }
            if record.project == project && record.repo == repo {
                found.push(record);
            }
        }
        found.sort_by(|left, right| {
            left.create_time_unix_ms
                .cmp(&right.create_time_unix_ms)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(found)
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut record: ChangeRecord,
    ) -> Result<ChangeRecord, ChangeStoreError> {
        let current_bytes = self
            .db
            .get_record(CHANGE_RECORD_COLLECTION, &record.name)
            .map_err(Self::map_db)?
            .ok_or_else(|| ChangeStoreError::NotFound(record.name.clone()))?;
        let current = Self::decode(&current_bytes)?;
        if current.etag != expected_etag {
            return Err(ChangeStoreError::EtagMismatch {
                name: record.name,
                expected: expected_etag.to_string(),
                current: current.etag,
            });
        }
        let version = current
            .etag
            .strip_prefix('v')
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                ChangeStoreError::Backend(format!("invalid stored etag: {}", current.etag))
            })?;
        record.etag = format!("v{}", version + 1);
        let replacement = Self::encode(&record)?;
        let replaced = self
            .db
            .compare_and_swap_record(
                CHANGE_RECORD_COLLECTION,
                &record.name,
                &current_bytes,
                &replacement,
            )
            .map_err(Self::map_db)?;
        if replaced {
            return Ok(record);
        }

        let latest = self
            .db
            .get_record(CHANGE_RECORD_COLLECTION, &record.name)
            .map_err(Self::map_db)?
            .ok_or_else(|| ChangeStoreError::NotFound(record.name.clone()))
            .and_then(|bytes| Self::decode(&bytes))?;
        Err(ChangeStoreError::EtagMismatch {
            name: record.name,
            expected: expected_etag.to_string(),
            current: latest.etag,
        })
    }
}
