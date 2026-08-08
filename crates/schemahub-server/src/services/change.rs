//! `ChangeService` — durable human/agent schema-change intent and lifecycle.
//!
//! Actor and validation/review data always come from Core authentication and
//! compiler policy; output-only audit fields are rejected on Create.

use std::sync::Arc;

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::change_service_server::ChangeService;
use schemahub_core::change_record::{
    ApplyAttempt, ApplyResult, ChangeActor, ChangeEdit, ChangeRecord, ChangeRecordPageCursor,
    ChangeRecordStatus, ChangeReview, ChangeReviewDecision, ChangeUpdate, CreateChange,
    ValidationIssue, ValidationResult,
};
use schemahub_core::Core;
use schemahub_types::{Action, IdentityKind, SchemaPath};
use tonic::{Request, Response, Status};

use crate::error::to_status;
use crate::services::token_from;

const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 200;

pub struct ChangeHandler {
    core: Arc<Core>,
}

impl ChangeHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl ChangeService for ChangeHandler {
    async fn create_change(
        &self,
        request: Request<pb::CreateChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let (project, repo) = parse_parent(&request.parent)?;
        let change = request
            .change
            .ok_or_else(|| Status::invalid_argument("change must be provided"))?;
        reject_create_output_fields(&change)?;

        let target_bookmark = if change.target_bookmark.trim().is_empty() {
            self.core
                .repository_default_bookmark(project, repo, Action::Write, token.as_deref())
                .map_err(to_status)?
        } else {
            change.target_bookmark
        };
        let input = CreateChange {
            project: project.to_string(),
            repo: repo.to_string(),
            change_id: (!request.change_id.is_empty()).then_some(request.change_id),
            target_bookmark,
            base_revision: (!change.base_revision.is_empty()).then_some(change.base_revision),
            title: change.title,
            description: change.description,
            external_references: change.external_references,
            edits: edits_from_proto(project, repo, change.edits)?,
        };
        let created = self
            .core
            .create_change_record(input, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(created)))
    }

    async fn get_change(
        &self,
        request: Request<pb::GetChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let change = self
            .core
            .get_change_record(&request.name, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(change)))
    }

    async fn list_changes(
        &self,
        request: Request<pb::ListChangesRequest>,
    ) -> Result<Response<pb::ListChangesResponse>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let (project, repo) = parse_parent(&request.parent)?;
        let page_size = page_size(request.page_size)?;
        let status_filter = status_filter_from_proto(request.status_filter)?;
        let cursor = parse_page_token(&request.page_token, &request.parent, request.status_filter)?;

        let page = self
            .core
            .list_change_records_page(
                project,
                repo,
                status_filter,
                cursor.as_ref(),
                page_size,
                token.as_deref(),
            )
            .map_err(to_status)?;
        let next_page_token = page
            .next_cursor
            .as_ref()
            .map(|cursor| make_page_token(cursor, request.status_filter))
            .unwrap_or_default();

        Ok(Response::new(pb::ListChangesResponse {
            changes: page.records.into_iter().map(change_to_proto).collect(),
            next_page_token,
        }))
    }

    async fn update_change(
        &self,
        request: Request<pb::UpdateChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let change = request
            .change
            .ok_or_else(|| Status::invalid_argument("change must be provided"))?;
        if change.name.is_empty() {
            return Err(Status::invalid_argument("change.name must not be empty"));
        }
        if change.etag.is_empty() {
            return Err(Status::invalid_argument("change.etag must not be empty"));
        }
        let mask = request
            .update_mask
            .ok_or_else(|| Status::invalid_argument("update_mask must be provided"))?;
        let (project, repo) = parse_change_name(&change.name)?;
        let patch = patch_from_mask(project, repo, &change, &mask.paths)?;
        let updated = self
            .core
            .update_change_record(&change.name, &change.etag, patch, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(updated)))
    }

    async fn validate_change(
        &self,
        request: Request<pb::ValidateChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let validated = self
            .core
            .validate_change_record(&request.name, &request.etag, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(validated)))
    }

    async fn mark_change_ready(
        &self,
        request: Request<pb::MarkChangeReadyRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let ready = self
            .core
            .mark_change_ready(&request.name, &request.etag, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(ready)))
    }

    async fn approve_change(
        &self,
        request: Request<pb::ApproveChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let approved = self
            .core
            .approve_change_record(
                &request.name,
                &request.etag,
                request.reason,
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(approved)))
    }

    async fn reject_change(
        &self,
        request: Request<pb::RejectChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let rejected = self
            .core
            .reject_change_record(
                &request.name,
                &request.etag,
                request.reason,
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(rejected)))
    }

    async fn apply_change(
        &self,
        request: Request<pb::ApplyChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let applied = self
            .core
            .apply_change_record(
                &request.name,
                &request.etag,
                &request.request_id,
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(applied)))
    }

    async fn delete_change(
        &self,
        request: Request<pb::DeleteChangeRequest>,
    ) -> Result<Response<()>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        self.core
            .abandon_change_record(&request.name, &request.etag, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(()))
    }

    async fn abandon_change(
        &self,
        request: Request<pb::AbandonChangeRequest>,
    ) -> Result<Response<pb::ChangeRecord>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let abandoned = self
            .core
            .abandon_change_record(&request.name, &request.etag, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(change_to_proto(abandoned)))
    }
}

