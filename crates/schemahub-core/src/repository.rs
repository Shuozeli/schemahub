//! Durable repository resources and their policy configuration.
//!
//! Repository metadata is mutable control-plane state, not a JJ object. The
//! production store therefore uses the same `ObjectDb` resource-record seam as
//! ChangeRecord while retaining independent optimistic concurrency and schema.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use schemahub_jj::{ObjectDb, ObjectDbError, RecordMutation};
use schemahub_types::Action;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{RepoConfig, ReviewPolicy, ServingPolicy};
use crate::control_plane_audit::{
    audit_collection, audit_index_collection, audit_index_key, make_event, ControlPlaneAuditAction,
    ControlPlaneAuditContext, ControlPlaneAuditSnapshot,
};
use crate::error::{CoreError, CoreResult};
use crate::Core;

const REPOSITORY_COLLECTION: &str = "schemahub.repositories.v1";
const REPOSITORY_INDEX_PREFIX: &str = "schemahub.repository_index.v1";
const REPOSITORY_INDEX_MIGRATION_COLLECTION: &str = "schemahub.repository_index_migration.v1";
const REPOSITORY_INDEX_MIGRATION_KEY: &str = "complete";
const REPOSITORY_INDEX_MIGRATION_VALUE: &[u8] = b"schemahub.repository_index.v1";
const MAX_INDEX_MIGRATION_RETRIES: usize = 8;
const MAX_REQUIRED_APPROVALS: u32 = 20;
const DEFAULT_INTERNAL_REPOSITORY_PAGE_SIZE: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryPage {
    pub repositories: Vec<Repository>,
    pub next_cursor: Option<String>,
}

/// A persisted repository control-plane resource.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// `projects/{project}/repos/{repo}`.
    pub resource_name: String,
    pub project: String,
    pub name: String,
    pub config: RepoConfig,
    pub created_by: String,
    pub etag: String,
    pub create_time_unix_ms: i64,
    pub update_time_unix_ms: i64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub archive_time_unix_ms: Option<i64>,
}

