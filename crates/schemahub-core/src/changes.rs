//! Authorized orchestration for durable schema change records.
//!
//! The ledger owns lifecycle invariants and persistence. These wrappers keep
//! authentication and repository authorization in Core so gRPC, HTTP, CLI,
//! and future agent integrations cannot accidentally create a second policy
//! path.

use std::collections::{BTreeMap, BTreeSet};

use schemahub_jj::{PublicationError, RefSpec, SchemaWrite};
use schemahub_types::{Action, Identity, IdentityKind};

use crate::change_record::validation::{PreparedChange, PreparedSchemaChange, ValidationOutcome};
use crate::change_record::{
    ApplyAcquisition, ApplyResult, ChangeLedger, ChangeLedgerError, ChangeRecord,
    ChangeReviewDecision, ChangeUpdate, CreateChange,
};
use crate::{Core, CoreResult};

const APPLY_LEASE_DURATION_MS: i64 = 5 * 60 * 1_000;
const CHANGE_RECORD_ATTRIBUTE: &str = "schemahub.change_record";
const APPLY_ATTEMPT_ATTRIBUTE: &str = "schemahub.apply_attempt";
const APPLY_REQUEST_ATTRIBUTE: &str = "schemahub.apply_request";

impl Core {
    /// Record human or agent schema-change intent as a draft. Actor metadata is
    /// derived from the authenticated identity and never accepted in `input`.
    pub fn create_change_record(
        &self,
        input: CreateChange,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        ChangeLedger::validate_scope(&input.project, &input.repo)?;
        let identity = self.authorize_repo_action(
            token,
            Action::Write,
            input.project.as_str(),
            input.repo.as_str(),
        )?;
        self.effective_repo_config(&input.project, &input.repo)?;
        let record = self.change_ledger.create(input, &identity)?;
        log_change_event("schemahub.change.created", &record, &identity, None);
        Ok(record)
    }

