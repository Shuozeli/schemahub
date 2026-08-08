use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use schemahub_jj::{ObjectDb, ObjectDbError, RecordMutation};
use thiserror::Error;

use super::{ChangeRecord, ChangeRecordStatus};

const DEFAULT_INTERNAL_PAGE_SIZE: usize = 256;

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

/// Stable creation-order cursor used below transport-specific opaque tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRecordPageCursor {
    pub create_time_unix_ms: i64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRecordPage {
    pub records: Vec<ChangeRecord>,
    pub next_cursor: Option<ChangeRecordPageCursor>,
}

/// Transactional persistence boundary for change records.
///
/// `replace` is a compare-and-set operation: durable implementations must read
/// the current ETag and write the replacement in one database transaction.
pub trait ChangeRecordStore: Send + Sync + 'static {
    fn create(&self, record: ChangeRecord) -> Result<ChangeRecord, ChangeStoreError>;
    fn get(&self, name: &str) -> Result<Option<ChangeRecord>, ChangeStoreError>;
    fn list_page(
        &self,
        project: &str,
        repo: &str,
        status_filter: Option<ChangeRecordStatus>,
        start_after: Option<&ChangeRecordPageCursor>,
        limit: usize,
    ) -> Result<ChangeRecordPage, ChangeStoreError>;
    fn list(&self, project: &str, repo: &str) -> Result<Vec<ChangeRecord>, ChangeStoreError> {
        let mut records = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.list_page(
                project,
                repo,
                None,
                cursor.as_ref(),
                DEFAULT_INTERNAL_PAGE_SIZE,
            )?;
            records.extend(page.records);
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(records)
    }
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

    fn list_page(
        &self,
        project: &str,
        repo: &str,
        status_filter: Option<ChangeRecordStatus>,
        start_after: Option<&ChangeRecordPageCursor>,
        limit: usize,
    ) -> Result<ChangeRecordPage, ChangeStoreError> {
        if limit == 0 {
            return Ok(ChangeRecordPage {
                records: Vec::new(),
                next_cursor: None,
            });
        }
        let records = self.lock()?;
        let mut found: Vec<_> = records
            .values()
            .filter(|record| {
                record.project == project
                    && record.repo == repo
                    && status_filter.is_none_or(|status| record.status == status)
                    && start_after
                        .is_none_or(|cursor| compare_record_to_cursor(record, cursor).is_gt())
            })
            .cloned()
            .collect();
        found.sort_by(compare_records);
        let has_more = found.len() > limit;
        found.truncate(limit);
        let next_cursor = has_more
            .then(|| found.last().map(cursor_from_record))
            .flatten();
        Ok(ChangeRecordPage {
            records: found,
            next_cursor,
        })
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
        validate_immutable_coordinates(current, &record)?;
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
const CHANGE_RECORD_INDEX_PREFIX: &str = "schemahub.change_record_index.v1";
const CHANGE_RECORD_INDEX_MIGRATION_COLLECTION: &str = "schemahub.change_record_index_migration.v1";
const CHANGE_RECORD_INDEX_MIGRATION_KEY: &str = "complete";
const CHANGE_RECORD_INDEX_MIGRATION_VALUE: &[u8] = b"schemahub.change_record_index.v1";
const MAX_INDEX_MIGRATION_RETRIES: usize = 8;

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

    fn ensure_indexes(&self) -> Result<(), ChangeStoreError> {
        for _ in 0..MAX_INDEX_MIGRATION_RETRIES {
            match self
                .db
                .get_record(
                    CHANGE_RECORD_INDEX_MIGRATION_COLLECTION,
                    CHANGE_RECORD_INDEX_MIGRATION_KEY,
                )
                .map_err(Self::map_db)?
            {
                Some(value) if value == CHANGE_RECORD_INDEX_MIGRATION_VALUE => return Ok(()),
                Some(_) => {
                    return Err(ChangeStoreError::Backend(
                        "change-record index migration marker is malformed".to_string(),
                    ));
                }
                None => {}
            }

            let mut missing = Vec::new();
            for (key, bytes) in self
                .db
                .list_records(CHANGE_RECORD_COLLECTION)
                .map_err(Self::map_db)?
            {
                let record = Self::decode(&bytes)?;
                validate_record_key(&record, &key)?;
                for collection in index_collections(&record) {
                    let index_key = change_record_index_key(&record)?;
                    match self
                        .db
                        .get_record(&collection, &index_key)
                        .map_err(Self::map_db)?
                    {
                        Some(value) if value == record.name.as_bytes() => {}
                        Some(_) => {
                            return Err(ChangeStoreError::Backend(format!(
                                "change-record index {collection:?}/{index_key:?} \
                                 does not identify {:?}",
                                record.name
                            )));
                        }
                        None => {
                            missing.push((collection, index_key, record.name.as_bytes().to_vec()))
                        }
                    }
                }
            }
            missing.push((
                CHANGE_RECORD_INDEX_MIGRATION_COLLECTION.to_string(),
                CHANGE_RECORD_INDEX_MIGRATION_KEY.to_string(),
                CHANGE_RECORD_INDEX_MIGRATION_VALUE.to_vec(),
            ));
            let mutations: Vec<_> = missing
                .iter()
                .map(|(collection, key, value)| RecordMutation::Create {
                    collection,
                    key,
                    value,
                })
                .collect();
            if self.db.transact_records(&mutations).map_err(Self::map_db)? {
                return Ok(());
            }
        }
        Err(ChangeStoreError::Backend(
            "change-record index migration did not converge".to_string(),
        ))
    }

    fn failed_replace(
        &self,
        expected_etag: &str,
        record: ChangeRecord,
    ) -> Result<ChangeRecord, ChangeStoreError> {
        let latest = self
            .db
            .get_record(CHANGE_RECORD_COLLECTION, &record.name)
            .map_err(Self::map_db)?
            .ok_or_else(|| ChangeStoreError::NotFound(record.name.clone()))
            .and_then(|bytes| Self::decode(&bytes))?;
        if latest.etag != expected_etag {
            return Err(ChangeStoreError::EtagMismatch {
                name: record.name,
                expected: expected_etag.to_string(),
                current: latest.etag,
            });
        }
        Err(ChangeStoreError::Backend(format!(
            "change-record index precondition failed for {:?}",
            record.name
        )))
    }
}