fn reject_create_output_fields(change: &pb::ChangeRecord) -> Result<(), Status> {
    if !change.name.is_empty()
        || change.created_by.is_some()
        || change.status != pb::ChangeStatus::Unspecified as i32
        || change.validation.is_some()
        || !change.reviews.is_empty()
        || change.apply_attempt.is_some()
        || change.apply_result.is_some()
        || !change.etag.is_empty()
        || change.create_time.is_some()
        || change.update_time.is_some()
    {
        return Err(Status::invalid_argument(
            "output-only change fields must not be set on create",
        ));
    }
    Ok(())
}

fn parse_parent(parent: &str) -> Result<(&str, &str), Status> {
    let parts: Vec<_> = parent.split('/').collect();
    if parts.len() != 4
        || parts[0] != "projects"
        || parts[2] != "repos"
        || parts[1].is_empty()
        || parts[3].is_empty()
    {
        return Err(Status::invalid_argument(
            "parent must be projects/{project}/repos/{repo}",
        ));
    }
    Ok((parts[1], parts[3]))
}

fn parse_change_name(name: &str) -> Result<(&str, &str), Status> {
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() != 6
        || parts[0] != "projects"
        || parts[2] != "repos"
        || parts[4] != "changes"
        || parts[1].is_empty()
        || parts[3].is_empty()
        || parts[5].is_empty()
    {
        return Err(Status::invalid_argument(
            "change.name must be projects/{project}/repos/{repo}/changes/{change}",
        ));
    }
    Ok((parts[1], parts[3]))
}

fn page_size(requested: i32) -> Result<usize, Status> {
    if requested < 0 {
        return Err(Status::invalid_argument("page_size must not be negative"));
    }
    Ok(if requested == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        (requested as usize).min(MAX_PAGE_SIZE)
    })
}

fn status_filter_from_proto(value: i32) -> Result<Option<ChangeRecordStatus>, Status> {
    let status = pb::ChangeStatus::try_from(value)
        .map_err(|_| Status::invalid_argument(format!("unknown status_filter value {value}")))?;
    Ok(match status {
        pb::ChangeStatus::Unspecified => None,
        pb::ChangeStatus::Draft => Some(ChangeRecordStatus::Draft),
        pb::ChangeStatus::Ready => Some(ChangeRecordStatus::Ready),
        pb::ChangeStatus::Applying => Some(ChangeRecordStatus::Applying),
        pb::ChangeStatus::Applied => Some(ChangeRecordStatus::Applied),
        pb::ChangeStatus::Rejected => Some(ChangeRecordStatus::Rejected),
        pb::ChangeStatus::Abandoned => Some(ChangeRecordStatus::Abandoned),
    })
}

