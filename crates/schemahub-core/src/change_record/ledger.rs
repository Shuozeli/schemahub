use std::collections::HashSet;
use std::sync::Arc;

use schemahub_types::Identity;
use thiserror::Error;

use super::{
    ApplyAttempt, ApplyResult, ChangeActor, ChangeClock, ChangeIdGenerator, ChangeRecord,
    ChangeRecordStatus, ChangeReview, ChangeReviewDecision, ChangeRuntimeError, ChangeStoreError,
    ChangeUpdate, CreateChange, ValidationResult,
};
use crate::change_record::ChangeRecordStore;

#[derive(Debug, Error)]
pub enum ChangeLedgerError {
    #[error("invalid change record: {0}")]
    InvalidArgument(String),
    #[error("change record precondition failed: {0}")]
    FailedPrecondition(String),
    #[error(transparent)]
    Store(#[from] ChangeStoreError),
    #[error(transparent)]
    Runtime(#[from] ChangeRuntimeError),
}

/// Lifecycle service over a transactional store with injected time and IDs.
#[derive(Clone)]
pub struct ChangeLedger {
    store: Arc<dyn ChangeRecordStore>,
    clock: Arc<dyn ChangeClock>,
    ids: Arc<dyn ChangeIdGenerator>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyAcquisition {
    /// This caller owns the current lease and may create the JJ operation.
    Acquired(ChangeRecord),
    /// Another caller owns an unexpired lease; reconciliation may still finish
    /// a JJ operation that is already visible.
    Observed(ChangeRecord),
    /// The same request id was already applied.
    AlreadyApplied(ChangeRecord),
}

impl ChangeLedger {
    pub fn new(
        store: Arc<dyn ChangeRecordStore>,
        clock: Arc<dyn ChangeClock>,
        ids: Arc<dyn ChangeIdGenerator>,
    ) -> Self {
        Self { store, clock, ids }
    }

    pub fn create(
        &self,
        input: CreateChange,
        identity: &Identity,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        validate_segment("project", &input.project)?;
        validate_segment("repo", &input.repo)?;
        validate_nonempty("target_bookmark", &input.target_bookmark)?;
        validate_title(&input.title)?;
        let external_references = normalize_external_references(input.external_references)?;

        let change_id = input
            .change_id
            .unwrap_or_else(|| self.ids.generate_change_id());
        validate_change_id(&change_id)?;
        let now = self.clock.now_unix_millis()?;
        let record = ChangeRecord {
            name: format!(
                "projects/{}/repos/{}/changes/{change_id}",
                input.project, input.repo
            ),
            project: input.project,
            repo: input.repo,
            target_bookmark: input.target_bookmark,
            base_revision: input.base_revision,
            title: input.title.trim().to_string(),
            description: input.description,
            external_references,
            edits: input.edits,
            created_by: ChangeActor::from(identity),
            status: ChangeRecordStatus::Draft,
            validation: None,
            reviews: Vec::new(),
            apply_attempt: None,
            apply_result: None,
            etag: String::new(),
            create_time_unix_ms: now,
            update_time_unix_ms: now,
        };
        Ok(self.store.create(record)?)
    }

    pub fn get(&self, name: &str) -> Result<ChangeRecord, ChangeLedgerError> {
        validate_resource_name(name)?;
        self.store
            .get(name)?
            .ok_or_else(|| ChangeStoreError::NotFound(name.to_string()).into())
    }

    /// Read a record only when the caller's optimistic-concurrency token is
    /// current. This lets Core reject stale lifecycle requests before running
    /// compiler validation or other potentially expensive work.
    pub fn get_at_etag(
        &self,
        name: &str,
        expected_etag: &str,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        if expected_etag.is_empty() {
            return Err(ChangeLedgerError::InvalidArgument(
                "etag must not be empty".to_string(),
            ));
        }
        let record = self.get(name)?;
        if record.etag != expected_etag {
            return Err(ChangeStoreError::EtagMismatch {
                name: name.to_string(),
                expected: expected_etag.to_string(),
                current: record.etag,
            }
            .into());
        }
        Ok(record)
    }

    pub fn list(&self, project: &str, repo: &str) -> Result<Vec<ChangeRecord>, ChangeLedgerError> {
        validate_segment("project", project)?;
        validate_segment("repo", repo)?;
        Ok(self.store.list(project, repo)?)
    }

    /// Parse and validate a change resource name, returning its repository
    /// scope. Core uses this before touching storage so authorization happens
    /// without first revealing whether the named record exists.
    pub fn scope(name: &str) -> Result<(&str, &str), ChangeLedgerError> {
        let parts = parse_resource_name(name)?;
        Ok((parts[1], parts[3]))
    }

    /// Validate a project/repository parent before authorization or storage.
    pub fn validate_scope(project: &str, repo: &str) -> Result<(), ChangeLedgerError> {
        validate_segment("project", project)?;
        validate_segment("repo", repo)
    }

    pub fn update(
        &self,
        name: &str,
        expected_etag: &str,
        patch: ChangeUpdate,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        if expected_etag.is_empty() {
            return Err(ChangeLedgerError::InvalidArgument(
                "etag must not be empty".to_string(),
            ));
        }
        if patch.is_empty() {
            return Err(ChangeLedgerError::InvalidArgument(
                "update mask selects no fields".to_string(),
            ));
        }
        let mut record = self.get(name)?;
        require_status(&record, ChangeRecordStatus::Draft, "update")?;

        let invalidates_validation = patch.target_bookmark.is_some()
            || patch.base_revision.is_some()
            || patch.edits.is_some();
        if let Some(target) = patch.target_bookmark {
            validate_nonempty("target_bookmark", &target)?;
            record.target_bookmark = target;
        }
        if let Some(base) = patch.base_revision {
            record.base_revision = (!base.is_empty()).then_some(base);
        }
        if let Some(title) = patch.title {
            validate_title(&title)?;
            record.title = title.trim().to_string();
        }
        if let Some(description) = patch.description {
            record.description = description;
        }
        if let Some(external_references) = patch.external_references {
            record.external_references = normalize_external_references(external_references)?;
        }
        if let Some(edits) = patch.edits {
            record.edits = edits;
        }
        if invalidates_validation {
            record.validation = None;
        }
        record.update_time_unix_ms = self.clock.now_unix_millis()?;
        Ok(self.store.replace(expected_etag, record)?)
    }

    /// Store a server-produced validation snapshot for the current draft.
    pub fn record_validation(
        &self,
        name: &str,
        expected_etag: &str,
        mut validation: ValidationResult,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        let mut record = self.get(name)?;
        require_status(&record, ChangeRecordStatus::Draft, "validate")?;
        let now = self.clock.now_unix_millis()?;
        validation.validated_at_unix_ms = now;
        record.validation = Some(validation);
        record.update_time_unix_ms = now;
        Ok(self.store.replace(expected_etag, record)?)
    }

    pub fn mark_ready(
        &self,
        name: &str,
        expected_etag: &str,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        let mut record = self.get(name)?;
        require_status(&record, ChangeRecordStatus::Draft, "mark ready")?;
        if record.edits.is_empty() {
            return Err(ChangeLedgerError::FailedPrecondition(
                "a note-only draft cannot be marked ready".to_string(),
            ));
        }
        let validation = record.validation.as_ref().ok_or_else(|| {
            ChangeLedgerError::FailedPrecondition(
                "the current edits have not been validated".to_string(),
            )
        })?;
        if !validation.valid {
            return Err(ChangeLedgerError::FailedPrecondition(
                "the current validation contains blocking issues".to_string(),
            ));
        }
        record.status = ChangeRecordStatus::Ready;
        record.update_time_unix_ms = self.clock.now_unix_millis()?;
        Ok(self.store.replace(expected_etag, record)?)
    }

    /// Acquire or observe the durable lease for an idempotent Apply request.
    /// Compare-and-set serialization happens in the backing store, so this is
    /// safe across server processes sharing redb/PostgreSQL storage.
    pub fn acquire_apply(
        &self,
        name: &str,
        expected_etag: &str,
        request_id: &str,
        actor: &Identity,
        lease_duration_ms: i64,
    ) -> Result<ApplyAcquisition, ChangeLedgerError> {
        validate_request_id(request_id)?;
        if expected_etag.is_empty() {
            return Err(ChangeLedgerError::InvalidArgument(
                "etag must not be empty".to_string(),
            ));
        }
        if lease_duration_ms <= 0 {
            return Err(ChangeLedgerError::InvalidArgument(
                "apply lease duration must be positive".to_string(),
            ));
        }

        for _ in 0..4 {
            let mut record = self.get(name)?;
            match record.status {
                ChangeRecordStatus::Applied => {
                    let attempt = record.apply_attempt.as_ref().ok_or_else(|| {
                        ChangeLedgerError::FailedPrecondition(
                            "applied change is missing its apply attempt".to_string(),
                        )
                    })?;
                    if attempt.request_id == request_id {
                        return Ok(ApplyAcquisition::AlreadyApplied(record));
                    }
                    return Err(ChangeLedgerError::FailedPrecondition(
                        "change was already applied by a different request id".to_string(),
                    ));
                }
                ChangeRecordStatus::Ready => {
                    if record.etag != expected_etag {
                        return Err(ChangeStoreError::EtagMismatch {
                            name: name.to_string(),
                            expected: expected_etag.to_string(),
                            current: record.etag,
                        }
                        .into());
                    }
                    if !record
                        .validation
                        .as_ref()
                        .is_some_and(|result| result.valid)
                    {
                        return Err(ChangeLedgerError::FailedPrecondition(
                            "change does not have a passing validation snapshot".to_string(),
                        ));
                    }
                    let now = self.clock.now_unix_millis()?;
                    let lease_expires_at_unix_ms = now
                        .checked_add(lease_duration_ms)
                        .ok_or(ChangeRuntimeError::TimestampOverflow)?;
                    record.status = ChangeRecordStatus::Applying;
                    record.apply_attempt = Some(ApplyAttempt {
                        request_id: request_id.to_string(),
                        attempt_id: self.ids.generate_apply_attempt_id(),
                        actor: ChangeActor::from(actor),
                        lease_owner: self.ids.generate_apply_lease_owner(),
                        lease_expires_at_unix_ms,
                        start_time_unix_ms: now,
                        update_time_unix_ms: now,
                    });
                    record.update_time_unix_ms = now;
                    match self.store.replace(expected_etag, record) {
                        Ok(acquired) => return Ok(ApplyAcquisition::Acquired(acquired)),
                        Err(ChangeStoreError::EtagMismatch { .. }) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                ChangeRecordStatus::Applying => {
                    let now = self.clock.now_unix_millis()?;
                    let attempt = record.apply_attempt.as_mut().ok_or_else(|| {
                        ChangeLedgerError::FailedPrecondition(
                            "applying change is missing its apply attempt".to_string(),
                        )
                    })?;
                    if attempt.request_id != request_id {
                        return Err(ChangeLedgerError::FailedPrecondition(
                            "another request id is already applying this change".to_string(),
                        ));
                    }
                    if attempt.lease_expires_at_unix_ms > now {
                        return Ok(ApplyAcquisition::Observed(record));
                    }
                    let current_etag = record.etag.clone();
                    attempt.lease_owner = self.ids.generate_apply_lease_owner();
                    attempt.lease_expires_at_unix_ms = now
                        .checked_add(lease_duration_ms)
                        .ok_or(ChangeRuntimeError::TimestampOverflow)?;
                    attempt.update_time_unix_ms = now;
                    record.update_time_unix_ms = now;
                    match self.store.replace(&current_etag, record) {
                        Ok(acquired) => return Ok(ApplyAcquisition::Acquired(acquired)),
                        Err(ChangeStoreError::EtagMismatch { .. }) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                status => {
                    return Err(ChangeLedgerError::FailedPrecondition(format!(
                        "cannot apply a {status:?} change record"
                    )));
                }
            }
        }
        Err(ChangeLedgerError::FailedPrecondition(
            "apply lease changed repeatedly; retry the request".to_string(),
        ))
    }

    /// Finalize a correlated JJ write. Concurrent reconcilers may call this;
    /// the first CAS wins and later callers return the same applied record.
    pub fn complete_apply(
        &self,
        name: &str,
        request_id: &str,
        attempt_id: &str,
        result: ApplyResult,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        for _ in 0..4 {
            let mut record = self.get(name)?;
            if record.status == ChangeRecordStatus::Applied {
                let same_request = record
                    .apply_attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.request_id == request_id);
                if same_request {
                    return Ok(record);
                }
                return Err(ChangeLedgerError::FailedPrecondition(
                    "change was already applied by a different request id".to_string(),
                ));
            }
            require_status(&record, ChangeRecordStatus::Applying, "complete apply")?;
            let attempt = record.apply_attempt.as_mut().ok_or_else(|| {
                ChangeLedgerError::FailedPrecondition(
                    "applying change is missing its apply attempt".to_string(),
                )
            })?;
            if attempt.request_id != request_id || attempt.attempt_id != attempt_id {
                return Err(ChangeLedgerError::FailedPrecondition(
                    "apply correlation does not match the active attempt".to_string(),
                ));
            }
            if result.commit_id.is_empty()
                || result.change_id.is_empty()
                || result.operation_id.is_empty()
            {
                return Err(ChangeLedgerError::InvalidArgument(
                    "apply result must include commit, change, and operation ids".to_string(),
                ));
            }
            let current_etag = record.etag.clone();
            let now = self.clock.now_unix_millis()?;
            attempt.update_time_unix_ms = now;
            record.status = ChangeRecordStatus::Applied;
            record.apply_result = Some(result.clone());
            record.update_time_unix_ms = now;
            match self.store.replace(&current_etag, record) {
                Ok(applied) => return Ok(applied),
                Err(ChangeStoreError::EtagMismatch { .. }) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ChangeLedgerError::FailedPrecondition(
            "apply completion changed repeatedly; retry the request".to_string(),
        ))
    }

    /// Release an Apply lease after a policy callback rejected the exact final
    /// JJ tree before publication. Only the current lease owner may move the
    /// record back to Ready; operationally ambiguous failures deliberately
    /// remain Applying for correlation-based recovery.
    pub fn release_apply(
        &self,
        name: &str,
        request_id: &str,
        attempt_id: &str,
        lease_owner: &str,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        for _ in 0..4 {
            let mut record = self.get(name)?;
            require_status(&record, ChangeRecordStatus::Applying, "release apply")?;
            let attempt = record.apply_attempt.as_ref().ok_or_else(|| {
                ChangeLedgerError::FailedPrecondition(
                    "applying change is missing its apply attempt".to_string(),
                )
            })?;
            if attempt.request_id != request_id
                || attempt.attempt_id != attempt_id
                || attempt.lease_owner != lease_owner
            {
                return Err(ChangeLedgerError::FailedPrecondition(
                    "apply release does not match the active lease".to_string(),
                ));
            }
            let current_etag = record.etag.clone();
            let now = self.clock.now_unix_millis()?;
            record.status = ChangeRecordStatus::Ready;
            record.apply_attempt = None;
            record.update_time_unix_ms = now;
            match self.store.replace(&current_etag, record) {
                Ok(released) => return Ok(released),
                Err(ChangeStoreError::EtagMismatch { .. }) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ChangeLedgerError::FailedPrecondition(
            "apply release changed repeatedly; retry the request".to_string(),
        ))
    }

    pub fn approve(
        &self,
        name: &str,
        expected_etag: &str,
        reviewer: &Identity,
        reason: String,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        self.review(
            name,
            expected_etag,
            reviewer,
            ChangeReviewDecision::Approved,
            reason,
        )
    }

    pub fn reject(
        &self,
        name: &str,
        expected_etag: &str,
        reviewer: &Identity,
        reason: String,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        if reason.trim().is_empty() {
            return Err(ChangeLedgerError::InvalidArgument(
                "rejection reason must not be empty".to_string(),
            ));
        }
        self.review(
            name,
            expected_etag,
            reviewer,
            ChangeReviewDecision::Rejected,
            reason,
        )
    }

    pub fn abandon(
        &self,
        name: &str,
        expected_etag: &str,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        let mut record = self.get(name)?;
        if !matches!(
            record.status,
            ChangeRecordStatus::Draft | ChangeRecordStatus::Ready
        ) {
            return Err(ChangeLedgerError::FailedPrecondition(format!(
                "cannot abandon a {:?} change record",
                record.status
            )));
        }
        record.status = ChangeRecordStatus::Abandoned;
        record.update_time_unix_ms = self.clock.now_unix_millis()?;
        Ok(self.store.replace(expected_etag, record)?)
    }

    fn review(
        &self,
        name: &str,
        expected_etag: &str,
        reviewer: &Identity,
        decision: ChangeReviewDecision,
        reason: String,
    ) -> Result<ChangeRecord, ChangeLedgerError> {
        let mut record = self.get(name)?;
        require_status(&record, ChangeRecordStatus::Ready, "review")?;
        let actor = ChangeActor::from(reviewer);
        if record.created_by.identity == actor.identity {
            return Err(ChangeLedgerError::FailedPrecondition(
                "the change author cannot review their own change".to_string(),
            ));
        }
        if record
            .reviews
            .iter()
            .any(|review| review.reviewer.identity == actor.identity)
        {
            return Err(ChangeLedgerError::FailedPrecondition(format!(
                "identity {:?} has already reviewed this change",
                actor.identity
            )));
        }
        let now = self.clock.now_unix_millis()?;
        record.reviews.push(ChangeReview {
            reviewer: actor,
            decision,
            reason,
            create_time_unix_ms: now,
        });
        if decision == ChangeReviewDecision::Rejected {
            record.status = ChangeRecordStatus::Rejected;
        }
        record.update_time_unix_ms = now;
        Ok(self.store.replace(expected_etag, record)?)
    }
}

fn require_status(
    record: &ChangeRecord,
    expected: ChangeRecordStatus,
    operation: &str,
) -> Result<(), ChangeLedgerError> {
    if record.status != expected {
        return Err(ChangeLedgerError::FailedPrecondition(format!(
            "cannot {operation} a {:?} change record; expected {expected:?}",
            record.status
        )));
    }
    Ok(())
}

fn validate_segment(label: &str, value: &str) -> Result<(), ChangeLedgerError> {
    if value.is_empty() || value.contains('/') || value.chars().any(char::is_control) {
        return Err(ChangeLedgerError::InvalidArgument(format!(
            "{label} must be a non-empty resource path segment without control characters"
        )));
    }
    Ok(())
}

fn validate_nonempty(label: &str, value: &str) -> Result<(), ChangeLedgerError> {
    if value.trim().is_empty() {
        return Err(ChangeLedgerError::InvalidArgument(format!(
            "{label} must not be empty"
        )));
    }
    Ok(())
}

fn validate_title(title: &str) -> Result<(), ChangeLedgerError> {
    validate_nonempty("title", title)?;
    if title.chars().count() > 200 {
        return Err(ChangeLedgerError::InvalidArgument(
            "title must be at most 200 characters".to_string(),
        ));
    }
    Ok(())
}

fn normalize_external_references(
    references: Vec<String>,
) -> Result<Vec<String>, ChangeLedgerError> {
    if references.len() > 32 {
        return Err(ChangeLedgerError::InvalidArgument(
            "external_references must contain at most 32 entries".to_string(),
        ));
    }
    let mut normalized = Vec::with_capacity(references.len());
    let mut unique = HashSet::with_capacity(references.len());
    for reference in references {
        let reference = reference.trim();
        if reference.is_empty()
            || reference.len() > 2_048
            || reference.chars().any(char::is_control)
        {
            return Err(ChangeLedgerError::InvalidArgument(
                "each external reference must be 1-2048 bytes without control characters"
                    .to_string(),
            ));
        }
        if !unique.insert(reference.to_string()) {
            return Err(ChangeLedgerError::InvalidArgument(format!(
                "external_references contains duplicate entry {reference:?}"
            )));
        }
        normalized.push(reference.to_string());
    }
    Ok(normalized)
}

fn validate_change_id(id: &str) -> Result<(), ChangeLedgerError> {
    let valid = (1..=63).contains(&id.len())
        && id
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        return Err(ChangeLedgerError::InvalidArgument(
            "change_id must match [a-z][a-z0-9-]{0,62}".to_string(),
        ));
    }
    Ok(())
}

fn validate_request_id(id: &str) -> Result<(), ChangeLedgerError> {
    if id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return Err(ChangeLedgerError::InvalidArgument(
            "request_id must be 1-128 characters without control characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_resource_name(name: &str) -> Result<(), ChangeLedgerError> {
    parse_resource_name(name).map(|_| ())
}

fn parse_resource_name(name: &str) -> Result<Vec<&str>, ChangeLedgerError> {
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() != 6 || parts[0] != "projects" || parts[2] != "repos" || parts[4] != "changes" {
        return Err(ChangeLedgerError::InvalidArgument(
            "change name must be projects/{project}/repos/{repo}/changes/{change}".to_string(),
        ));
    }
    validate_segment("project", parts[1])?;
    validate_segment("repo", parts[3])?;
    validate_change_id(parts[5])?;
    Ok(parts)
}