impl ChangeRecordStore for ObjectDbChangeRecordStore {
    fn create(&self, mut record: ChangeRecord) -> Result<ChangeRecord, ChangeStoreError> {
        self.ensure_indexes()?;
        record.etag = "v1".to_string();
        let bytes = Self::encode(&record)?;
        let index_key = change_record_index_key(&record)?;
        let [all_collection, status_collection] = index_collections(&record);
        let mutations = [
            RecordMutation::Create {
                collection: CHANGE_RECORD_COLLECTION,
                key: &record.name,
                value: &bytes,
            },
            RecordMutation::Create {
                collection: &all_collection,
                key: &index_key,
                value: record.name.as_bytes(),
            },
            RecordMutation::Create {
                collection: &status_collection,
                key: &index_key,
                value: record.name.as_bytes(),
            },
        ];
        if self.db.transact_records(&mutations).map_err(Self::map_db)? {
            return Ok(record);
        }
        if self
            .db
            .get_record(CHANGE_RECORD_COLLECTION, &record.name)
            .map_err(Self::map_db)?
            .is_some()
        {
            return Err(ChangeStoreError::AlreadyExists(record.name));
        }
        Err(ChangeStoreError::Backend(format!(
            "change-record index collision while creating {:?}",
            record.name
        )))
    }

    fn get(&self, name: &str) -> Result<Option<ChangeRecord>, ChangeStoreError> {
        self.db
            .get_record(CHANGE_RECORD_COLLECTION, name)
            .map_err(Self::map_db)?
            .map(|bytes| Self::decode(&bytes))
            .transpose()
    }