fn parse_page_token(
    token: &str,
    parent: &str,
    status_filter: i32,
) -> Result<Option<ChangeRecordPageCursor>, Status> {
    if token.is_empty() {
        return Ok(None);
    }
    let mut parts = token.splitn(4, ':');
    let version = parts.next();
    let token_status = parts.next().and_then(|part| part.parse::<i32>().ok());
    let create_time = parts.next().and_then(|part| part.parse::<i64>().ok());
    let encoded_name = parts.next();
    let name = encoded_name
        .and_then(|value| hex::decode(value).ok())
        .and_then(|value| String::from_utf8(value).ok());
    let expected_prefix = format!("{parent}/changes/");
    if version != Some("v1")
        || token_status != Some(status_filter)
        || create_time.is_none_or(|value| value < 0)
        || name
            .as_deref()
            .and_then(|name| name.strip_prefix(&expected_prefix))
            .is_none_or(|change_id| {
                change_id.is_empty()
                    || change_id.contains('/')
                    || change_id.chars().any(char::is_control)
            })
    {
        return Err(Status::invalid_argument(
            "page_token is invalid for this parent or filter",
        ));
    }
    Ok(Some(ChangeRecordPageCursor {
        create_time_unix_ms: create_time.expect("checked above"),
        name: name.expect("checked above"),
    }))
}

fn make_page_token(cursor: &ChangeRecordPageCursor, status_filter: i32) -> String {
    format!(
        "v1:{status_filter}:{}:{}",
        cursor.create_time_unix_ms,
        hex::encode(cursor.name.as_bytes())
    )
}

fn patch_from_mask(
    project: &str,
    repo: &str,
    change: &pb::ChangeRecord,
    paths: &[String],
) -> Result<ChangeUpdate, Status> {
    let mut patch = ChangeUpdate::default();
    for path in paths {
        match path.as_str() {
            "target_bookmark" if patch.target_bookmark.is_none() => {
                patch.target_bookmark = Some(change.target_bookmark.clone());
            }
            "base_revision" if patch.base_revision.is_none() => {
                patch.base_revision = Some(change.base_revision.clone());
            }
            "title" if patch.title.is_none() => patch.title = Some(change.title.clone()),
            "description" if patch.description.is_none() => {
                patch.description = Some(change.description.clone());
            }
            "external_references" if patch.external_references.is_none() => {
                patch.external_references = Some(change.external_references.clone());
            }
            "edits" if patch.edits.is_none() => {
                patch.edits = Some(edits_from_proto(project, repo, change.edits.clone())?);
            }
            "target_bookmark"
            | "base_revision"
            | "title"
            | "description"
            | "external_references"
            | "edits" => {
                return Err(Status::invalid_argument(format!(
                    "update_mask contains duplicate path {path:?}"
                )));
            }
            _ => {
                return Err(Status::invalid_argument(format!(
                    "update_mask path {path:?} is not mutable"
                )));
            }
        }
    }
    Ok(patch)
}

fn edits_from_proto(
    project: &str,
    repo: &str,
    edits: Vec<pb::ChangeEdit>,
) -> Result<Vec<ChangeEdit>, Status> {
    edits
        .into_iter()
        .map(|edit| {
            use pb::change_edit::Edit;
            match edit.edit {
                Some(Edit::Mutation(edit)) => {
                    validate_edit_fields(&edit.schema_path, &edit.format_id)?;
                    if edit.operation.is_empty() {
                        return Err(Status::invalid_argument(
                            "mutation edit operation must not be empty",
                        ));
                    }
                    Ok(ChangeEdit::Mutation {
                        schema: SchemaPath::new(project, repo, edit.schema_path),
                        format_id: edit.format_id,
                        operation: edit.operation,
                    })
                }
                Some(Edit::ReplaceSource(edit)) => {
                    validate_edit_fields(&edit.schema_path, &edit.format_id)?;
                    Ok(ChangeEdit::ReplaceSource {
                        schema: SchemaPath::new(project, repo, edit.schema_path),
                        format_id: edit.format_id,
                        source: edit.source,
                    })
                }
                Some(Edit::DeleteSchema(edit)) => {
                    validate_edit_fields(&edit.schema_path, &edit.format_id)?;
                    Ok(ChangeEdit::DeleteSchema {
                        schema: SchemaPath::new(project, repo, edit.schema_path),
                        format_id: edit.format_id,
                    })
                }
                None => Err(Status::invalid_argument("change edit must select an edit")),
            }
        })
        .collect()
}

