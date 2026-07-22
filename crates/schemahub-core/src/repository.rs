//! Durable repository resources and their policy configuration.
//!
//! Repository metadata is mutable control-plane state, not a JJ object. The
//! production store therefore uses the same `ObjectDb` resource-record seam as
//! ChangeRecord while retaining independent optimistic concurrency and schema.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use schemahub_jj::{ObjectDb, ObjectDbError};
use schemahub_types::Action;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{RepoConfig, ReviewPolicy, ServingPolicy};
use crate::error::{CoreError, CoreResult};
use crate::Core;

const REPOSITORY_COLLECTION: &str = "schemahub.repositories.v1";
const MAX_REQUIRED_APPROVALS: u32 = 20;

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
    fn get(&self, project: &str, repo: &str) -> Result<Option<Repository>, RepositoryStoreError>;
    fn list(&self, project: &str) -> Result<Vec<Repository>, RepositoryStoreError>;
    fn replace(
        &self,
        expected_etag: &str,
        repository: Repository,
    ) -> Result<Repository, RepositoryStoreError>;
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
}

impl RepositoryStore for ObjectDbRepositoryStore {
    fn create(&self, mut repository: Repository) -> Result<Repository, RepositoryStoreError> {
        repository.etag = "v1".to_string();
        let bytes = Self::encode(&repository)?;
        let inserted = self
            .db
            .create_record(REPOSITORY_COLLECTION, &repository.resource_name, &bytes)
            .map_err(Self::map_db)?;
        if !inserted {
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

    fn replace(
        &self,
        expected_etag: &str,
        mut repository: Repository,
    ) -> Result<Repository, RepositoryStoreError> {
        let current_bytes = self
            .db
            .get_record(REPOSITORY_COLLECTION, &repository.resource_name)
            .map_err(Self::map_db)?
            .ok_or_else(|| RepositoryStoreError::NotFound(repository.resource_name.clone()))?;
        let current = Self::decode(&current_bytes)?;
        require_etag(&current, expected_etag)?;
        repository.etag = next_etag(&current.etag)?;
        let replacement = Self::encode(&repository)?;
        let replaced = self
            .db
            .compare_and_swap_record(
                REPOSITORY_COLLECTION,
                &repository.resource_name,
                &current_bytes,
                &replacement,
            )
            .map_err(Self::map_db)?;
        if replaced {
            return Ok(repository);
        }
        let latest = self
            .db
            .get_record(REPOSITORY_COLLECTION, &repository.resource_name)
            .map_err(Self::map_db)?
            .ok_or_else(|| RepositoryStoreError::NotFound(repository.resource_name.clone()))
            .and_then(|bytes| Self::decode(&bytes))?;
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
        Ok(self.repository_store.create(record)?)
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
        Ok(self
            .repository_store
            .list(project)?
            .into_iter()
            .filter(|repository| include_archived || !repository.archived)
            .collect())
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
        self.authorize_repo_action(token, Action::ManageRepo, project, repo)?;
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
        Ok(self.repository_store.replace(expected_etag, repository)?)
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
        self.authorize_repo_action(token, Action::ManageRepo, project, repo)?;
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
        Ok(self.repository_store.replace(expected_etag, repository)?)
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
        if self
            .jj
            .list_bookmarks(project, repo)?
            .iter()
            .any(|(_, targets)| !targets.is_empty())
        {
            return Ok(true);
        }
        Ok(!self.jj.list_tags(project, repo)?.is_empty())
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
    use schemahub_jj::RedbObjectDb;

    use super::*;

    fn repository(name: &str) -> Repository {
        Repository::new("acme", name, RepoConfig::default(), "alice", 1_000)
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
            .get("acme", "commerce")
            .expect("read repository")
            .expect("repository exists");

        // Assert
        assert_eq!(restored, expected);
    }
}