    /// Read one change record after authorizing against its resource-name
    /// scope. The name is parsed before storage access to avoid existence
    /// leaks across private projects.
    pub fn get_change_record(&self, name: &str, token: Option<&str>) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        self.authorize_repo_action(token, Action::Read, project, repo)?;
        Ok(self.change_ledger.get(name)?)
    }

    /// List change records in stable creation order for an authorized repo.
    pub fn list_change_records(
        &self,
        project: &str,
        repo: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<ChangeRecord>> {
        ChangeLedger::validate_scope(project, repo)?;
        self.authorize_repo_action(token, Action::Read, project, repo)?;
        Ok(self.change_ledger.list(project, repo)?)
    }

    /// Patch mutable draft fields using optimistic concurrency.
    pub fn update_change_record(
        &self,
        name: &str,
        expected_etag: &str,
        patch: ChangeUpdate,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        let identity = self.authorize_repo_action(token, Action::Write, project, repo)?;
        self.effective_repo_config(project, repo)?;
        let record = self.change_ledger.update(name, expected_etag, patch)?;
        log_change_event("schemahub.change.updated", &record, &identity, None);
        Ok(record)
    }

    /// Rebuild and persist a deterministic validation snapshot from the
    /// record's current ordered edits and immutable JJ base. Validation
    /// findings are data on the returned record, not forged client input.
    pub fn validate_change_record(
        &self,
        name: &str,
        expected_etag: &str,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        let identity = self.authorize_repo_action(token, Action::Write, project, repo)?;
        self.effective_repo_config(project, repo)?;
        let record = self.change_ledger.get_at_etag(name, expected_etag)?;
        let outcome = crate::change_record::validation::validate(self, &record)?;
        debug_assert_eq!(outcome.result.valid, outcome.prepared.is_some());
        debug_assert!(outcome
            .prepared
            .as_ref()
            .is_none_or(|prepared| prepared.matches(&outcome.result)));
        let record = self
            .change_ledger
            .record_validation(name, expected_etag, outcome.result)?;
        log_change_event("schemahub.change.validated", &record, &identity, None);
        Ok(record)
    }

    /// Promote a validated executable draft to Ready.
    pub fn mark_change_ready(
        &self,
        name: &str,
        expected_etag: &str,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        let identity = self.authorize_repo_action(token, Action::Write, project, repo)?;
        self.effective_repo_config(project, repo)?;
        let record = self.change_ledger.mark_ready(name, expected_etag)?;
        log_change_event("schemahub.change.ready", &record, &identity, None);
        Ok(record)
    }

    /// Append a maintainer approval. Reviewer identity is derived from the
    /// authenticated request and cannot be supplied by the client.
    pub fn approve_change_record(
        &self,
        name: &str,
        expected_etag: &str,
        reason: String,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        let reviewer = self.authorize_repo_action(token, Action::ManageRepo, project, repo)?;
        self.effective_repo_config(project, repo)?;
        let record = self
            .change_ledger
            .approve(name, expected_etag, &reviewer, reason)?;
        log_change_event("schemahub.change.approved", &record, &reviewer, None);
        Ok(record)
    }

    /// Append a maintainer rejection and terminate the proposed change.
    pub fn reject_change_record(
        &self,
        name: &str,
        expected_etag: &str,
        reason: String,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        let reviewer = self.authorize_repo_action(token, Action::ManageRepo, project, repo)?;
        self.effective_repo_config(project, repo)?;
        let record = self
            .change_ledger
            .reject(name, expected_etag, &reviewer, reason)?;
        log_change_event("schemahub.change.rejected", &record, &reviewer, None);
        Ok(record)
    }

    /// Apply a Ready record exactly once from the caller's perspective. A
    /// durable lease serializes writers and JJ operation attributes let a
    /// retry recover a commit that was published before the record receipt.
    pub fn apply_change_record(
        &self,
        name: &str,
        expected_etag: &str,
        request_id: &str,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        let identity = self.authorize_repo_action(token, Action::Write, project, repo)?;
        let config = self.effective_repo_config(project, repo)?;

        // Fail before entering APPLYING if current policy/compiler output no
        // longer matches the stored Ready snapshot.
        let before = self.change_ledger.get(name)?;
        if before.status == crate::change_record::ChangeRecordStatus::Ready {
            let required = config.review_policy.required_approvals as usize;
            let approvals: BTreeSet<_> = before
                .reviews
                .iter()
                .filter(|review| review.decision == ChangeReviewDecision::Approved)
                .map(|review| review.reviewer.identity.as_str())
                .collect();
            if approvals.len() < required {
                return Err(ChangeLedgerError::FailedPrecondition(format!(
                    "repository policy requires {required} approval(s); change has {}",
                    approvals.len()
                ))
                .into());
            }
            let outcome = crate::change_record::validation::validate(self, &before)?;
            require_current_plan(&before, &outcome)?;
        }

        let acquisition = self.change_ledger.acquire_apply(
            name,
            expected_etag,
            request_id,
            &identity,
            APPLY_LEASE_DURATION_MS,
        )?;
        let (record, owns_lease) = match acquisition {
            ApplyAcquisition::AlreadyApplied(record) => {
                log_change_event(
                    "schemahub.change.apply_replayed",
                    &record,
                    &identity,
                    Some(request_id),
                );
                return Ok(record);
            }
            ApplyAcquisition::Observed(record) => (record, false),
            ApplyAcquisition::Acquired(record) => (record, true),
        };
        let attempt = record.apply_attempt.clone().ok_or_else(|| {
            ChangeLedgerError::FailedPrecondition(
                "applying change is missing correlation metadata".to_string(),
            )
        })?;
        let attributes = apply_attributes(name, request_id, &attempt.attempt_id);

        // Reconciliation is safe for both the lease owner and observers.
        if let Some(write) = self.jj.find_correlated_write(
            &record.project,
            &record.repo,
            &record.target_bookmark,
            &attributes,
        )? {
            let completed = self.change_ledger.complete_apply(
                name,
                request_id,
                &attempt.attempt_id,
                apply_result(write),
            )?;
            log_change_event(
                "schemahub.change.apply_reconciled",
                &completed,
                &identity,
                Some(request_id),
            );
            return Ok(completed);
        }
        if !owns_lease {
            log_change_event(
                "schemahub.change.apply_observed",
                &record,
                &identity,
                Some(request_id),
            );
            return Ok(record);
        }

        let outcome = crate::change_record::validation::validate(self, &record)?;
        let prepared = require_current_plan(&record, &outcome)?;
        let writes = prepared
            .writes
            .into_iter()
            .map(|write| match write {
                PreparedSchemaChange::Patch {
                    schema_name,
                    effect,
                } => SchemaWrite::Patch {
                    schema_path: schema_name,
                    effect,
                },
                PreparedSchemaChange::Delete { schema_name } => SchemaWrite::Delete {
                    schema_path: schema_name,
                },
            })
            .collect();
        let author = identity.id().unwrap_or("anonymous");
        let protected = schemahub_jj::bookmark::is_protected(
            &record.target_bookmark,
            &config.protected_bookmarks,
        );
        let write = match self.jj.commit_schema_changes_validated(
            &record.project,
            &record.repo,
            &record.target_bookmark,
            &RefSpec::commit(prepared.resolved_base_commit),
            writes,
            author,
            &record.title,
            attributes,
            |snapshot| {
                self.validate_publication_snapshot(
                    &record.project,
                    &record.repo,
                    &record.target_bookmark,
                    protected,
                    snapshot,
                )
            },
        ) {
            Ok(write) => write,
            Err(PublicationError::Jj(error)) => return Err(error.into()),
            Err(PublicationError::Rejected(error)) => {
                let released = self.change_ledger.release_apply(
                    name,
                    request_id,
                    &attempt.attempt_id,
                    &attempt.lease_owner,
                )?;
                log_change_event(
                    "schemahub.change.apply_policy_rejected",
                    &released,
                    &identity,
                    Some(request_id),
                );
                return Err(error);
            }
        };
        let completed = self.change_ledger.complete_apply(
            name,
            request_id,
            &attempt.attempt_id,
            apply_result(write),
        )?;
        log_change_event(
            "schemahub.change.applied",
            &completed,
            &identity,
            Some(request_id),
        );
        Ok(completed)
    }

    /// Soft-delete a draft/ready change. Authors with Writer access may
    /// abandon their own records; abandoning another actor's record requires
    /// repository-maintainer access.
    pub fn abandon_change_record(
        &self,
        name: &str,
        expected_etag: &str,
        token: Option<&str>,
    ) -> CoreResult<ChangeRecord> {
        let (project, repo) = ChangeLedger::scope(name)?;
        let identity = self.authorize_repo_action(token, Action::Write, project, repo)?;
        self.effective_repo_config(project, repo)?;
        let existing = self.change_ledger.get(name)?;
        if existing.created_by.identity != identity.id().unwrap_or_default() {
            self.authorize_repo_action(token, Action::ManageRepo, project, repo)?;
        }
        let record = self.change_ledger.abandon(name, expected_etag)?;
        log_change_event("schemahub.change.abandoned", &record, &identity, None);
        Ok(record)
    }

    /// Read-only access for composition/testing. Product callers should use
    /// the authorized methods above.
    pub fn change_ledger(&self) -> &ChangeLedger {
        &self.change_ledger
    }
}