fn validate_edit_fields(schema_path: &str, format_id: &str) -> Result<(), Status> {
    if schema_path.trim().is_empty() {
        return Err(Status::invalid_argument(
            "change edit schema_path must not be empty",
        ));
    }
    if format_id.trim().is_empty() {
        return Err(Status::invalid_argument(
            "change edit format_id must not be empty",
        ));
    }
    Ok(())
}

fn change_to_proto(change: ChangeRecord) -> pb::ChangeRecord {
    pb::ChangeRecord {
        name: change.name,
        target_bookmark: change.target_bookmark,
        base_revision: change.base_revision.unwrap_or_default(),
        title: change.title,
        description: change.description,
        external_references: change.external_references,
        edits: change.edits.into_iter().map(edit_to_proto).collect(),
        created_by: Some(actor_to_proto(change.created_by)),
        status: status_to_proto(change.status) as i32,
        validation: change.validation.map(validation_to_proto),
        reviews: change.reviews.into_iter().map(review_to_proto).collect(),
        apply_attempt: change.apply_attempt.map(apply_attempt_to_proto),
        apply_result: change.apply_result.map(apply_result_to_proto),
        etag: change.etag,
        create_time: Some(timestamp_from_millis(change.create_time_unix_ms)),
        update_time: Some(timestamp_from_millis(change.update_time_unix_ms)),
    }
}

fn apply_attempt_to_proto(attempt: ApplyAttempt) -> pb::ChangeApplyAttempt {
    pb::ChangeApplyAttempt {
        request_id: attempt.request_id,
        attempt_id: attempt.attempt_id,
        actor: Some(actor_to_proto(attempt.actor)),
        lease_owner: attempt.lease_owner,
        lease_expires_at: Some(timestamp_from_millis(attempt.lease_expires_at_unix_ms)),
        start_time: Some(timestamp_from_millis(attempt.start_time_unix_ms)),
        update_time: Some(timestamp_from_millis(attempt.update_time_unix_ms)),
    }
}

fn edit_to_proto(edit: ChangeEdit) -> pb::ChangeEdit {
    use pb::change_edit::Edit;
    let edit = match edit {
        ChangeEdit::Mutation {
            schema,
            format_id,
            operation,
        } => Edit::Mutation(pb::MutationChangeEdit {
            schema_path: schema.schema_name,
            format_id,
            operation,
        }),
        ChangeEdit::ReplaceSource {
            schema,
            format_id,
            source,
        } => Edit::ReplaceSource(pb::ReplaceSchemaSourceEdit {
            schema_path: schema.schema_name,
            format_id,
            source,
        }),
        ChangeEdit::DeleteSchema { schema, format_id } => {
            Edit::DeleteSchema(pb::DeleteSchemaEdit {
                schema_path: schema.schema_name,
                format_id,
            })
        }
    };
    pb::ChangeEdit { edit: Some(edit) }
}

fn actor_to_proto(actor: ChangeActor) -> pb::Actor {
    pb::Actor {
        identity: actor.identity,
        kind: match actor.kind {
            IdentityKind::Anonymous => pb::ActorKind::Anonymous,
            IdentityKind::Human => pb::ActorKind::Human,
            IdentityKind::Agent => pb::ActorKind::Agent,
            IdentityKind::Service => pb::ActorKind::Service,
        } as i32,
        display_name: actor.display_name.unwrap_or_default(),
        delegated_by: actor.delegated_by.unwrap_or_default(),
    }
}

fn status_to_proto(status: ChangeRecordStatus) -> pb::ChangeStatus {
    match status {
        ChangeRecordStatus::Draft => pb::ChangeStatus::Draft,
        ChangeRecordStatus::Ready => pb::ChangeStatus::Ready,
        ChangeRecordStatus::Applying => pb::ChangeStatus::Applying,
        ChangeRecordStatus::Applied => pb::ChangeStatus::Applied,
        ChangeRecordStatus::Rejected => pb::ChangeStatus::Rejected,
        ChangeRecordStatus::Abandoned => pb::ChangeStatus::Abandoned,
    }
}

