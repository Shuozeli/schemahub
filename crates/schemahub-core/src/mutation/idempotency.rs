//! Durable RPC-edge idempotency with bounded retention and JJ reconciliation.
//!
//! A request claims a stable ObjectDb record immediately before its JJ write.
//! The same attempt id is stamped on the JJ operation. If the process stops
//! after publishing JJ but before completing the receipt, a retry finds the
//! correlated historical operation and repairs the receipt instead of writing
//! a second commit.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use schemahub_jj::{
    MemoryObjectDb, ObjectDb, ObjectDbError, PublicationError, RefSpec, SchemaWrite, WriteResult,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::request::{MutationResponse, TransactionDeadline};
use crate::{Core, CoreError, CoreResult};

const COLLECTION: &str = "schemahub.idempotency.v1";
const LOCK_COLLECTION: &str = "schemahub.idempotency.locks.v1";
const CAPACITY_LOCK_KEY: &str = "capacity";
const RECORD_ATTRIBUTE: &str = "schemahub.idempotency_record";
const ATTEMPT_ATTRIBUTE: &str = "schemahub.idempotency_attempt";
const MAX_CAS_RETRIES: usize = 8;

pub const DEFAULT_MAX_ENTRIES: usize = 1024;
pub const DEFAULT_TTL_HOURS: u32 = 24;
pub const DEFAULT_TTL_MS: i64 = DEFAULT_TTL_HOURS as i64 * 60 * 60 * 1_000;
pub const DEFAULT_LEASE_MS: i64 = 30_000;
const PENDING_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdempotencyError {
    #[error("invalid idempotency request: {0}")]
    InvalidArgument(String),
    #[error("idempotency key is already in progress: {0}")]
    InProgress(String),
    #[error("idempotency key was reused for a different request: {0}")]
    KeyReuse(String),
    #[error("idempotency capacity is occupied by in-progress requests")]
    Capacity,
    #[error("idempotency store error: {0}")]
    Backend(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IdempotencyAttempt {
    record_key: String,
    attempt_id: String,
    fingerprint: String,
}

impl IdempotencyAttempt {
    pub(crate) fn attributes(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (RECORD_ATTRIBUTE.to_string(), self.record_key.clone()),
            (ATTEMPT_ATTRIBUTE.to_string(), self.attempt_id.clone()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IdempotencyAcquisition {
    Disabled,
    Complete(MutationResponse),
    Acquired(IdempotencyAttempt),
    Observed(IdempotencyAttempt),
}

enum IdempotencyObservation {
    Disabled,
    Missing,
    Complete(MutationResponse),
    Pending {
        attempt: IdempotencyAttempt,
        lease_live: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdempotencyRecord {
    version: u32,
    record_key: String,
    scope: String,
    fingerprint: String,
    attempt_id: String,
    lease_expires_at_unix_ms: i64,
    create_time_unix_ms: i64,
    update_time_unix_ms: i64,
    completion_time_unix_ms: Option<i64>,
    response: Option<MutationResponse>,
}

#[derive(Serialize, Deserialize)]
struct CapacityLock {
    owner: String,
    expires_at_unix_ms: i64,
}

struct CapacityGuard {
    db: Arc<dyn ObjectDb>,
    bytes: Vec<u8>,
}

impl Drop for CapacityGuard {
    fn drop(&mut self) {
        let _ = self
            .db
            .compare_and_delete_record(LOCK_COLLECTION, CAPACITY_LOCK_KEY, &self.bytes);
    }
}

pub struct IdempotencyStore {
    db: Arc<dyn ObjectDb>,
    max_entries: usize,
    ttl_ms: i64,
    lease_ms: i64,
}

impl IdempotencyStore {
    pub fn new() -> Self {
        Self::over_object_db(Arc::new(MemoryObjectDb::new()))
    }

    pub fn over_object_db(db: Arc<dyn ObjectDb>) -> Self {
        Self::with_limits(db, DEFAULT_MAX_ENTRIES, DEFAULT_TTL_MS, DEFAULT_LEASE_MS)
    }

    pub fn with_limits(
        db: Arc<dyn ObjectDb>,
        max_entries: usize,
        ttl_ms: i64,
        lease_ms: i64,
    ) -> Self {
        Self {
            db,
            max_entries,
            ttl_ms,
            lease_ms,
        }
    }

    pub(crate) fn acquire(
        &self,
        scope: &str,
        client_key: &str,
        fingerprint: &str,
    ) -> Result<IdempotencyAcquisition, IdempotencyError> {
        validate_input(scope, client_key, fingerprint)?;
        if self.max_entries == 0 {
            return Ok(IdempotencyAcquisition::Disabled);
        }
        let record_key = record_key(scope, client_key);
        for _ in 0..MAX_CAS_RETRIES {
            let now = now_unix_millis()?;
            let Some(current_bytes) = self
                .db
                .get_record(COLLECTION, &record_key)
                .map_err(map_db)?
            else {
                let _capacity_guard = self.acquire_capacity_guard()?;
                if self
                    .db
                    .get_record(COLLECTION, &record_key)
                    .map_err(map_db)?
                    .is_some()
                {
                    continue;
                }
                self.prune_with_reserve(1)?;
                let record = IdempotencyRecord {
                    version: 1,
                    record_key: record_key.clone(),
                    scope: scope.to_string(),
                    fingerprint: fingerprint.to_string(),
                    attempt_id: Uuid::new_v4().to_string(),
                    lease_expires_at_unix_ms: now.checked_add(self.lease_ms).ok_or_else(|| {
                        IdempotencyError::Backend("lease timestamp overflow".into())
                    })?,
                    create_time_unix_ms: now,
                    update_time_unix_ms: now,
                    completion_time_unix_ms: None,
                    response: None,
                };
                if self
                    .db
                    .create_record(COLLECTION, &record_key, &encode(&record)?)
                    .map_err(map_db)?
                {
                    return Ok(IdempotencyAcquisition::Acquired(attempt(&record)));
                }
                continue;
            };
            let mut record = decode(&current_bytes)?;
            validate_existing(&record, &record_key, scope, fingerprint)?;
            if let Some(response) = record.response.clone() {
                if is_expired(&record, now, self.ttl_ms)? {
                    self.db
                        .compare_and_delete_record(COLLECTION, &record_key, &current_bytes)
                        .map_err(map_db)?;
                    continue;
                }
                return Ok(IdempotencyAcquisition::Complete(response));
            }
            if record.lease_expires_at_unix_ms > now {
                return Ok(IdempotencyAcquisition::Observed(attempt(&record)));
            }
            record.lease_expires_at_unix_ms = now
                .checked_add(self.lease_ms)
                .ok_or_else(|| IdempotencyError::Backend("lease timestamp overflow".into()))?;
            record.update_time_unix_ms = now;
            let replacement = encode(&record)?;
            if self
                .db
                .compare_and_swap_record(COLLECTION, &record_key, &current_bytes, &replacement)
                .map_err(map_db)?
            {
                return Ok(IdempotencyAcquisition::Acquired(attempt(&record)));
            }
        }
        Err(IdempotencyError::InProgress(record_key))
    }

    fn acquire_capacity_guard(&self) -> Result<CapacityGuard, IdempotencyError> {
        for _ in 0..MAX_CAS_RETRIES {
            let now = now_unix_millis()?;
            let lock = CapacityLock {
                owner: Uuid::new_v4().to_string(),
                expires_at_unix_ms: now
                    .checked_add(DEFAULT_LEASE_MS)
                    .ok_or_else(|| IdempotencyError::Backend("lock timestamp overflow".into()))?,
            };
            let replacement = serde_json::to_vec(&lock).map_err(|error| {
                IdempotencyError::Backend(format!("encode capacity lock: {error}"))
            })?;
            let current = self
                .db
                .get_record(LOCK_COLLECTION, CAPACITY_LOCK_KEY)
                .map_err(map_db)?;
            match current {
                None => {
                    if self
                        .db
                        .create_record(LOCK_COLLECTION, CAPACITY_LOCK_KEY, &replacement)
                        .map_err(map_db)?
                    {
                        return Ok(CapacityGuard {
                            db: self.db.clone(),
                            bytes: replacement,
                        });
                    }
                }
                Some(current) => {
                    let existing: CapacityLock =
                        serde_json::from_slice(&current).map_err(|error| {
                            IdempotencyError::Backend(format!("decode capacity lock: {error}"))
                        })?;
                    if existing.expires_at_unix_ms <= now
                        && self
                            .db
                            .compare_and_swap_record(
                                LOCK_COLLECTION,
                                CAPACITY_LOCK_KEY,
                                &current,
                                &replacement,
                            )
                            .map_err(map_db)?
                    {
                        return Ok(CapacityGuard {
                            db: self.db.clone(),
                            bytes: replacement,
                        });
                    }
                }
            }
            std::thread::yield_now();
        }
        Err(IdempotencyError::InProgress(
            "capacity reservation".to_string(),
        ))
    }

    fn observe(
        &self,
        scope: &str,
        client_key: &str,
        fingerprint: &str,
    ) -> Result<IdempotencyObservation, IdempotencyError> {
        validate_input(scope, client_key, fingerprint)?;
        if self.max_entries == 0 {
            return Ok(IdempotencyObservation::Disabled);
        }
        let record_key = record_key(scope, client_key);
        for _ in 0..MAX_CAS_RETRIES {
            let Some(bytes) = self
                .db
                .get_record(COLLECTION, &record_key)
                .map_err(map_db)?
            else {
                return Ok(IdempotencyObservation::Missing);
            };
            let record = decode(&bytes)?;
            validate_existing(&record, &record_key, scope, fingerprint)?;
            let now = now_unix_millis()?;
            if let Some(response) = record.response.clone() {
                if is_expired(&record, now, self.ttl_ms)? {
                    if self
                        .db
                        .compare_and_delete_record(COLLECTION, &record_key, &bytes)
                        .map_err(map_db)?
                    {
                        return Ok(IdempotencyObservation::Missing);
                    }
                    continue;
                }
                return Ok(IdempotencyObservation::Complete(response));
            }
            return Ok(IdempotencyObservation::Pending {
                attempt: attempt(&record),
                lease_live: record.lease_expires_at_unix_ms > now,
            });
        }
        Err(IdempotencyError::InProgress(record_key))
    }

    pub(crate) fn complete(
        &self,
        attempt: &IdempotencyAttempt,
        response: MutationResponse,
    ) -> Result<MutationResponse, IdempotencyError> {
        for _ in 0..MAX_CAS_RETRIES {
            let current_bytes = self
                .db
                .get_record(COLLECTION, &attempt.record_key)
                .map_err(map_db)?
                .ok_or_else(|| {
                    IdempotencyError::Backend(format!(
                        "idempotency record {} disappeared before completion",
                        attempt.record_key
                    ))
                })?;
            let mut record = decode(&current_bytes)?;
            if record.attempt_id != attempt.attempt_id || record.fingerprint != attempt.fingerprint
            {
                return Err(IdempotencyError::KeyReuse(attempt.record_key.clone()));
            }
            if let Some(stored) = record.response {
                if stored == response {
                    return Ok(stored);
                }
                return Err(IdempotencyError::Backend(format!(
                    "idempotency record {} has a different completion receipt",
                    attempt.record_key
                )));
            }
            let now = now_unix_millis()?;
            record.response = Some(response.clone());
            record.completion_time_unix_ms = Some(now);
            record.update_time_unix_ms = now;
            record.lease_expires_at_unix_ms = 0;
            let replacement = encode(&record)?;
            if self
                .db
                .compare_and_swap_record(
                    COLLECTION,
                    &attempt.record_key,
                    &current_bytes,
                    &replacement,
                )
                .map_err(map_db)?
            {
                self.prune()?;
                return Ok(response);
            }
        }
        Err(IdempotencyError::InProgress(attempt.record_key.clone()))
    }

    /// Remove a still-pending receipt after a publication policy rejected the
    /// candidate before JJ committed an operation. This is deliberately not
    /// used for operational JJ errors, whose publication outcome can be
    /// ambiguous and must remain recoverable through correlation metadata.
    pub(crate) fn abort(&self, attempt: &IdempotencyAttempt) -> Result<(), IdempotencyError> {
        for _ in 0..MAX_CAS_RETRIES {
            let Some(current_bytes) = self
                .db
                .get_record(COLLECTION, &attempt.record_key)
                .map_err(map_db)?
            else {
                return Ok(());
            };
            let record = decode(&current_bytes)?;
            if record.attempt_id != attempt.attempt_id || record.fingerprint != attempt.fingerprint
            {
                return Err(IdempotencyError::KeyReuse(attempt.record_key.clone()));
            }
            if record.response.is_some() {
                return Err(IdempotencyError::Backend(format!(
                    "cannot abort completed idempotency record {}",
                    attempt.record_key
                )));
            }
            if self
                .db
                .compare_and_delete_record(COLLECTION, &attempt.record_key, &current_bytes)
                .map_err(map_db)?
            {
                return Ok(());
            }
        }
        Err(IdempotencyError::InProgress(attempt.record_key.clone()))
    }

    pub fn prune(&self) -> Result<usize, IdempotencyError> {
        self.prune_with_reserve(0)
    }

    fn prune_with_reserve(&self, reserve: usize) -> Result<usize, IdempotencyError> {
        let now = now_unix_millis()?;
        let mut records = Vec::new();
        let mut deleted = 0;
        for (key, bytes) in self.db.list_records(COLLECTION).map_err(map_db)? {
            let record = decode(&bytes)?;
            let expired = is_expired(&record, now, self.ttl_ms)?;
            if expired
                && self
                    .db
                    .compare_and_delete_record(COLLECTION, &key, &bytes)
                    .map_err(map_db)?
            {
                deleted += 1;
            } else if !expired {
                records.push((key, bytes, record));
            }
        }

        let target = self.max_entries.saturating_sub(reserve);
        if records.len() > target {
            let mut completed: Vec<_> = records
                .iter()
                .filter(|(_, _, record)| record.response.is_some())
                .collect();
            completed.sort_by(|left, right| {
                left.2
                    .completion_time_unix_ms
                    .cmp(&right.2.completion_time_unix_ms)
                    .then_with(|| left.0.cmp(&right.0))
            });
            let mut excess = records.len() - target;
            for (key, bytes, _) in completed {
                if excess == 0 {
                    break;
                }
                if self
                    .db
                    .compare_and_delete_record(COLLECTION, key, bytes)
                    .map_err(map_db)?
                {
                    deleted += 1;
                    excess -= 1;
                }
            }
            if excess > 0 {
                return Err(IdempotencyError::Capacity);
            }
        }
        Ok(deleted)
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) enum IdempotentWrite {
    Replay(MutationResponse),
    Proceed(Option<IdempotencyAttempt>),
}

impl Core {
    /// Replay a completed/correlated receipt before performing state-dependent
    /// validation. Missing or expired-unpublished receipts return `None`; a
    /// concurrently live request returns `InProgress`.
    #[allow(clippy::too_many_arguments)]
    pub fn replay_idempotent_write(
        &self,
        scope: &str,
        client_key: Option<&str>,
        fingerprint: &str,
        project: &str,
        repo: &str,
        bookmark: &str,
    ) -> CoreResult<Option<MutationResponse>> {
        let Some(client_key) = client_key else {
            return Ok(None);
        };
        match self.idempotency.observe(scope, client_key, fingerprint)? {
            IdempotencyObservation::Disabled | IdempotencyObservation::Missing => Ok(None),
            IdempotencyObservation::Complete(response) => Ok(Some(response)),
            IdempotencyObservation::Pending {
                attempt,
                lease_live,
            } => {
                if let Some(write) =
                    self.jj
                        .find_correlated_write(project, repo, bookmark, &attempt.attributes())?
                {
                    return Ok(Some(
                        self.idempotency
                            .complete(&attempt, response_from_write(write))?,
                    ));
                }
                if lease_live {
                    return Err(IdempotencyError::InProgress(attempt.record_key).into());
                }
                Ok(None)
            }
        }
    }

    pub(crate) fn begin_idempotent_write(
        &self,
        scope: &str,
        client_key: Option<&str>,
        fingerprint: &str,
        project: &str,
        repo: &str,
        bookmark: &str,
    ) -> CoreResult<IdempotentWrite> {
        let Some(client_key) = client_key else {
            return Ok(IdempotentWrite::Proceed(None));
        };
        match self.idempotency.acquire(scope, client_key, fingerprint)? {
            IdempotencyAcquisition::Disabled => Ok(IdempotentWrite::Proceed(None)),
            IdempotencyAcquisition::Complete(response) => Ok(IdempotentWrite::Replay(response)),
            IdempotencyAcquisition::Acquired(attempt) => {
                if let Some(write) =
                    self.jj
                        .find_correlated_write(project, repo, bookmark, &attempt.attributes())?
                {
                    let response = response_from_write(write);
                    return Ok(IdempotentWrite::Replay(
                        self.idempotency.complete(&attempt, response)?,
                    ));
                }
                Ok(IdempotentWrite::Proceed(Some(attempt)))
            }
            IdempotencyAcquisition::Observed(attempt) => {
                if let Some(write) =
                    self.jj
                        .find_correlated_write(project, repo, bookmark, &attempt.attributes())?
                {
                    let response = response_from_write(write);
                    return Ok(IdempotentWrite::Replay(
                        self.idempotency.complete(&attempt, response)?,
                    ));
                }
                Err(IdempotencyError::InProgress(attempt.record_key).into())
            }
        }
    }

    pub(crate) fn complete_idempotent_write(
        &self,
        attempt: Option<&IdempotencyAttempt>,
        response: MutationResponse,
    ) -> CoreResult<MutationResponse> {
        match attempt {
            Some(attempt) => Ok(self.idempotency.complete(attempt, response)?),
            None => Ok(response),
        }
    }

    pub(crate) fn abort_idempotent_write(
        &self,
        attempt: Option<&IdempotencyAttempt>,
    ) -> CoreResult<()> {
        if let Some(attempt) = attempt {
            self.idempotency.abort(attempt)?;
        }
        Ok(())
    }

    pub fn prune_idempotency(&self) -> CoreResult<usize> {
        Ok(self.idempotency.prune()?)
    }

    /// Publish schema-file writes under a durable idempotency receipt. The
    /// receipt is claimed immediately before the JJ operation, and the same
    /// attempt metadata is stamped onto that operation for crash recovery.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_idempotent_schema_changes(
        &self,
        scope: &str,
        client_key: Option<&str>,
        fingerprint: &str,
        project: &str,
        repo: &str,
        bookmark: &str,
        base_ref: &RefSpec,
        writes: Vec<SchemaWrite>,
        author: &str,
        message: &str,
    ) -> CoreResult<MutationResponse> {
        self.commit_idempotent_schema_changes_with_attributes(
            scope,
            client_key,
            fingerprint,
            project,
            repo,
            bookmark,
            base_ref,
            writes,
            author,
            message,
            BTreeMap::new(),
        )
    }

    /// Publish an idempotent schema write with workflow audit attributes. JJ
    /// correlation keys are always inserted last so a caller cannot shadow the
    /// durable receipt identity.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_idempotent_schema_changes_with_attributes(
        &self,
        scope: &str,
        client_key: Option<&str>,
        fingerprint: &str,
        project: &str,
        repo: &str,
        bookmark: &str,
        base_ref: &RefSpec,
        writes: Vec<SchemaWrite>,
        author: &str,
        message: &str,
        attributes: BTreeMap<String, String>,
    ) -> CoreResult<MutationResponse> {
        self.commit_idempotent_schema_changes_with_attributes_and_deadline(
            scope,
            client_key,
            fingerprint,
            project,
            repo,
            bookmark,
            base_ref,
            writes,
            author,
            message,
            attributes,
            None,
        )
    }

    /// Deadline-aware variant used by the transaction RPC. The final deadline
    /// check runs inside JJ's publication callback while the repository guard
    /// is held. A deadline rejection is therefore known to precede publication
    /// and safely releases any pending receipt.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_idempotent_schema_changes_with_attributes_and_deadline(
        &self,
        scope: &str,
        client_key: Option<&str>,
        fingerprint: &str,
        project: &str,
        repo: &str,
        bookmark: &str,
        base_ref: &RefSpec,
        writes: Vec<SchemaWrite>,
        author: &str,
        message: &str,
        mut attributes: BTreeMap<String, String>,
        deadline: Option<&TransactionDeadline>,
    ) -> CoreResult<MutationResponse> {
        let config = self.effective_repo_config(project, repo)?;
        let protected = schemahub_jj::bookmark::is_protected(bookmark, &config.protected_bookmarks);
        let attempt = match self.begin_idempotent_write(
            scope,
            client_key,
            fingerprint,
            project,
            repo,
            bookmark,
        )? {
            IdempotentWrite::Replay(response) => return Ok(response),
            IdempotentWrite::Proceed(attempt) => attempt,
        };
        if deadline.is_some_and(TransactionDeadline::is_exceeded) {
            self.abort_idempotent_write(attempt.as_ref())?;
            return Err(CoreError::TransactionDeadlineExceeded);
        }
        if let Some(attempt) = &attempt {
            attributes.extend(attempt.attributes());
        }
        let write = match self.jj.commit_schema_changes_validated(
            project,
            repo,
            bookmark,
            base_ref,
            writes,
            author,
            message,
            attributes,
            |snapshot| {
                if deadline.is_some_and(TransactionDeadline::is_exceeded) {
                    return Err(CoreError::TransactionDeadlineExceeded);
                }
                self.validate_publication_snapshot(project, repo, bookmark, protected, snapshot)
            },
        ) {
            Ok(write) => write,
            Err(PublicationError::Jj(error)) => return Err(error.into()),
            Err(PublicationError::Rejected(error)) => {
                self.abort_idempotent_write(attempt.as_ref())?;
                return Err(error);
            }
        };
        self.complete_idempotent_write(attempt.as_ref(), response_from_write(write))
    }
}

pub(crate) fn force_audit_attributes(force: bool) -> BTreeMap<String, String> {
    force
        .then(|| ("schemahub.force".to_string(), "true".to_string()))
        .into_iter()
        .collect()
}

/// Length-delimited request fingerprint builder used by every idempotent write
/// surface. Callers should add all semantic request fields except credentials
/// and the idempotency key itself.
pub struct FingerprintBuilder(Sha256);

impl FingerprintBuilder {
    pub fn new(kind: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"schemahub-idempotency-fingerprint-v1");
        let mut builder = Self(hasher);
        builder.update(kind.as_bytes());
        builder
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
    }

    pub fn finish(self) -> String {
        format!("sha256:{}", hex::encode(self.0.finalize()))
    }
}

fn response_from_write(write: WriteResult) -> MutationResponse {
    MutationResponse {
        commit_id: write.commit_id,
        change_id: write.change_id,
        conflicted_decls: write.conflicted_decls,
    }
}

fn attempt(record: &IdempotencyRecord) -> IdempotencyAttempt {
    IdempotencyAttempt {
        record_key: record.record_key.clone(),
        attempt_id: record.attempt_id.clone(),
        fingerprint: record.fingerprint.clone(),
    }
}

fn validate_input(scope: &str, key: &str, fingerprint: &str) -> Result<(), IdempotencyError> {
    if scope.is_empty() || scope.len() > 512 || scope.chars().any(char::is_control) {
        return Err(IdempotencyError::InvalidArgument(
            "scope must be 1-512 characters without control characters".into(),
        ));
    }
    if key.is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
        return Err(IdempotencyError::InvalidArgument(
            "key must be 1-512 characters without control characters".into(),
        ));
    }
    if !fingerprint.starts_with("sha256:") || fingerprint.len() != 71 {
        return Err(IdempotencyError::InvalidArgument(
            "fingerprint must be a sha256 digest".into(),
        ));
    }
    Ok(())
}

fn validate_existing(
    record: &IdempotencyRecord,
    record_key: &str,
    scope: &str,
    fingerprint: &str,
) -> Result<(), IdempotencyError> {
    if record.version != 1 || record.record_key != record_key || record.scope != scope {
        return Err(IdempotencyError::Backend(format!(
            "idempotency record {record_key} has inconsistent identity"
        )));
    }
    if record.fingerprint != fingerprint {
        return Err(IdempotencyError::KeyReuse(record_key.to_string()));
    }
    Ok(())
}

fn is_expired(
    record: &IdempotencyRecord,
    now: i64,
    completed_ttl_ms: i64,
) -> Result<bool, IdempotencyError> {
    match (&record.response, record.completion_time_unix_ms) {
        (Some(_), Some(completed)) => Ok(now.saturating_sub(completed) >= completed_ttl_ms),
        (None, None) => Ok(now.saturating_sub(record.create_time_unix_ms) >= PENDING_RETENTION_MS),
        _ => Err(IdempotencyError::Backend(format!(
            "idempotency record {} has inconsistent completion state",
            record.record_key
        ))),
    }
}

fn record_key(scope: &str, client_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"schemahub-idempotency-key-v1\0");
    hasher.update(scope.as_bytes());
    hasher.update(b"\0");
    hasher.update(client_key.as_bytes());
    hex::encode(hasher.finalize())
}

fn encode(record: &IdempotencyRecord) -> Result<Vec<u8>, IdempotencyError> {
    serde_json::to_vec(record)
        .map_err(|error| IdempotencyError::Backend(format!("encode record: {error}")))
}

fn decode(bytes: &[u8]) -> Result<IdempotencyRecord, IdempotencyError> {
    serde_json::from_slice(bytes)
        .map_err(|error| IdempotencyError::Backend(format!("decode record: {error}")))
}

fn now_unix_millis() -> Result<i64, IdempotencyError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| IdempotencyError::Backend(error.to_string()))?;
    i64::try_from(duration.as_millis())
        .map_err(|_| IdempotencyError::Backend("system timestamp exceeds i64".into()))
}