impl Repository {
    pub fn new(
        project: impl Into<String>,
        name: impl Into<String>,
        config: RepoConfig,
        created_by: impl Into<String>,
        now_unix_ms: i64,
    ) -> Self {
        let project = project.into();
        let name = name.into();
        Self {
            resource_name: format!("projects/{project}/repos/{name}"),
            project,
            name,
            config,
            created_by: created_by.into(),
            etag: String::new(),
            create_time_unix_ms: now_unix_ms,
            update_time_unix_ms: now_unix_ms,
            archived: false,
            archive_time_unix_ms: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateRepository {
    pub project: String,
    pub name: String,
    pub config: RepoConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryUpdate {
    pub default_bookmark: Option<String>,
    pub compatibility_direction: Option<schemahub_types::CompatibilityDirection>,
    pub protected_bookmarks: Option<Vec<String>>,
    pub review_policy: Option<ReviewPolicy>,
    pub serving_policy: Option<ServingPolicy>,
}

impl RepositoryUpdate {
    pub fn is_empty(&self) -> bool {
        self.default_bookmark.is_none()
            && self.compatibility_direction.is_none()
            && self.protected_bookmarks.is_none()
            && self.review_policy.is_none()
            && self.serving_policy.is_none()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RepositoryStoreError {
    #[error("repository already exists: {0}")]
    AlreadyExists(String),
    #[error("repository not found: {0}")]
    NotFound(String),
    #[error("repository etag mismatch for {name}: expected {expected}, current {current}")]
    EtagMismatch {
        name: String,
        expected: String,
        current: String,
    },
    #[error("repository store error: {0}")]
    Backend(String),
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("invalid repository: {0}")]
    InvalidArgument(String),
    #[error("repository precondition failed: {0}")]
    FailedPrecondition(String),
    #[error(transparent)]
    Store(#[from] RepositoryStoreError),
}

/// Transactional repository persistence boundary.
pub trait RepositoryStore: Send + Sync + 'static {
    fn create(&self, repository: Repository) -> Result<Repository, RepositoryStoreError>;
    fn create_audited(
        &self,
        repository: Repository,
        _audit: &ControlPlaneAuditContext,
    ) -> Result<Repository, RepositoryStoreError> {
        self.create(repository)
    }
    fn get(&self, project: &str, repo: &str) -> Result<Option<Repository>, RepositoryStoreError>;
    fn list(&self, project: &str) -> Result<Vec<Repository>, RepositoryStoreError>;
    fn list_page(
        &self,
        project: &str,
        include_archived: bool,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> Result<RepositoryPage, RepositoryStoreError> {
        if limit == 0 {
            return Ok(RepositoryPage {
                repositories: Vec::new(),
                next_cursor: None,
            });
        }
        let mut repositories = self.list(project)?;
        repositories.retain(|repository| {
            (include_archived || !repository.archived)
                && repository.name.starts_with(name_prefix)
                && start_after.is_none_or(|cursor| repository.name.as_str() > cursor)
        });
        repositories.sort_by(|left, right| left.name.cmp(&right.name));
        let has_more = repositories.len() > limit;
        repositories.truncate(limit);
        let next_cursor = has_more
            .then(|| {
                repositories
                    .last()
                    .map(|repository| repository.name.clone())
            })
            .flatten();
        Ok(RepositoryPage {
            repositories,
            next_cursor,
        })
    }
    fn replace(
        &self,
        expected_etag: &str,
        repository: Repository,
    ) -> Result<Repository, RepositoryStoreError>;
    fn replace_audited(
        &self,
        expected_etag: &str,
        repository: Repository,
        _audit: &ControlPlaneAuditContext,
    ) -> Result<Repository, RepositoryStoreError> {
        self.replace(expected_etag, repository)
    }
}

/// In-memory fake used by Core tests and constructors that do not supply a
/// production resource store.
#[derive(Debug, Default)]
pub struct MemoryRepositoryStore {
    records: Mutex<BTreeMap<String, Repository>>,
}

impl MemoryRepositoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, Repository>>, RepositoryStoreError> {
        self.records.lock().map_err(|error| {
            RepositoryStoreError::Backend(format!("poisoned memory store: {error}"))
        })
    }
}

impl RepositoryStore for MemoryRepositoryStore {
    fn create(&self, mut repository: Repository) -> Result<Repository, RepositoryStoreError> {
        let mut records = self.lock()?;
        if records.contains_key(&repository.resource_name) {
            return Err(RepositoryStoreError::AlreadyExists(
                repository.resource_name,
            ));
        }
        repository.etag = "v1".to_string();
        records.insert(repository.resource_name.clone(), repository.clone());
        Ok(repository)
    }

    fn get(&self, project: &str, repo: &str) -> Result<Option<Repository>, RepositoryStoreError> {
        Ok(self.lock()?.get(&resource_name(project, repo)).cloned())
    }

    fn list(&self, project: &str) -> Result<Vec<Repository>, RepositoryStoreError> {
        let mut records: Vec<_> = self
            .lock()?
            .values()
            .filter(|repository| repository.project == project)
            .cloned()
            .collect();
        records.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(records)
    }

    fn list_page(
        &self,
        project: &str,
        include_archived: bool,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> Result<RepositoryPage, RepositoryStoreError> {
        if limit == 0 {
            return Ok(RepositoryPage {
                repositories: Vec::new(),
                next_cursor: None,
            });
        }
        let records = self.lock()?;
        let mut repositories: Vec<_> = records
            .values()
            .filter(|repository| {
                repository.project == project
                    && (include_archived || !repository.archived)
                    && repository.name.starts_with(name_prefix)
                    && start_after.is_none_or(|cursor| repository.name.as_str() > cursor)
            })
            .cloned()
            .collect();
        repositories.sort_by(|left, right| left.name.cmp(&right.name));
        let has_more = repositories.len() > limit;
        repositories.truncate(limit);
        let next_cursor = has_more
            .then(|| {
                repositories
                    .last()
                    .map(|repository| repository.name.clone())
            })
            .flatten();
        Ok(RepositoryPage {
            repositories,
            next_cursor,
        })
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut repository: Repository,
    ) -> Result<Repository, RepositoryStoreError> {
        let mut records = self.lock()?;
        let current = records
            .get(&repository.resource_name)
            .ok_or_else(|| RepositoryStoreError::NotFound(repository.resource_name.clone()))?;
        require_etag(current, expected_etag)?;
        repository.etag = next_etag(&current.etag)?;
        records.insert(repository.resource_name.clone(), repository.clone());
        Ok(repository)
    }
}

/// Durable repository store over redb/PostgreSQL through `ObjectDb`.
#[derive(Debug)]
pub struct ObjectDbRepositoryStore {
    db: Arc<dyn ObjectDb>,
}

impl ObjectDbRepositoryStore {
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    fn encode(repository: &Repository) -> Result<Vec<u8>, RepositoryStoreError> {
        serde_json::to_vec(repository)
            .map_err(|error| RepositoryStoreError::Backend(format!("encode repository: {error}")))
    }

    fn decode(bytes: &[u8]) -> Result<Repository, RepositoryStoreError> {
        serde_json::from_slice(bytes)
            .map_err(|error| RepositoryStoreError::Backend(format!("decode repository: {error}")))
    }

    fn map_db(error: ObjectDbError) -> RepositoryStoreError {
        RepositoryStoreError::Backend(error.to_string())
    }

    fn ensure_indexes(&self) -> Result<(), RepositoryStoreError> {
        for _ in 0..MAX_INDEX_MIGRATION_RETRIES {
            match self
                .db
                .get_record(
                    REPOSITORY_INDEX_MIGRATION_COLLECTION,
                    REPOSITORY_INDEX_MIGRATION_KEY,
                )
                .map_err(Self::map_db)?
            {
                Some(value) if value == REPOSITORY_INDEX_MIGRATION_VALUE => return Ok(()),
                Some(_) => {
                    return Err(RepositoryStoreError::Backend(
                        "repository index migration marker is malformed".to_string(),
                    ));
                }
                None => {}
            }

            let mut missing = Vec::new();
            for (key, bytes) in self
                .db
                .list_records(REPOSITORY_COLLECTION)
                .map_err(Self::map_db)?
            {
                let repository = Self::decode(&bytes)?;
                validate_repository_record(&repository, &key)?;
                let index_key = repository_index_key(&repository.name)?;
                for collection in repository_index_collections(&repository) {
                    match self
                        .db
                        .get_record(&collection, &index_key)
                        .map_err(Self::map_db)?
                    {
                        Some(value) if value == repository.resource_name.as_bytes() => {}
                        Some(_) => {
                            return Err(RepositoryStoreError::Backend(format!(
                                "repository index {collection:?}/{index_key:?} \
                                 does not identify {:?}",
                                repository.resource_name
                            )));
                        }
                        None => missing.push((
                            collection,
                            index_key.clone(),
                            repository.resource_name.as_bytes().to_vec(),
                        )),
                    }
                }
            }
            missing.push((
                REPOSITORY_INDEX_MIGRATION_COLLECTION.to_string(),
                REPOSITORY_INDEX_MIGRATION_KEY.to_string(),
                REPOSITORY_INDEX_MIGRATION_VALUE.to_vec(),
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
        Err(RepositoryStoreError::Backend(
            "repository index migration did not converge".to_string(),
        ))
    }

    fn failed_replace(
        &self,
        expected_etag: &str,
        repository: Repository,
    ) -> Result<Repository, RepositoryStoreError> {
        let latest = self
            .db
            .get_record(REPOSITORY_COLLECTION, &repository.resource_name)
            .map_err(Self::map_db)?
            .ok_or_else(|| RepositoryStoreError::NotFound(repository.resource_name.clone()))
            .and_then(|bytes| Self::decode(&bytes))?;
        if latest.etag != expected_etag {
            return Err(RepositoryStoreError::EtagMismatch {
                name: repository.resource_name,
                expected: expected_etag.to_string(),
                current: latest.etag,
            });
        }
        Err(RepositoryStoreError::Backend(format!(
            "repository index precondition failed for {:?}",
            repository.resource_name
        )))
    }
}

impl RepositoryStore for ObjectDbRepositoryStore {
    fn create(&self, mut repository: Repository) -> Result<Repository, RepositoryStoreError> {
        self.ensure_indexes()?;
        repository.etag = "v1".to_string();
        validate_repository_record(&repository, &repository.resource_name)?;
        let bytes = Self::encode(&repository)?;
        let index_key = repository_index_key(&repository.name)?;
        let all_collection = repository_index_collection(&repository.project, true);
        let active_collection = repository_index_collection(&repository.project, false);
        let mut mutations = vec![
            RecordMutation::Create {
                collection: REPOSITORY_COLLECTION,
                key: &repository.resource_name,
                value: &bytes,
            },
            RecordMutation::Create {
                collection: &all_collection,
                key: &index_key,
                value: repository.resource_name.as_bytes(),
            },
        ];
        if !repository.archived {
            mutations.push(RecordMutation::Create {
                collection: &active_collection,
                key: &index_key,
                value: repository.resource_name.as_bytes(),
            });
        }
        if self.db.transact_records(&mutations).map_err(Self::map_db)? {
            return Ok(repository);
        }
        if self
            .db
            .get_record(REPOSITORY_COLLECTION, &repository.resource_name)
            .map_err(Self::map_db)?
            .is_some()
        {
            return Err(RepositoryStoreError::AlreadyExists(
                repository.resource_name,
            ));
        }
        Err(RepositoryStoreError::Backend(format!(
            "repository index collision while creating {:?}",
            repository.resource_name
        )))
    }

    fn create_audited(
        &self,
        mut repository: Repository,
        audit: &ControlPlaneAuditContext,
    ) -> Result<Repository, RepositoryStoreError> {
        self.ensure_indexes()?;
        repository.etag = "v1".to_string();
        validate_repository_record(&repository, &repository.resource_name)?;
        let bytes = Self::encode(&repository)?;
        let repository_index_key = repository_index_key(&repository.name)?;
        let repository_all_collection = repository_index_collection(&repository.project, true);
        let repository_active_collection = repository_index_collection(&repository.project, false);
        let (event, event_bytes) = make_event(
            audit,
            &repository.project,
            &repository.resource_name,
            ControlPlaneAuditAction::RepositoryCreated,
            None,
            Some(ControlPlaneAuditSnapshot::Repository(repository.clone())),
        )
        .map_err(|error| RepositoryStoreError::Backend(error.to_string()))?;
        let audit_collection = audit_collection(&repository.project);
        let audit_index_collection = audit_index_collection(&repository.project);
        let audit_index_key = audit_index_key(&event)
            .map_err(|error| RepositoryStoreError::Backend(error.to_string()))?;
        let mut mutations = vec![
            RecordMutation::Create {
                collection: REPOSITORY_COLLECTION,
                key: &repository.resource_name,
                value: &bytes,
            },
            RecordMutation::Create {
                collection: &repository_all_collection,
                key: &repository_index_key,
                value: repository.resource_name.as_bytes(),
            },
            RecordMutation::Create {
                collection: &audit_collection,
                key: &event.name,
                value: &event_bytes,
            },
            RecordMutation::Create {
                collection: &audit_index_collection,
                key: &audit_index_key,
                value: event.name.as_bytes(),
            },
        ];
        if !repository.archived {
            mutations.push(RecordMutation::Create {
                collection: &repository_active_collection,
                key: &repository_index_key,
                value: repository.resource_name.as_bytes(),
            });
        }
        if !self.db.transact_records(&mutations).map_err(Self::map_db)? {
            return Err(RepositoryStoreError::AlreadyExists(
                repository.resource_name,
            ));
        }
        Ok(repository)
    }

    fn get(&self, project: &str, repo: &str) -> Result<Option<Repository>, RepositoryStoreError> {
        self.db
            .get_record(REPOSITORY_COLLECTION, &resource_name(project, repo))
            .map_err(Self::map_db)?
            .map(|bytes| Self::decode(&bytes))
            .transpose()
    }

    fn list(&self, project: &str) -> Result<Vec<Repository>, RepositoryStoreError> {
        let mut found = Vec::new();
        for (key, bytes) in self
            .db
            .list_records(REPOSITORY_COLLECTION)
            .map_err(Self::map_db)?
        {
            let repository = Self::decode(&bytes)?;
            if repository.resource_name != key {
                return Err(RepositoryStoreError::Backend(format!(
                    "repository key/name mismatch: key={key:?}, name={:?}",
                    repository.resource_name
                )));
            }
            if repository.project == project {
                found.push(repository);
            }
        }
        found.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(found)
    }

    fn list_page(
        &self,
        project: &str,
        include_archived: bool,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> Result<RepositoryPage, RepositoryStoreError> {
        validate_repository_filter(project, name_prefix, start_after)?;
        if limit == 0 {
            return Ok(RepositoryPage {
                repositories: Vec::new(),
                next_cursor: None,
            });
        }
        self.ensure_indexes()?;
        let collection = repository_index_collection(project, include_archived);
        let start_after = repository_page_start(name_prefix, start_after)?;
        let fetch_limit = limit.checked_add(1).ok_or_else(|| {
            RepositoryStoreError::Backend("repository page limit overflow".to_string())
        })?;
        let rows = self
            .db
            .list_records_page(&collection, start_after.as_deref(), fetch_limit)
            .map_err(Self::map_db)?;
        let encoded_prefix = hex::encode(name_prefix.as_bytes());
        let mut repositories = Vec::with_capacity(rows.len().min(limit));
        for (index_key, resource_name_bytes) in rows {
            if !index_key.starts_with(&encoded_prefix) {
                break;
            }
            let resource_name = std::str::from_utf8(&resource_name_bytes).map_err(|error| {
                RepositoryStoreError::Backend(format!(
                    "repository index {collection:?}/{index_key:?} \
                         contains an invalid resource name: {error}"
                ))
            })?;
            let bytes = self
                .db
                .get_record(REPOSITORY_COLLECTION, resource_name)
                .map_err(Self::map_db)?
                .ok_or_else(|| {
                    RepositoryStoreError::Backend(format!(
                        "repository index {collection:?}/{index_key:?} \
                         points to missing repository {resource_name:?}"
                    ))
                })?;
            let repository = Self::decode(&bytes)?;
            validate_repository_record(&repository, resource_name)?;
            if repository.project != project {
                return Err(RepositoryStoreError::Backend(format!(
                    "repository index scope mismatch for {:?}",
                    repository.resource_name
                )));
            }
            if !include_archived && repository.archived {
                return Err(RepositoryStoreError::Backend(format!(
                    "active repository index contains archived repository {:?}",
                    repository.resource_name
                )));
            }
            if !repository.name.starts_with(name_prefix) {
                return Err(RepositoryStoreError::Backend(format!(
                    "repository index prefix mismatch for {:?}",
                    repository.resource_name
                )));
            }
            let expected_index_key = repository_index_key(&repository.name)?;
            if index_key != expected_index_key {
                return Err(RepositoryStoreError::Backend(format!(
                    "repository index key mismatch: key={index_key:?}, \
                     expected={expected_index_key:?}"
                )));
            }
            repositories.push(repository);
        }
        let has_more = repositories.len() > limit;
        repositories.truncate(limit);
        let next_cursor = has_more
            .then(|| {
                repositories
                    .last()
                    .map(|repository| repository.name.clone())
            })
            .flatten();
        Ok(RepositoryPage {
            repositories,
            next_cursor,
        })
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut repository: Repository,
    ) -> Result<Repository, RepositoryStoreError> {
        self.ensure_indexes()?;
        let current_bytes = self
            .db
            .get_record(REPOSITORY_COLLECTION, &repository.resource_name)
            .map_err(Self::map_db)?
            .ok_or_else(|| RepositoryStoreError::NotFound(repository.resource_name.clone()))?;
        let current = Self::decode(&current_bytes)?;
        validate_repository_record(&current, &repository.resource_name)?;
        require_etag(&current, expected_etag)?;
        validate_repository_replacement(&current, &repository)?;
        repository.etag = next_etag(&current.etag)?;
        let replacement = Self::encode(&repository)?;
        let committed = if current.archived == repository.archived {
            self.db
                .compare_and_swap_record(
                    REPOSITORY_COLLECTION,
                    &repository.resource_name,
                    &current_bytes,
                    &replacement,
                )
                .map_err(Self::map_db)?
        } else {
            let index_key = repository_index_key(&repository.name)?;
            let active_collection = repository_index_collection(&repository.project, false);
            let index_mutation = if repository.archived {
                RecordMutation::CompareAndDelete {
                    collection: &active_collection,
                    key: &index_key,
                    expected: repository.resource_name.as_bytes(),
                }
            } else {
                RecordMutation::Create {
                    collection: &active_collection,
                    key: &index_key,
                    value: repository.resource_name.as_bytes(),
                }
            };
            self.db
                .transact_records(&[
                    RecordMutation::CompareAndSwap {
                        collection: REPOSITORY_COLLECTION,
                        key: &repository.resource_name,
                        expected: &current_bytes,
                        replacement: &replacement,
                    },
                    index_mutation,
                ])
                .map_err(Self::map_db)?
        };
        if committed {
            return Ok(repository);
        }
        self.failed_replace(expected_etag, repository)
    }

    fn replace_audited(
        &self,
        expected_etag: &str,
        mut repository: Repository,
        audit: &ControlPlaneAuditContext,
    ) -> Result<Repository, RepositoryStoreError> {
        self.ensure_indexes()?;
        let current_bytes = self
            .db
            .get_record(REPOSITORY_COLLECTION, &repository.resource_name)
            .map_err(Self::map_db)?
            .ok_or_else(|| RepositoryStoreError::NotFound(repository.resource_name.clone()))?;
        let current = Self::decode(&current_bytes)?;
        validate_repository_record(&current, &repository.resource_name)?;
        require_etag(&current, expected_etag)?;
        validate_repository_replacement(&current, &repository)?;
        repository.etag = next_etag(&current.etag)?;
        let replacement = Self::encode(&repository)?;
        let action = if !current.archived && repository.archived {
            ControlPlaneAuditAction::RepositoryArchived
        } else {
            ControlPlaneAuditAction::RepositoryUpdated
        };
        let (event, event_bytes) = make_event(
            audit,
            &repository.project,
            &repository.resource_name,
            action,
            Some(ControlPlaneAuditSnapshot::Repository(current.clone())),
            Some(ControlPlaneAuditSnapshot::Repository(repository.clone())),
        )
        .map_err(|error| RepositoryStoreError::Backend(error.to_string()))?;
        let audit_collection = audit_collection(&repository.project);
        let audit_index_collection = audit_index_collection(&repository.project);
        let audit_index_key = audit_index_key(&event)
            .map_err(|error| RepositoryStoreError::Backend(error.to_string()))?;
        let active_index_key = (current.archived != repository.archived)
            .then(|| repository_index_key(&repository.name))
            .transpose()?;
        let active_collection = (current.archived != repository.archived)
            .then(|| repository_index_collection(&repository.project, false));
        let mut mutations = vec![
            RecordMutation::CompareAndSwap {
                collection: REPOSITORY_COLLECTION,
                key: &repository.resource_name,
                expected: &current_bytes,
                replacement: &replacement,
            },
            RecordMutation::Create {
                collection: &audit_collection,
                key: &event.name,
                value: &event_bytes,
            },
            RecordMutation::Create {
                collection: &audit_index_collection,
                key: &audit_index_key,
                value: event.name.as_bytes(),
            },
        ];
        if let (Some(index_key), Some(active_collection)) =
            (active_index_key.as_ref(), active_collection.as_ref())
        {
            mutations.push(if repository.archived {
                RecordMutation::CompareAndDelete {
                    collection: active_collection,
                    key: index_key,
                    expected: repository.resource_name.as_bytes(),
                }
            } else {
                RecordMutation::Create {
                    collection: active_collection,
                    key: index_key,
                    value: repository.resource_name.as_bytes(),
                }
            });
        }
        if self.db.transact_records(&mutations).map_err(Self::map_db)? {
            return Ok(repository);
        }
        let latest = self
            .db
            .get_record(REPOSITORY_COLLECTION, &repository.resource_name)
            .map_err(Self::map_db)?
            .ok_or_else(|| RepositoryStoreError::NotFound(repository.resource_name.clone()))
            .and_then(|bytes| Self::decode(&bytes))?;
        if latest.etag == expected_etag {
            return Err(RepositoryStoreError::Backend(format!(
                "repository audit/index precondition failed for event '{}'",
                event.event_id,
            )));
        }
        Err(RepositoryStoreError::EtagMismatch {
            name: repository.resource_name,
            expected: expected_etag.to_string(),
            current: latest.etag,
        })
    }
}

impl Core {
    pub fn create_repository(
        &self,
        input: CreateRepository,
        token: Option<&str>,
    ) -> CoreResult<Repository> {
        validate_segment("project", &input.project)?;
        validate_segment("repo", &input.name)?;
        validate_config(&input.config)?;
        let _guard = self.acquire_control_plane_guard(&input.project)?;
        self.ensure_project_exists(&input.project)?;
        let identity =
            self.authorize_repo_action(token, Action::ManageRepo, &input.project, &input.name)?;
        let record = Repository::new(
            input.project,
            input.name,
            input.config,
            identity.id().unwrap_or("anonymous"),
            now_unix_millis()?,
        );
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        Ok(self.repository_store.create_audited(record, &audit)?)
    }

    pub fn get_repository(
        &self,
        project: &str,
        repo: &str,
        include_archived: bool,
        token: Option<&str>,
    ) -> CoreResult<Option<Repository>> {
        validate_segment("project", project)?;
        validate_segment("repo", repo)?;
        self.authorize_repo_action(token, Action::Read, project, repo)?;
        Ok(self
            .repository_store
            .get(project, repo)?
            .filter(|repository| include_archived || !repository.archived))
    }

    pub fn list_repositories(
        &self,
        project: &str,
        include_archived: bool,
        token: Option<&str>,
    ) -> CoreResult<Vec<Repository>> {
        validate_segment("project", project)?;
        self.ensure_project_exists(project)?;
        self.authorize_repo_action(token, Action::Read, project, "")?;
        let mut repositories = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.repository_store.list_page(
                project,
                include_archived,
                "",
                cursor.as_deref(),
                DEFAULT_INTERNAL_REPOSITORY_PAGE_SIZE,
            )?;
            repositories.extend(page.repositories);
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(repositories)
    }

    pub fn list_repositories_page(
        &self,
        project: &str,
        include_archived: bool,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> CoreResult<RepositoryPage> {
        validate_segment("project", project)?;
        validate_name_prefix(name_prefix)?;
        if let Some(cursor) = start_after {
            validate_segment("repository page cursor", cursor)?;
            if !cursor.starts_with(name_prefix) {
                return Err(RepositoryError::InvalidArgument(
                    "repository page cursor is outside the requested name prefix".to_string(),
                )
                .into());
            }
        }
        self.ensure_project_exists(project)?;
        self.authorize_repo_action(token, Action::Read, project, "")?;
        Ok(self.repository_store.list_page(
            project,
            include_archived,
            name_prefix,
            start_after,
            limit,
        )?)
    }

    pub fn update_repository(
        &self,
        project: &str,
        repo: &str,
        expected_etag: &str,
        patch: RepositoryUpdate,
        token: Option<&str>,
    ) -> CoreResult<Repository> {
        validate_segment("project", project)?;
        validate_segment("repo", repo)?;
        validate_expected_etag(expected_etag)?;
        if patch.is_empty() {
            return Err(RepositoryError::InvalidArgument(
                "update mask selects no repository fields".to_string(),
            )
            .into());
        }
        let _guard = self.acquire_control_plane_guard(project)?;
        let identity = self.authorize_repo_action(token, Action::ManageRepo, project, repo)?;
        let mut repository = self
            .repository_store
            .get(project, repo)?
            .ok_or_else(|| RepositoryStoreError::NotFound(resource_name(project, repo)))?;
        if repository.archived {
            return Err(RepositoryError::FailedPrecondition(
                "an archived repository cannot be updated".to_string(),
            )
            .into());
        }
        if let Some(value) = patch.default_bookmark {
            repository.config.default_bookmark = value;
        }
        if let Some(value) = patch.compatibility_direction {
            repository.config.compatibility_direction = value;
        }
        if let Some(value) = patch.protected_bookmarks {
            repository.config.protected_bookmarks = value;
        }
        if let Some(value) = patch.review_policy {
            repository.config.review_policy = value;
        }
        if let Some(value) = patch.serving_policy {
            repository.config.serving_policy = value;
        }
        validate_config(&repository.config)?;
        repository.update_time_unix_ms = now_unix_millis()?;
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        Ok(self
            .repository_store
            .replace_audited(expected_etag, repository, &audit)?)
    }

    /// Archive a repository registry entry. JJ history remains retained so an
    /// audit or explicit future recovery operation cannot observe data loss.
    pub fn archive_repository(
        &self,
        project: &str,
        repo: &str,
        expected_etag: &str,
        force: bool,
        token: Option<&str>,
    ) -> CoreResult<Repository> {
        validate_segment("project", project)?;
        validate_segment("repo", repo)?;
        validate_expected_etag(expected_etag)?;
        let _guard = self.acquire_control_plane_guard(project)?;
        let identity = self.authorize_repo_action(token, Action::ManageRepo, project, repo)?;
        let mut repository = self
            .repository_store
            .get(project, repo)?
            .ok_or_else(|| RepositoryStoreError::NotFound(resource_name(project, repo)))?;
        if repository.archived {
            if repository.etag != expected_etag {
                return Err(RepositoryStoreError::EtagMismatch {
                    name: repository.resource_name,
                    expected: expected_etag.to_string(),
                    current: repository.etag,
                }
                .into());
            }
            return Ok(repository);
        }
        if !force && self.repository_has_refs(project, repo)? {
            return Err(RepositoryError::FailedPrecondition(
                "repository has schema refs; set force=true to archive while retaining history"
                    .to_string(),
            )
            .into());
        }
        let now = now_unix_millis()?;
        repository.archived = true;
        repository.archive_time_unix_ms = Some(now);
        repository.update_time_unix_ms = now;
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        Ok(self
            .repository_store
            .replace_audited(expected_etag, repository, &audit)?)
    }

    /// Runtime policy lookup. A durable repository record wins; legacy startup
    /// TOML remains a compatibility fallback for schema repos that predate the
    /// registry. Archived registered repos fail closed.
    pub(crate) fn effective_repo_config(
        &self,
        project: &str,
        repo: &str,
    ) -> CoreResult<RepoConfig> {
        match self.repository_store.get(project, repo)? {
            Some(repository) if repository.archived => Err(RepositoryError::FailedPrecondition(
                format!("repository {project}/{repo} is archived"),
            )
            .into()),
            Some(repository) => Ok(repository.config),
            None => Ok(self.repo_configs.get(project, repo)),
        }
    }

    /// Return the configured default bookmark after authorizing the requested
    /// repository action. Transport adapters use this only when a VersionRef is
    /// absent; hard-coding `main` would bypass repository configuration.
    pub fn repository_default_bookmark(
        &self,
        project: &str,
        repo: &str,
        action: Action,
        token: Option<&str>,
    ) -> CoreResult<String> {
        validate_segment("project", project)?;
        validate_segment("repo", repo)?;
        self.authorize_repo_action(token, action, project, repo)?;
        Ok(self.effective_repo_config(project, repo)?.default_bookmark)
    }

    pub fn ensure_direct_write_allowed(&self, project: &str, repo: &str) -> CoreResult<()> {
        if self
            .effective_repo_config(project, repo)?
            .review_policy
            .require_change_record
        {
            return Err(RepositoryError::FailedPrecondition(
                "repository policy requires publication through a ChangeRecord".to_string(),
            )
            .into());
        }
        Ok(())
    }

    fn repository_has_refs(&self, project: &str, repo: &str) -> CoreResult<bool> {
        if !self
            .jj
            .list_bookmarks_page(project, repo, "", None, 1)?
            .refs
            .is_empty()
        {
            return Ok(true);
        }
        Ok(!self
            .jj
            .list_tags_page(project, repo, "", None, 1)?
            .refs
            .is_empty())
    }
}

fn validate_segment(label: &str, value: &str) -> Result<(), RepositoryError> {
    if value.is_empty()
        || value.contains('/')
        || value.chars().any(char::is_control)
        || value.len() > 128
    {
        return Err(RepositoryError::InvalidArgument(format!(
            "{label} must be a 1-128 character resource path segment without control characters"
        )));
    }
    Ok(())
}

fn validate_name_prefix(value: &str) -> Result<(), RepositoryError> {
    if value.contains('/') || value.chars().any(char::is_control) || value.len() > 128 {
        return Err(RepositoryError::InvalidArgument(
            "repository name prefix must be at most 128 characters without '/' or control characters"
                .to_string(),
        ));
    }
    Ok(())
}

fn repository_index_collection(project: &str, include_archived: bool) -> String {
    format!(
        "{REPOSITORY_INDEX_PREFIX}/projects/{}/{}",
        hex::encode(project.as_bytes()),
        if include_archived { "all" } else { "active" }
    )
}

fn repository_index_collections(repository: &Repository) -> Vec<String> {
    let mut collections = vec![repository_index_collection(&repository.project, true)];
    if !repository.archived {
        collections.push(repository_index_collection(&repository.project, false));
    }
    collections
}

fn repository_index_key(name: &str) -> Result<String, RepositoryStoreError> {
    validate_catalog_segment("repository", name)?;
    Ok(format!("{}/", hex::encode(name.as_bytes())))
}

fn repository_page_start(
    name_prefix: &str,
    start_after: Option<&str>,
) -> Result<Option<String>, RepositoryStoreError> {
    if let Some(cursor) = start_after {
        return repository_index_key(cursor).map(Some);
    }
    Ok((!name_prefix.is_empty()).then(|| hex::encode(name_prefix.as_bytes())))
}

fn validate_repository_filter(
    project: &str,
    name_prefix: &str,
    start_after: Option<&str>,
) -> Result<(), RepositoryStoreError> {
    validate_catalog_segment("project", project)?;
    if name_prefix.contains('/')
        || name_prefix.chars().any(char::is_control)
        || name_prefix.len() > 128
    {
        return Err(RepositoryStoreError::Backend(
            "repository name prefix must be at most 128 characters without '/' or control characters"
                .to_string(),
        ));
    }
    if let Some(cursor) = start_after {
        validate_catalog_segment("repository cursor", cursor)?;
        if !cursor.starts_with(name_prefix) {
            return Err(RepositoryStoreError::Backend(
                "repository cursor is outside the requested name prefix".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_repository_record(
    repository: &Repository,
    key: &str,
) -> Result<(), RepositoryStoreError> {
    validate_catalog_segment("project", &repository.project)?;
    repository_index_key(&repository.name)?;
    let expected_name = resource_name(&repository.project, &repository.name);
    if repository.resource_name != key || repository.resource_name != expected_name {
        return Err(RepositoryStoreError::Backend(format!(
            "repository key/name mismatch: key={key:?}, name={:?}, expected={expected_name:?}",
            repository.resource_name
        )));
    }
    Ok(())
}

fn validate_repository_replacement(
    current: &Repository,
    replacement: &Repository,
) -> Result<(), RepositoryStoreError> {
    if current.resource_name != replacement.resource_name
        || current.project != replacement.project
        || current.name != replacement.name
        || current.created_by != replacement.created_by
        || current.create_time_unix_ms != replacement.create_time_unix_ms
    {
        return Err(RepositoryStoreError::Backend(
            "repository replacement modifies immutable coordinates".to_string(),
        ));
    }
    Ok(())
}

fn validate_catalog_segment(label: &str, value: &str) -> Result<(), RepositoryStoreError> {
    if value.trim().is_empty()
        || value.contains('/')
        || value.chars().any(char::is_control)
        || value.len() > 128
    {
        return Err(RepositoryStoreError::Backend(format!(
            "{label} must be a 1-128 character resource path segment without control characters"
        )));
    }
    Ok(())
}

pub fn validate_config(config: &RepoConfig) -> Result<(), RepositoryError> {
    validate_ref_name("default_branch", &config.default_bookmark)?;
    if config.protected_bookmarks.len() > 100 {
        return Err(RepositoryError::InvalidArgument(
            "protected_branches must contain at most 100 entries".to_string(),
        ));
    }
    let mut seen = BTreeSet::new();
    for pattern in &config.protected_bookmarks {
        validate_ref_name("protected branch pattern", pattern)?;
        if pattern.contains('*') && !pattern.ends_with("/*") {
            return Err(RepositoryError::InvalidArgument(format!(
                "protected branch pattern {pattern:?} may use '*' only as a trailing '/*'"
            )));
        }
        if !seen.insert(pattern) {
            return Err(RepositoryError::InvalidArgument(format!(
                "duplicate protected branch pattern {pattern:?}"
            )));
        }
    }
    if config.review_policy.required_approvals > MAX_REQUIRED_APPROVALS {
        return Err(RepositoryError::InvalidArgument(format!(
            "required_approvals must be at most {MAX_REQUIRED_APPROVALS}"
        )));
    }
    Ok(())
}

fn validate_ref_name(label: &str, value: &str) -> Result<(), RepositoryError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) || value.len() > 255 {
        return Err(RepositoryError::InvalidArgument(format!(
            "{label} must be a non-empty value of at most 255 characters without control characters"
        )));
    }
    Ok(())
}

fn validate_expected_etag(value: &str) -> Result<(), RepositoryError> {
    if value.is_empty() {
        return Err(RepositoryError::InvalidArgument(
            "etag must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn resource_name(project: &str, repo: &str) -> String {
    format!("projects/{project}/repos/{repo}")
}

fn require_etag(current: &Repository, expected_etag: &str) -> Result<(), RepositoryStoreError> {
    if current.etag != expected_etag {
        return Err(RepositoryStoreError::EtagMismatch {
            name: current.resource_name.clone(),
            expected: expected_etag.to_string(),
            current: current.etag.clone(),
        });
    }
    Ok(())
}

fn next_etag(current: &str) -> Result<String, RepositoryStoreError> {
    let version = current
        .strip_prefix('v')
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            RepositoryStoreError::Backend(format!("invalid stored repository etag: {current}"))
        })?;
    Ok(format!("v{}", version + 1))
}

pub fn now_unix_millis() -> Result<i64, RepositoryError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RepositoryError::FailedPrecondition(error.to_string()))?;
    i64::try_from(duration.as_millis()).map_err(|_| {
        RepositoryError::FailedPrecondition("system timestamp exceeds i64 milliseconds".to_string())
    })
}

impl From<RepositoryStoreError> for CoreError {
    fn from(error: RepositoryStoreError) -> Self {
        RepositoryError::from(error).into()
    }
}

#[cfg(test)]
mod tests {
    use schemahub_jj::{MemoryObjectDb, RedbObjectDb};

    use super::*;
    use crate::ObjectDbControlPlaneAuditLog;

    fn repository(name: &str) -> Repository {
        Repository::new("acme", name, RepoConfig::default(), "alice", 1_000)
    }

    fn audit_context() -> ControlPlaneAuditContext {
        ControlPlaneAuditContext {
            event_id: "audit-repository".to_string(),
            actor_id: "alice".to_string(),
            event_time_unix_ms: 2_000,
        }
    }

    #[test]
    fn memory_store_compare_and_swap_rejects_stale_etag() {
        // Arrange
        let store = MemoryRepositoryStore::new();
        let created = store.create(repository("commerce")).expect("create");
        let mut replacement = created.clone();
        replacement.config.default_bookmark = "stable".to_string();
        let updated = store
            .replace(&created.etag, replacement.clone())
            .expect("first update");

        // Act
        let stale = store.replace(&created.etag, replacement);

        // Assert
        assert!(matches!(
            stale,
            Err(RepositoryStoreError::EtagMismatch { current, .. }) if current == updated.etag
        ));
    }

    #[test]
    fn audited_repository_create_commits_resource_and_typed_event() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbRepositoryStore::new(db.clone());
        let audit = ObjectDbControlPlaneAuditLog::new(db);

        // Act
        let created = store
            .create_audited(repository("commerce"), &audit_context())
            .expect("create repository with audit");

        // Assert
        assert_eq!(
            store.get("acme", "commerce").unwrap(),
            Some(created.clone())
        );
        let events = audit.list("acme").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, ControlPlaneAuditAction::RepositoryCreated);
        assert_eq!(
            events[0].after,
            Some(ControlPlaneAuditSnapshot::Repository(created))
        );
    }

    #[test]
    fn repository_pages_are_prefix_bounded_and_active_index_follows_archive() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbRepositoryStore::new(db);
        for name in ["app", "apple", "beta"] {
            store.create(repository(name)).unwrap();
        }
        let first = store.list_page("acme", false, "app", None, 1).unwrap();
        let mut archived = first.repositories[0].clone();
        let expected_etag = archived.etag.clone();
        archived.archived = true;
        archived.archive_time_unix_ms = Some(2_000);
        archived.update_time_unix_ms = 2_000;

        // Act
        let archived = store.replace(&expected_etag, archived).unwrap();
        let second = store
            .list_page("acme", false, "app", first.next_cursor.as_deref(), 1)
            .unwrap();
        let active = store.list_page("acme", false, "app", None, 2).unwrap();
        let all = store.list_page("acme", true, "app", None, 2).unwrap();

        // Assert
        assert_eq!(first.repositories[0].name, "app");
        assert_eq!(first.next_cursor.as_deref(), Some("app"));
        assert_eq!(second.repositories[0].name, "apple");
        assert_eq!(active.repositories[0].name, "apple");
        assert_eq!(
            all.repositories
                .iter()
                .map(|repository| repository.name.as_str())
                .collect::<Vec<_>>(),
            ["app", "apple"]
        );
        assert_eq!(all.repositories[0], archived);
    }

    #[test]
    fn legacy_repositories_are_indexed_before_the_first_page_is_served() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let mut legacy = repository("legacy");
        legacy.etag = "v1".to_string();
        db.create_record(
            REPOSITORY_COLLECTION,
            &legacy.resource_name,
            &ObjectDbRepositoryStore::encode(&legacy).unwrap(),
        )
        .unwrap();
        let store = ObjectDbRepositoryStore::new(db.clone());

        // Act
        let page = store.list_page("acme", false, "", None, 1).unwrap();

        // Assert
        assert_eq!(page.repositories, [legacy.clone()]);
        assert_eq!(
            db.get_record(
                REPOSITORY_INDEX_MIGRATION_COLLECTION,
                REPOSITORY_INDEX_MIGRATION_KEY,
            )
            .unwrap(),
            Some(REPOSITORY_INDEX_MIGRATION_VALUE.to_vec())
        );
        let index_key = repository_index_key(&legacy.name).unwrap();
        for collection in repository_index_collections(&legacy) {
            assert_eq!(
                db.get_record(&collection, &index_key).unwrap(),
                Some(legacy.resource_name.as_bytes().to_vec())
            );
        }
    }

    #[test]
    fn missing_repository_index_target_fails_closed() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbRepositoryStore::new(db.clone());
        let created = store.create(repository("orphaned")).unwrap();
        let bytes = ObjectDbRepositoryStore::encode(&created).unwrap();
        assert!(db
            .compare_and_delete_record(REPOSITORY_COLLECTION, &created.resource_name, &bytes)
            .unwrap());

        // Act
        let result = store.list_page("acme", false, "", None, 1);

        // Assert
        assert!(matches!(
            result,
            Err(RepositoryStoreError::Backend(message))
                if message.contains("points to missing repository")
        ));
    }

    #[test]
    fn repository_index_collision_rolls_back_resource_creation() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbRepositoryStore::new(db.clone());
        store.list_page("acme", false, "", None, 1).unwrap();
        let candidate = repository("collision");
        let index_key = repository_index_key(&candidate.name).unwrap();
        let active_collection = repository_index_collection("acme", false);
        db.create_record(&active_collection, &index_key, b"reserved")
            .unwrap();

        // Act
        let result = store.create(candidate.clone());

        // Assert
        assert!(matches!(result, Err(RepositoryStoreError::Backend(_))));
        assert_eq!(store.get("acme", "collision").unwrap(), None);
        assert_eq!(
            db.get_record(&repository_index_collection("acme", true), &index_key,)
                .unwrap(),
            None
        );
    }

    #[test]
    fn active_index_collision_rolls_back_repository_archive() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbRepositoryStore::new(db.clone());
        let created = store.create(repository("archive-collision")).unwrap();
        let index_key = repository_index_key(&created.name).unwrap();
        let active_collection = repository_index_collection("acme", false);
        assert!(db
            .compare_and_delete_record(
                &active_collection,
                &index_key,
                created.resource_name.as_bytes(),
            )
            .unwrap());
        db.create_record(&active_collection, &index_key, b"reserved")
            .unwrap();
        let mut candidate = created.clone();
        candidate.archived = true;
        candidate.archive_time_unix_ms = Some(2_000);
        candidate.update_time_unix_ms = 2_000;

        // Act
        let result = store.replace(&created.etag, candidate);

        // Assert
        assert!(matches!(result, Err(RepositoryStoreError::Backend(_))));
        assert_eq!(
            store.get("acme", "archive-collision").unwrap(),
            Some(created)
        );
    }

    #[test]
    fn indexed_repository_page_ignores_an_unrelated_corrupt_primary_record() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let store = ObjectDbRepositoryStore::new(db.clone());
        let created = store.create(repository("indexed")).unwrap();
        db.create_record(
            REPOSITORY_COLLECTION,
            "projects/other/repos/corrupt",
            b"not-json",
        )
        .unwrap();

        // Act
        let page = store.list_page("acme", false, "", None, 1).unwrap();

        // Assert
        assert_eq!(page.repositories, [created]);
    }

    #[test]
    fn object_db_repository_survives_redb_restart() {
        // Arrange
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("schemahub.redb");
        let expected = {
            let db: Arc<dyn ObjectDb> =
                Arc::new(RedbObjectDb::open(&path).expect("open redb writer"));
            ObjectDbRepositoryStore::new(db)
                .create(repository("commerce"))
                .expect("persist repository")
        };
        let db: Arc<dyn ObjectDb> =
            Arc::new(RedbObjectDb::open(&path).expect("reopen redb reader"));
        let store = ObjectDbRepositoryStore::new(db);

        // Act
        let restored = store
            .list_page("acme", false, "", None, 1)
            .expect("list repository after reopen")
            .repositories
            .into_iter()
            .next()
            .expect("repository exists");

        // Assert
        assert_eq!(restored, expected);
    }
}