    fn list_page(
        &self,
        project: &str,
        repo: &str,
        status_filter: Option<ChangeRecordStatus>,
        start_after: Option<&ChangeRecordPageCursor>,
        limit: usize,
    ) -> Result<ChangeRecordPage, ChangeStoreError> {
        if limit == 0 {
            return Ok(ChangeRecordPage {
                records: Vec::new(),
                next_cursor: None,
            });
        }
        self.ensure_indexes()?;
        let collection = change_record_index_collection(project, repo, status_filter);
        let start_after = start_after
            .map(|cursor| change_record_cursor_key(project, repo, cursor))
            .transpose()?;
        let fetch_limit = limit.checked_add(1).ok_or_else(|| {
            ChangeStoreError::Backend("change-record page limit overflow".to_string())
        })?;
        let mut rows = self
            .db
            .list_records_page(&collection, start_after.as_deref(), fetch_limit)
            .map_err(Self::map_db)?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let mut records = Vec::with_capacity(rows.len());
        for (index_key, record_name_bytes) in rows {
            let record_name = std::str::from_utf8(&record_name_bytes).map_err(|error| {
                ChangeStoreError::Backend(format!(
                    "change-record index {collection:?}/{index_key:?} \
                     contains an invalid resource name: {error}"
                ))
            })?;
            let bytes = self
                .db
                .get_record(CHANGE_RECORD_COLLECTION, record_name)
                .map_err(Self::map_db)?
                .ok_or_else(|| {
                    ChangeStoreError::Backend(format!(
                        "change-record index {collection:?}/{index_key:?} \
                         points to missing record {record_name:?}"
                    ))
                })?;
            let record = Self::decode(&bytes)?;
            validate_record_key(&record, record_name)?;
            if record.project != project || record.repo != repo {
                return Err(ChangeStoreError::Backend(format!(
                    "change-record index scope mismatch for {:?}",
                    record.name
                )));
            }
            if status_filter.is_some_and(|status| record.status != status) {
                return Err(ChangeStoreError::Backend(format!(
                    "change-record status index mismatch for {:?}",
                    record.name
                )));
            }
            let expected_index_key = change_record_index_key(&record)?;
            if index_key != expected_index_key {
                return Err(ChangeStoreError::Backend(format!(
                    "change-record index key mismatch: key={index_key:?}, \
                     expected={expected_index_key:?}"
                )));
            }
            records.push(record);
        }
        let next_cursor = has_more
            .then(|| records.last().map(cursor_from_record))
            .flatten();
        Ok(ChangeRecordPage {
            records,
            next_cursor,
        })
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut record: ChangeRecord,
    ) -> Result<ChangeRecord, ChangeStoreError> {
        self.ensure_indexes()?;
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
        validate_immutable_coordinates(&current, &record)?;
        let version = current
            .etag
            .strip_prefix('v')
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                ChangeStoreError::Backend(format!("invalid stored etag: {}", current.etag))
            })?;
        record.etag = format!("v{}", version + 1);
        let replacement = Self::encode(&record)?;
        if current.status == record.status {
            if self
                .db
                .compare_and_swap_record(
                    CHANGE_RECORD_COLLECTION,
                    &record.name,
                    &current_bytes,
                    &replacement,
                )
                .map_err(Self::map_db)?
            {
                return Ok(record);
            }
            return self.failed_replace(expected_etag, record);
        }
        let index_key = change_record_index_key(&current)?;
        let old_status_collection =
            change_record_index_collection(&current.project, &current.repo, Some(current.status));
        let new_status_collection =
            change_record_index_collection(&record.project, &record.repo, Some(record.status));
        let mutations = [
            RecordMutation::CompareAndSwap {
                collection: CHANGE_RECORD_COLLECTION,
                key: &record.name,
                expected: &current_bytes,
                replacement: &replacement,
            },
            RecordMutation::CompareAndDelete {
                collection: &old_status_collection,
                key: &index_key,
                expected: record.name.as_bytes(),
            },
            RecordMutation::Create {
                collection: &new_status_collection,
                key: &index_key,
                value: record.name.as_bytes(),
            },
        ];
        if self.db.transact_records(&mutations).map_err(Self::map_db)? {
            return Ok(record);
        }
        self.failed_replace(expected_etag, record)
    }
}