fn map_db(error: ObjectDbError) -> IdempotencyError {
    IdempotencyError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;

    fn response(commit: &str) -> MutationResponse {
        MutationResponse {
            commit_id: commit.to_string(),
            change_id: format!("change-{commit}"),
            conflicted_decls: Vec::new(),
        }
    }

    fn fingerprint(value: &str) -> String {
        let mut builder = FingerprintBuilder::new("test");
        builder.update(value.as_bytes());
        builder.finish()
    }

    #[test]
    fn completed_receipt_is_shared_across_store_instances() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let first = IdempotencyStore::over_object_db(db.clone());
        let second = IdempotencyStore::over_object_db(db);
        let acquired = first.acquire("mutation/acme/core", "k1", &fingerprint("a"));
        let IdempotencyAcquisition::Acquired(attempt) = acquired.unwrap() else {
            panic!("first request must acquire");
        };
        first
            .complete(&attempt, response("commit-1"))
            .expect("complete receipt");

        // Act
        let replay = second
            .acquire("mutation/acme/core", "k1", &fingerprint("a"))
            .expect("read receipt");

        // Assert
        assert_eq!(
            replay,
            IdempotencyAcquisition::Complete(response("commit-1"))
        );
    }

    #[test]
    fn key_reuse_with_different_request_is_rejected() {
        // Arrange
        let store = IdempotencyStore::new();
        store
            .acquire("mutation/acme/core", "k1", &fingerprint("a"))
            .expect("first acquisition");

        // Act
        let result = store.acquire("mutation/acme/core", "k1", &fingerprint("b"));

        // Assert
        assert!(matches!(result, Err(IdempotencyError::KeyReuse(_))));
    }

    #[test]
    fn live_pending_request_is_observed_not_reacquired() {
        // Arrange
        let store = IdempotencyStore::new();
        store
            .acquire("mutation/acme/core", "k1", &fingerprint("a"))
            .expect("first acquisition");

        // Act
        let result = store
            .acquire("mutation/acme/core", "k1", &fingerprint("a"))
            .expect("observe acquisition");

        // Assert
        assert!(matches!(result, IdempotencyAcquisition::Observed(_)));
    }

    #[test]
    fn policy_rejection_abort_allows_immediate_retry() {
        // Arrange
        let store = IdempotencyStore::new();
        let first = store
            .acquire("mutation/acme/core", "k1", &fingerprint("a"))
            .expect("first acquisition");
        let IdempotencyAcquisition::Acquired(first_attempt) = first else {
            panic!("first request must acquire");
        };

        // Act
        store.abort(&first_attempt).expect("abort pending receipt");
        let retried = store
            .acquire("mutation/acme/core", "k1", &fingerprint("a"))
            .expect("retry acquisition");

        // Assert
        let IdempotencyAcquisition::Acquired(second_attempt) = retried else {
            panic!("retry must acquire a fresh attempt");
        };
        assert_ne!(second_attempt.attempt_id, first_attempt.attempt_id);
    }

    #[test]
    fn capacity_evicts_oldest_completed_receipt() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let store = IdempotencyStore::with_limits(db, 1, DEFAULT_TTL_MS, DEFAULT_LEASE_MS);
        let IdempotencyAcquisition::Acquired(attempt) = store
            .acquire("mutation/acme/core", "k1", &fingerprint("k1"))
            .unwrap()
        else {
            panic!("request must acquire");
        };
        store.complete(&attempt, response("c1")).unwrap();

        // Act
        let third = store
            .acquire("mutation/acme/core", "k2", &fingerprint("k2"))
            .expect("reserve replacement entry");
        let IdempotencyAcquisition::Acquired(second_attempt) = &third else {
            panic!("replacement request must acquire");
        };
        store
            .complete(second_attempt, response("c2"))
            .expect("complete replacement receipt");
        let first = store
            .acquire("mutation/acme/core", "k1", &fingerprint("k1"))
            .expect("oldest key was evicted and can acquire again");

        // Assert
        assert!(matches!(third, IdempotencyAcquisition::Acquired(_)));
        assert!(matches!(first, IdempotencyAcquisition::Acquired(_)));
    }

    #[test]
    fn same_key_lazily_discards_an_expired_completed_receipt() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let store = IdempotencyStore::with_limits(db.clone(), 10, 1, DEFAULT_LEASE_MS);
        let scope = "mutation/acme/core";
        let request_fingerprint = fingerprint("expired");
        let key = record_key(scope, "k1");
        let completed = now_unix_millis().unwrap() - 10;
        let record = IdempotencyRecord {
            version: 1,
            record_key: key.clone(),
            scope: scope.to_string(),
            fingerprint: request_fingerprint.clone(),
            attempt_id: Uuid::new_v4().to_string(),
            lease_expires_at_unix_ms: 0,
            create_time_unix_ms: completed,
            update_time_unix_ms: completed,
            completion_time_unix_ms: Some(completed),
            response: Some(response("expired-commit")),
        };
        db.create_record(COLLECTION, &key, &encode(&record).unwrap())
            .unwrap();

        // Act
        let acquisition = store
            .acquire(scope, "k1", &request_fingerprint)
            .expect("expired receipt can be reclaimed");

        // Assert
        assert!(matches!(acquisition, IdempotencyAcquisition::Acquired(_)));
    }

    #[test]
    fn concurrent_store_instances_do_not_exceed_the_pending_capacity_bound() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = ["k1", "k2"]
            .into_iter()
            .map(|key| {
                let db = db.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let store =
                        IdempotencyStore::with_limits(db, 1, DEFAULT_TTL_MS, DEFAULT_LEASE_MS);
                    barrier.wait();
                    store.acquire("mutation/acme/core", key, &fingerprint(key))
                })
            })
            .collect();

        // Act
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("worker did not panic"))
            .collect();
        let records = db.list_records(COLLECTION).unwrap();

        // Assert
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Ok(IdempotencyAcquisition::Acquired(_))))
                .count(),
            1
        );
        assert!(results.iter().any(|result| matches!(
            result,
            Err(IdempotencyError::Capacity | IdempotencyError::InProgress(_))
        )));
        assert_eq!(records.len(), 1);
    }
}