fn validation_to_proto(validation: ValidationResult) -> pb::ChangeValidationResult {
    pb::ChangeValidationResult {
        valid: validation.valid,
        resolved_base_commit: validation.resolved_base_commit,
        edit_digest: validation.edit_digest,
        issues: validation
            .issues
            .into_iter()
            .map(validation_issue_to_proto)
            .collect(),
        validated_at: Some(timestamp_from_millis(validation.validated_at_unix_ms)),
        validator_version: validation.validator_version,
    }
}

fn validation_issue_to_proto(issue: ValidationIssue) -> pb::ValidationIssue {
    pb::ValidationIssue {
        code: issue.code,
        message: issue.message,
        schema_name: issue.schema_name.unwrap_or_default(),
        declaration_name: issue.declaration_name.unwrap_or_default(),
    }
}

fn review_to_proto(review: ChangeReview) -> pb::ChangeReview {
    pb::ChangeReview {
        reviewer: Some(actor_to_proto(review.reviewer)),
        decision: match review.decision {
            ChangeReviewDecision::Approved => pb::ReviewDecision::Approved,
            ChangeReviewDecision::Rejected => pb::ReviewDecision::Rejected,
        } as i32,
        reason: review.reason,
        create_time: Some(timestamp_from_millis(review.create_time_unix_ms)),
    }
}

fn apply_result_to_proto(result: ApplyResult) -> pb::ChangeApplyResult {
    pb::ChangeApplyResult {
        commit_id: result.commit_id,
        change_id: result.change_id,
        operation_id: result.operation_id,
        conflicted_declarations: result.conflicted_declarations,
        artifact_digest: result.artifact_digest.unwrap_or_default(),
    }
}

fn timestamp_from_millis(millis: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: millis.div_euclid(1_000),
        nanos: (millis.rem_euclid(1_000) * 1_000_000) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor() -> ChangeRecordPageCursor {
        ChangeRecordPageCursor {
            create_time_unix_ms: 1_000,
            name: "projects/acme/repos/commerce/changes/change-a".to_string(),
        }
    }

    #[test]
    fn change_page_token_round_trips_its_parent_filter_and_cursor() {
        // Arrange
        let cursor = cursor();
        let status = pb::ChangeStatus::Draft as i32;
        let token = make_page_token(&cursor, status);

        // Act
        let parsed = parse_page_token(&token, "projects/acme/repos/commerce", status);

        // Assert
        assert_eq!(parsed.unwrap(), Some(cursor));
    }

    #[test]
    fn change_page_token_cannot_cross_parent() {
        // Arrange
        let status = pb::ChangeStatus::Draft as i32;
        let token = make_page_token(&cursor(), status);

        // Act
        let result = parse_page_token(&token, "projects/acme/repos/other", status);

        // Assert
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn change_page_token_cannot_cross_status_filter() {
        // Arrange
        let token = make_page_token(&cursor(), pb::ChangeStatus::Draft as i32);

        // Act
        let result = parse_page_token(
            &token,
            "projects/acme/repos/commerce",
            pb::ChangeStatus::Ready as i32,
        );

        // Assert
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn change_page_token_rejects_negative_time() {
        // Arrange
        let status = pb::ChangeStatus::Draft as i32;
        let negative = format!(
            "v1:{status}:-1:{}",
            hex::encode("projects/acme/repos/commerce/changes/change-a")
        );

        // Act
        let result = parse_page_token(&negative, "projects/acme/repos/commerce", status);

        // Assert
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn change_page_token_rejects_nested_change_name() {
        // Arrange
        let status = pb::ChangeStatus::Draft as i32;
        let nested = format!(
            "v1:{status}:1000:{}",
            hex::encode("projects/acme/repos/commerce/changes/change-a/child")
        );

        // Act
        let result = parse_page_token(&nested, "projects/acme/repos/commerce", status);

        // Assert
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }
}