fn log_change_event(
    event: &'static str,
    record: &ChangeRecord,
    identity: &Identity,
    request_id: Option<&str>,
) {
    tracing::info!(
        event,
        change_name = record.name,
        project = record.project,
        repo = record.repo,
        target_bookmark = record.target_bookmark,
        status = ?record.status,
        etag = record.etag,
        actor_id = identity.id().unwrap_or("anonymous"),
        actor_kind = identity_kind_name(identity.kind()),
        delegated_by = identity.delegated_by().unwrap_or(""),
        request_id = request_id.unwrap_or(""),
        edit_count = record.edits.len(),
        review_count = record.reviews.len(),
        validation_valid = record.validation.as_ref().map(|result| result.valid),
        commit_id = record
            .apply_result
            .as_ref()
            .map(|result| result.commit_id.as_str())
            .unwrap_or(""),
        operation_id = record
            .apply_result
            .as_ref()
            .map(|result| result.operation_id.as_str())
            .unwrap_or(""),
        "change lifecycle transition"
    );
}

fn identity_kind_name(kind: IdentityKind) -> &'static str {
    match kind {
        IdentityKind::Anonymous => "anonymous",
        IdentityKind::Human => "human",
        IdentityKind::Agent => "agent",
        IdentityKind::Service => "service",
    }
}

fn require_current_plan(
    record: &ChangeRecord,
    outcome: &ValidationOutcome,
) -> CoreResult<PreparedChange> {
    let stored = record.validation.as_ref().ok_or_else(|| {
        ChangeLedgerError::FailedPrecondition(
            "change does not have a stored validation snapshot".to_string(),
        )
    })?;
    let prepared = outcome.prepared.clone().ok_or_else(|| {
        ChangeLedgerError::FailedPrecondition(
            "change no longer passes current compiler and repository policy".to_string(),
        )
    })?;
    if !stored.valid
        || stored.resolved_base_commit != outcome.result.resolved_base_commit
        || stored.edit_digest != outcome.result.edit_digest
        || stored.validator_version != outcome.result.validator_version
        || !prepared.matches(&outcome.result)
    {
        return Err(ChangeLedgerError::FailedPrecondition(
            "stored validation snapshot is stale; validate the draft again".to_string(),
        )
        .into());
    }
    Ok(prepared)
}

fn apply_attributes(name: &str, request_id: &str, attempt_id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (CHANGE_RECORD_ATTRIBUTE.to_string(), name.to_string()),
        (APPLY_ATTEMPT_ATTRIBUTE.to_string(), attempt_id.to_string()),
        (APPLY_REQUEST_ATTRIBUTE.to_string(), request_id.to_string()),
    ])
}

fn apply_result(write: schemahub_jj::WriteResult) -> ApplyResult {
    ApplyResult {
        commit_id: write.commit_id,
        change_id: write.change_id,
        operation_id: write.operation_id,
        conflicted_declarations: write.conflicted_decls,
        artifact_digest: None,
    }
}