fn compare_records(left: &ChangeRecord, right: &ChangeRecord) -> Ordering {
    left.create_time_unix_ms
        .cmp(&right.create_time_unix_ms)
        .then_with(|| left.name.cmp(&right.name))
}

fn compare_record_to_cursor(record: &ChangeRecord, cursor: &ChangeRecordPageCursor) -> Ordering {
    record
        .create_time_unix_ms
        .cmp(&cursor.create_time_unix_ms)
        .then_with(|| record.name.cmp(&cursor.name))
}

fn cursor_from_record(record: &ChangeRecord) -> ChangeRecordPageCursor {
    ChangeRecordPageCursor {
        create_time_unix_ms: record.create_time_unix_ms,
        name: record.name.clone(),
    }
}

fn status_segment(status: ChangeRecordStatus) -> &'static str {
    match status {
        ChangeRecordStatus::Draft => "draft",
        ChangeRecordStatus::Ready => "ready",
        ChangeRecordStatus::Applying => "applying",
        ChangeRecordStatus::Applied => "applied",
        ChangeRecordStatus::Rejected => "rejected",
        ChangeRecordStatus::Abandoned => "abandoned",
    }
}

fn change_record_index_collection(
    project: &str,
    repo: &str,
    status_filter: Option<ChangeRecordStatus>,
) -> String {
    let suffix = status_filter
        .map(|status| format!("status/{}", status_segment(status)))
        .unwrap_or_else(|| "all".to_string());
    format!(
        "{CHANGE_RECORD_INDEX_PREFIX}/projects/{}/repos/{repo_hex}/{suffix}",
        hex::encode(project.as_bytes()),
        repo_hex = hex::encode(repo.as_bytes())
    )
}

fn index_collections(record: &ChangeRecord) -> [String; 2] {
    [
        change_record_index_collection(&record.project, &record.repo, None),
        change_record_index_collection(&record.project, &record.repo, Some(record.status)),
    ]
}

fn change_record_index_key(record: &ChangeRecord) -> Result<String, ChangeStoreError> {
    change_record_cursor_key(
        &record.project,
        &record.repo,
        &ChangeRecordPageCursor {
            create_time_unix_ms: record.create_time_unix_ms,
            name: record.name.clone(),
        },
    )
}

fn change_record_cursor_key(
    project: &str,
    repo: &str,
    cursor: &ChangeRecordPageCursor,
) -> Result<String, ChangeStoreError> {
    if cursor.create_time_unix_ms < 0 {
        return Err(ChangeStoreError::Backend(
            "change-record cursor time must not precede the Unix epoch".to_string(),
        ));
    }
    let expected_prefix = format!("projects/{project}/repos/{repo}/changes/");
    let Some(change_id) = cursor.name.strip_prefix(&expected_prefix) else {
        return Err(ChangeStoreError::Backend(
            "change-record cursor is outside the requested repository".to_string(),
        ));
    };
    if change_id.is_empty() || change_id.contains('/') || change_id.chars().any(char::is_control) {
        return Err(ChangeStoreError::Backend(
            "change-record cursor contains an invalid resource name".to_string(),
        ));
    }
    Ok(format!(
        "{:019}/{}",
        cursor.create_time_unix_ms,
        hex::encode(cursor.name.as_bytes())
    ))
}

fn validate_record_key(record: &ChangeRecord, key: &str) -> Result<(), ChangeStoreError> {
    if record.name != key {
        return Err(ChangeStoreError::Backend(format!(
            "change record key/name mismatch: key={key:?}, name={:?}",
            record.name
        )));
    }
    change_record_index_key(record).map(|_| ())
}

fn validate_immutable_coordinates(
    current: &ChangeRecord,
    replacement: &ChangeRecord,
) -> Result<(), ChangeStoreError> {
    if current.name != replacement.name
        || current.project != replacement.project
        || current.repo != replacement.repo
        || current.created_by != replacement.created_by
        || current.create_time_unix_ms != replacement.create_time_unix_ms
    {
        return Err(ChangeStoreError::Backend(
            "change-record replacement modifies immutable coordinates".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use schemahub_jj::MemoryObjectDb;
    use schemahub_types::IdentityKind;

    use super::*;
    use crate::change_record::ChangeActor;

    fn record(id: &str, create_time_unix_ms: i64) -> ChangeRecord {
        ChangeRecord {
            name: format!("projects/acme/repos/commerce/changes/{id}"),
            project: "acme".to_string(),
            repo: "commerce".to_string(),
            target_bookmark: "main".to_string(),
            base_revision: None,
            title: format!("Change {id}"),
            description: String::new(),
            external_references: Vec::new(),
            edits: Vec::new(),
            created_by: ChangeActor {
                identity: "alice".to_string(),
                kind: IdentityKind::Human,
                display_name: Some("Alice".to_string()),
                delegated_by: None,
            },
            status: ChangeRecordStatus::Draft,
            validation: None,
            reviews: Vec::new(),
            apply_attempt: None,
            apply_result: None,
            etag: String::new(),
            create_time_unix_ms,
            update_time_unix_ms: create_time_unix_ms,
        }
    }

    #[test]
    fn object_db_pages_are_bounded_and_status_indexes_follow_transitions() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbChangeRecordStore::new(db);
        let first = store.create(record("first", 100)).unwrap();
        let second = store.create(record("second", 200)).unwrap();
        let first_page = store
            .list_page("acme", "commerce", Some(ChangeRecordStatus::Draft), None, 1)
            .unwrap();
        let mut ready = first.clone();
        ready.status = ChangeRecordStatus::Ready;

        // Act
        let ready = store.replace(&first.etag, ready).unwrap();
        let remaining_drafts = store
            .list_page(
                "acme",
                "commerce",
                Some(ChangeRecordStatus::Draft),
                first_page.next_cursor.as_ref(),
                1,
            )
            .unwrap();
        let ready_page = store
            .list_page("acme", "commerce", Some(ChangeRecordStatus::Ready), None, 1)
            .unwrap();
        let all_page = store.list_page("acme", "commerce", None, None, 2).unwrap();

        // Assert
        assert_eq!(first_page.records, [first]);
        assert!(first_page.next_cursor.is_some());
        assert_eq!(
            remaining_drafts.records.as_slice(),
            std::slice::from_ref(&second)
        );
        assert_eq!(remaining_drafts.next_cursor, None);
        assert_eq!(ready_page.records, [ready]);
        assert_eq!(all_page.records, [ready_page.records[0].clone(), second]);
    }

    #[test]
    fn legacy_records_are_indexed_before_the_first_page_is_served() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let legacy = record("legacy", 100);
        db.create_record(
            CHANGE_RECORD_COLLECTION,
            &legacy.name,
            &ObjectDbChangeRecordStore::encode(&legacy).unwrap(),
        )
        .unwrap();
        let store = ObjectDbChangeRecordStore::new(db.clone());

        // Act
        let page = store
            .list_page("acme", "commerce", Some(ChangeRecordStatus::Draft), None, 1)
            .unwrap();

        // Assert
        assert_eq!(page.records.as_slice(), std::slice::from_ref(&legacy));
        assert_eq!(
            db.get_record(
                CHANGE_RECORD_INDEX_MIGRATION_COLLECTION,
                CHANGE_RECORD_INDEX_MIGRATION_KEY,
            )
            .unwrap(),
            Some(CHANGE_RECORD_INDEX_MIGRATION_VALUE.to_vec())
        );
        let index_key = change_record_index_key(&legacy).unwrap();
        for collection in index_collections(&legacy) {
            assert_eq!(
                db.get_record(&collection, &index_key).unwrap(),
                Some(legacy.name.as_bytes().to_vec())
            );
        }
    }

    #[test]
    fn missing_index_target_fails_closed() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbChangeRecordStore::new(db.clone());
        let created = store.create(record("orphaned", 100)).unwrap();
        let bytes = ObjectDbChangeRecordStore::encode(&created).unwrap();
        assert!(db
            .compare_and_delete_record(CHANGE_RECORD_COLLECTION, &created.name, &bytes)
            .unwrap());

        // Act
        let result = store.list_page("acme", "commerce", None, None, 1);

        // Assert
        assert!(matches!(
            result,
            Err(ChangeStoreError::Backend(message)) if message.contains("points to missing record")
        ));
    }

    #[test]
    fn index_collision_rolls_back_change_record_creation() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbChangeRecordStore::new(db.clone());
        store.list_page("acme", "commerce", None, None, 1).unwrap();
        let candidate = record("collision", 100);
        let index_key = change_record_index_key(&candidate).unwrap();
        let all_collection = change_record_index_collection("acme", "commerce", None);
        db.create_record(&all_collection, &index_key, b"reserved")
            .unwrap();

        // Act
        let result = store.create(candidate.clone());

        // Assert
        assert!(matches!(result, Err(ChangeStoreError::Backend(_))));
        assert_eq!(
            db.get_record(CHANGE_RECORD_COLLECTION, &candidate.name)
                .unwrap(),
            None
        );
        let status_collection =
            change_record_index_collection("acme", "commerce", Some(ChangeRecordStatus::Draft));
        assert_eq!(db.get_record(&status_collection, &index_key).unwrap(), None);
    }

    #[test]
    fn status_index_collision_rolls_back_lifecycle_transition() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbChangeRecordStore::new(db.clone());
        let created = store.create(record("status-collision", 100)).unwrap();
        let index_key = change_record_index_key(&created).unwrap();
        let ready_collection =
            change_record_index_collection("acme", "commerce", Some(ChangeRecordStatus::Ready));
        db.create_record(&ready_collection, &index_key, b"reserved")
            .unwrap();
        let mut candidate = created.clone();
        candidate.status = ChangeRecordStatus::Ready;

        // Act
        let result = store.replace(&created.etag, candidate);

        // Assert
        assert!(matches!(result, Err(ChangeStoreError::Backend(_))));
        assert_eq!(store.get(&created.name).unwrap(), Some(created.clone()));
        let draft_collection =
            change_record_index_collection("acme", "commerce", Some(ChangeRecordStatus::Draft));
        assert_eq!(
            db.get_record(&draft_collection, &index_key).unwrap(),
            Some(created.name.as_bytes().to_vec())
        );
    }

    #[test]
    fn indexed_page_does_not_decode_unrelated_global_records() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbChangeRecordStore::new(db.clone());
        let created = store.create(record("indexed", 100)).unwrap();
        db.create_record(
            CHANGE_RECORD_COLLECTION,
            "projects/other/repos/other/changes/corrupt",
            b"not-json",
        )
        .unwrap();

        // Act
        let page = store.list_page("acme", "commerce", None, None, 1).unwrap();

        // Assert
        assert_eq!(page.records.as_slice(), std::slice::from_ref(&created));
    }

    #[test]
    fn page_cursor_cannot_cross_repository_scope() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbChangeRecordStore::new(db);
        let cursor = ChangeRecordPageCursor {
            create_time_unix_ms: 100,
            name: "projects/acme/repos/other/changes/change-a".to_string(),
        };

        // Act
        let result = store.list_page("acme", "commerce", None, Some(&cursor), 1);

        // Assert
        assert!(matches!(result, Err(ChangeStoreError::Backend(_))));
    }
}
