//! `schemahub change ...` — durable human/agent schema-change notes.

use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use prost_types::FieldMask;
use schemahub_api::schemahub_v1::{
    self as pb, change_service_client::ChangeServiceClient, AbandonChangeRequest,
    ApplyChangeRequest, ApproveChangeRequest, CreateChangeRequest, GetChangeRequest,
    ListChangesRequest, MarkChangeReadyRequest, RejectChangeRequest, UpdateChangeRequest,
    ValidateChangeRequest,
};
use serde_json::{json, Value};
use tonic::transport::Channel;

use crate::cmd::bearer;

#[derive(Args)]
pub struct ChangeArgs {
    /// Emit stable machine-readable JSON instead of human-oriented text.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub action: ChangeAction,
}

#[derive(Subcommand)]
pub enum ChangeAction {
    /// Record a note about a schema change before executable edits exist.
    Note {
        /// Repository in project/repo form.
        repo: String,
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        description: String,
        /// External issue, incident, design, or automation reference. Repeatable.
        #[arg(long = "reference")]
        external_reference: Vec<String>,
        #[arg(long, default_value = "main")]
        target_bookmark: String,
        #[arg(long, default_value = "")]
        base_revision: String,
        /// Optional deterministic resource id for automation correlation.
        #[arg(long, default_value = "")]
        id: String,
    },
    /// Get one change by its full resource name.
    Get {
        /// projects/{project}/repos/{repo}/changes/{change}
        name: String,
    },
    /// List changes for one repository.
    List {
        /// Repository in project/repo form.
        repo: String,
        /// Optional status: draft, ready, applying, applied, rejected, abandoned.
        #[arg(long, default_value = "")]
        status: String,
        #[arg(long, default_value_t = 50)]
        page_size: i32,
        #[arg(long, default_value = "")]
        page_token: String,
    },
    /// Patch selected fields on a draft using its current ETag.
    Update {
        /// Full change resource name.
        name: String,
        #[arg(long)]
        etag: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Replace external references with these values. Repeatable.
        #[arg(long = "reference", conflicts_with = "clear_references")]
        external_reference: Vec<String>,
        /// Remove every external reference from the draft.
        #[arg(long, conflicts_with = "external_reference")]
        clear_references: bool,
        #[arg(long)]
        target_bookmark: Option<String>,
        /// Set to an empty string to clear the base revision.
        #[arg(long)]
        base_revision: Option<String>,
    },
    /// Append a full-source replacement edit to a draft.
    AddSource {
        /// Full change resource name.
        name: String,
        #[arg(long)]
        etag: String,
        #[arg(long)]
        schema_path: String,
        /// Compiler format. Inferred from schema_path when omitted.
        #[arg(long, default_value = "")]
        format_id: String,
        /// UTF-8 schema source file.
        #[arg(long)]
        file: std::path::PathBuf,
    },
    /// Append a compiler-specific mutation envelope to a draft.
    AddMutation {
        /// Full change resource name.
        name: String,
        #[arg(long)]
        etag: String,
        #[arg(long)]
        schema_path: String,
        #[arg(long)]
        format_id: String,
        /// File containing the compiler's encoded operation bytes.
        #[arg(long)]
        operation_file: std::path::PathBuf,
    },
    /// Append an edit that deletes one schema file.
    DeleteSchema {
        /// Full change resource name.
        name: String,
        #[arg(long)]
        etag: String,
        #[arg(long)]
        schema_path: String,
        /// Compiler format. Inferred from schema_path when omitted.
        #[arg(long, default_value = "")]
        format_id: String,
    },
    /// Validate the draft's ordered edits against an immutable base.
    Validate {
        name: String,
        #[arg(long)]
        etag: String,
    },
    /// Promote a successfully validated draft to Ready.
    Ready {
        name: String,
        #[arg(long)]
        etag: String,
    },
    /// Append a maintainer approval.
    Approve {
        name: String,
        #[arg(long)]
        etag: String,
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Reject a ready change with a required reason.
    Reject {
        name: String,
        #[arg(long)]
        etag: String,
        #[arg(long)]
        reason: String,
    },
    /// Apply a ready change with a stable idempotency key.
    Apply {
        name: String,
        #[arg(long)]
        etag: String,
        /// Reuse this value for every retry of the same logical application.
        #[arg(long)]
        request_id: String,
    },
    /// Soft-delete a draft/ready record while retaining its audit history.
    Abandon {
        /// Full change resource name.
        name: String,
        #[arg(long)]
        etag: String,
    },
}

pub async fn run(args: ChangeArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    let mut client = ChangeServiceClient::new(channel);
    match args.action {
        ChangeAction::Note {
            repo,
            title,
            description,
            external_reference,
            target_bookmark,
            base_revision,
            id,
        } => {
            let (project, repo) = parse_repo(&repo)?;
            let change = client
                .create_change(bearer(
                    CreateChangeRequest {
                        parent: format!("projects/{project}/repos/{repo}"),
                        change: Some(pb::ChangeRecord {
                            target_bookmark,
                            base_revision,
                            title,
                            description,
                            external_references: external_reference,
                            ..Default::default()
                        }),
                        change_id: id,
                    },
                    token,
                )?)
                .await
                .context("CreateChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::Get { name } => {
            let change = client
                .get_change(bearer(GetChangeRequest { name }, token)?)
                .await
                .context("GetChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::List {
            repo,
            status,
            page_size,
            page_token,
        } => {
            let (project, repo) = parse_repo(&repo)?;
            let response = client
                .list_changes(bearer(
                    ListChangesRequest {
                        parent: format!("projects/{project}/repos/{repo}"),
                        page_size,
                        page_token,
                        status_filter: parse_status(&status)? as i32,
                    },
                    token,
                )?)
                .await
                .context("ListChanges RPC")?
                .into_inner();
            if args.json {
                let changes: Vec<_> = response.changes.iter().map(record_json).collect();
                print_json(&json!({
                    "changes": changes,
                    "next_page_token": response.next_page_token,
                }))?;
            } else {
                if response.changes.is_empty() {
                    println!("(no changes)");
                }
                for change in &response.changes {
                    print_record_text(change);
                }
                if !response.next_page_token.is_empty() {
                    println!("next page token: {}", response.next_page_token);
                }
            }
        }
        ChangeAction::Update {
            name,
            etag,
            title,
            description,
            external_reference,
            clear_references,
            target_bookmark,
            base_revision,
        } => {
            let mut change = pb::ChangeRecord {
                name,
                etag,
                ..Default::default()
            };
            let mut paths = Vec::new();
            if let Some(value) = title {
                change.title = value;
                paths.push("title".to_string());
            }
            if let Some(value) = description {
                change.description = value;
                paths.push("description".to_string());
            }
            if clear_references {
                change.external_references.clear();
                paths.push("external_references".to_string());
            } else if !external_reference.is_empty() {
                change.external_references = external_reference;
                paths.push("external_references".to_string());
            }
            if let Some(value) = target_bookmark {
                change.target_bookmark = value;
                paths.push("target_bookmark".to_string());
            }
            if let Some(value) = base_revision {
                change.base_revision = value;
                paths.push("base_revision".to_string());
            }
            if paths.is_empty() {
                bail!(
                    "change update requires at least one of --title, --description, \
                     --reference, --clear-references, --target-bookmark, or --base-revision"
                );
            }
            let change = client
                .update_change(bearer(
                    UpdateChangeRequest {
                        change: Some(change),
                        update_mask: Some(FieldMask { paths }),
                    },
                    token,
                )?)
                .await
                .context("UpdateChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::AddSource {
            name,
            etag,
            schema_path,
            format_id,
            file,
        } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("read schema source {}", file.display()))?;
            let format_id = resolve_format(&schema_path, &format_id)?;
            let edit = pb::ChangeEdit {
                edit: Some(pb::change_edit::Edit::ReplaceSource(
                    pb::ReplaceSchemaSourceEdit {
                        schema_path,
                        format_id,
                        source,
                    },
                )),
            };
            let change = append_edit(&mut client, name, etag, edit, token).await?;
            print_record(&change, args.json)?;
        }
        ChangeAction::AddMutation {
            name,
            etag,
            schema_path,
            format_id,
            operation_file,
        } => {
            if format_id.trim().is_empty() {
                bail!("--format-id must not be empty");
            }
            let operation = std::fs::read(&operation_file)
                .with_context(|| format!("read mutation operation {}", operation_file.display()))?;
            if operation.is_empty() {
                bail!("mutation operation file must not be empty");
            }
            let edit = pb::ChangeEdit {
                edit: Some(pb::change_edit::Edit::Mutation(pb::MutationChangeEdit {
                    schema_path,
                    format_id,
                    operation,
                })),
            };
            let change = append_edit(&mut client, name, etag, edit, token).await?;
            print_record(&change, args.json)?;
        }
        ChangeAction::DeleteSchema {
            name,
            etag,
            schema_path,
            format_id,
        } => {
            let format_id = resolve_format(&schema_path, &format_id)?;
            let edit = pb::ChangeEdit {
                edit: Some(pb::change_edit::Edit::DeleteSchema(pb::DeleteSchemaEdit {
                    schema_path,
                    format_id,
                })),
            };
            let change = append_edit(&mut client, name, etag, edit, token).await?;
            print_record(&change, args.json)?;
        }
        ChangeAction::Validate { name, etag } => {
            let change = client
                .validate_change(bearer(ValidateChangeRequest { name, etag }, token)?)
                .await
                .context("ValidateChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::Ready { name, etag } => {
            let change = client
                .mark_change_ready(bearer(MarkChangeReadyRequest { name, etag }, token)?)
                .await
                .context("MarkChangeReady RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::Approve { name, etag, reason } => {
            let change = client
                .approve_change(bearer(ApproveChangeRequest { name, etag, reason }, token)?)
                .await
                .context("ApproveChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::Reject { name, etag, reason } => {
            let change = client
                .reject_change(bearer(RejectChangeRequest { name, etag, reason }, token)?)
                .await
                .context("RejectChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::Apply {
            name,
            etag,
            request_id,
        } => {
            let change = client
                .apply_change(bearer(
                    ApplyChangeRequest {
                        name,
                        etag,
                        request_id,
                    },
                    token,
                )?)
                .await
                .context("ApplyChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
        ChangeAction::Abandon { name, etag } => {
            let change = client
                .abandon_change(bearer(AbandonChangeRequest { name, etag }, token)?)
                .await
                .context("AbandonChange RPC")?
                .into_inner();
            print_record(&change, args.json)?;
        }
    }
    Ok(())
}

async fn append_edit(
    client: &mut ChangeServiceClient<Channel>,
    name: String,
    expected_etag: String,
    edit: pb::ChangeEdit,
    token: &str,
) -> anyhow::Result<pb::ChangeRecord> {
    let mut change = client
        .get_change(bearer(GetChangeRequest { name: name.clone() }, token)?)
        .await
        .context("GetChange RPC before appending edit")?
        .into_inner();
    if change.etag != expected_etag {
        bail!(
            "change ETag is stale: expected {}, current {}",
            expected_etag,
            change.etag
        );
    }
    change.etag = expected_etag;
    change.edits.push(edit);
    client
        .update_change(bearer(
            UpdateChangeRequest {
                change: Some(change),
                update_mask: Some(FieldMask {
                    paths: vec!["edits".to_string()],
                }),
            },
            token,
        )?)
        .await
        .context("UpdateChange RPC while appending edit")
        .map(tonic::Response::into_inner)
}

fn resolve_format(schema_path: &str, requested: &str) -> anyhow::Result<String> {
    if !requested.trim().is_empty() {
        return Ok(requested.to_string());
    }
    let inferred = if schema_path.ends_with(".proto") {
        "protobuf"
    } else if schema_path.ends_with(".fbs") {
        "flatbuffers"
    } else if schema_path.ends_with(".yaml")
        || schema_path.ends_with(".yml")
        || schema_path.ends_with(".json")
    {
        "openapi"
    } else {
        bail!("cannot infer format from schema path {schema_path:?}; pass --format-id");
    };
    Ok(inferred.to_string())
}

fn parse_repo(value: &str) -> anyhow::Result<(String, String)> {
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("repo must be 'project/repo', got: {value}");
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

fn parse_status(value: &str) -> anyhow::Result<pb::ChangeStatus> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "" => pb::ChangeStatus::Unspecified,
        "draft" => pb::ChangeStatus::Draft,
        "ready" => pb::ChangeStatus::Ready,
        "applying" => pb::ChangeStatus::Applying,
        "applied" => pb::ChangeStatus::Applied,
        "rejected" => pb::ChangeStatus::Rejected,
        "abandoned" => pb::ChangeStatus::Abandoned,
        _ => bail!(
            "unknown change status {value:?}; expected draft, ready, applying, \
             applied, rejected, or abandoned"
        ),
    })
}

fn print_record(change: &pb::ChangeRecord, as_json: bool) -> anyhow::Result<()> {
    if as_json {
        print_json(&record_json(change))
    } else {
        print_record_text(change);
        Ok(())
    }
}

fn print_record_text(change: &pb::ChangeRecord) {
    let actor = change.created_by.as_ref();
    let actor_label = actor
        .map(|actor| {
            let kind = actor_kind_name(actor.kind);
            if actor.delegated_by.is_empty() {
                format!("{kind}:{}", actor.identity)
            } else {
                format!(
                    "{kind}:{} (delegated by {})",
                    actor.identity, actor.delegated_by
                )
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!(
        "{} [{}] {}",
        change.name,
        status_name(change.status),
        change.title
    );
    println!("  actor: {actor_label}");
    println!("  target: {}", change.target_bookmark);
    println!("  etag: {}", change.etag);
    if !change.description.is_empty() {
        println!("  {}", change.description);
    }
    for reference in &change.external_references {
        println!("  reference: {reference}");
    }
    if let Some(validation) = &change.validation {
        println!(
            "  validation: {} ({} issue(s), base {})",
            if validation.valid { "valid" } else { "invalid" },
            validation.issues.len(),
            validation.resolved_base_commit
        );
        for issue in &validation.issues {
            println!("    - {}: {}", issue.code, issue.message);
        }
    }
    if !change.reviews.is_empty() {
        println!("  reviews: {}", change.reviews.len());
    }
    if let Some(result) = &change.apply_result {
        println!("  commit: {}", result.commit_id);
        println!("  operation: {}", result.operation_id);
        if !result.conflicted_declarations.is_empty() {
            println!("  conflicts: {}", result.conflicted_declarations.join(", "));
        }
    }
}

fn print_json(value: &Value) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("encode change JSON")?
    );
    Ok(())
}

fn record_json(change: &pb::ChangeRecord) -> Value {
    json!({
        "name": change.name,
        "target_bookmark": change.target_bookmark,
        "base_revision": optional_string(&change.base_revision),
        "title": change.title,
        "description": change.description,
        "external_references": change.external_references,
        "edits": change.edits.iter().map(edit_json).collect::<Vec<_>>(),
        "created_by": change.created_by.as_ref().map(actor_json),
        "status": status_name(change.status),
        "validation": change.validation.as_ref().map(validation_json),
        "reviews": change.reviews.iter().map(review_json).collect::<Vec<_>>(),
        "apply_attempt": change.apply_attempt.as_ref().map(apply_attempt_json),
        "apply_result": change.apply_result.as_ref().map(apply_result_json),
        "etag": change.etag,
        "create_time": change.create_time.as_ref().map(timestamp_json),
        "update_time": change.update_time.as_ref().map(timestamp_json),
    })
}

fn apply_attempt_json(attempt: &pb::ChangeApplyAttempt) -> Value {
    json!({
        "request_id": attempt.request_id,
        "attempt_id": attempt.attempt_id,
        "actor": attempt.actor.as_ref().map(actor_json),
        "lease_owner": attempt.lease_owner,
        "lease_expires_at": attempt.lease_expires_at.as_ref().map(timestamp_json),
        "start_time": attempt.start_time.as_ref().map(timestamp_json),
        "update_time": attempt.update_time.as_ref().map(timestamp_json),
    })
}

fn actor_json(actor: &pb::Actor) -> Value {
    json!({
        "identity": actor.identity,
        "kind": actor_kind_name(actor.kind),
        "display_name": optional_string(&actor.display_name),
        "delegated_by": optional_string(&actor.delegated_by),
    })
}

fn edit_json(edit: &pb::ChangeEdit) -> Value {
    use pb::change_edit::Edit;
    match edit.edit.as_ref() {
        Some(Edit::Mutation(edit)) => json!({
            "kind": "mutation",
            "schema_path": edit.schema_path,
            "format_id": edit.format_id,
            "operation": edit.operation,
        }),
        Some(Edit::ReplaceSource(edit)) => json!({
            "kind": "replace_source",
            "schema_path": edit.schema_path,
            "format_id": edit.format_id,
            "source": edit.source,
        }),
        Some(Edit::DeleteSchema(edit)) => json!({
            "kind": "delete_schema",
            "schema_path": edit.schema_path,
            "format_id": edit.format_id,
        }),
        None => Value::Null,
    }
}

fn validation_json(validation: &pb::ChangeValidationResult) -> Value {
    json!({
        "valid": validation.valid,
        "resolved_base_commit": validation.resolved_base_commit,
        "edit_digest": validation.edit_digest,
        "issues": validation.issues.iter().map(|issue| json!({
            "code": issue.code,
            "message": issue.message,
            "schema_name": optional_string(&issue.schema_name),
            "declaration_name": optional_string(&issue.declaration_name),
        })).collect::<Vec<_>>(),
        "validated_at": validation.validated_at.as_ref().map(timestamp_json),
        "validator_version": validation.validator_version,
    })
}

fn review_json(review: &pb::ChangeReview) -> Value {
    json!({
        "reviewer": review.reviewer.as_ref().map(actor_json),
        "decision": match pb::ReviewDecision::try_from(review.decision) {
            Ok(pb::ReviewDecision::Approved) => "approved",
            Ok(pb::ReviewDecision::Rejected) => "rejected",
            Ok(pb::ReviewDecision::Unspecified) | Err(_) => "unspecified",
        },
        "reason": review.reason,
        "create_time": review.create_time.as_ref().map(timestamp_json),
    })
}

fn apply_result_json(result: &pb::ChangeApplyResult) -> Value {
    json!({
        "commit_id": result.commit_id,
        "change_id": result.change_id,
        "operation_id": result.operation_id,
        "conflicted_declarations": result.conflicted_declarations,
        "artifact_digest": optional_string(&result.artifact_digest),
    })
}

fn timestamp_json(timestamp: &prost_types::Timestamp) -> Value {
    json!({
        "seconds": timestamp.seconds,
        "nanos": timestamp.nanos,
    })
}

fn optional_string(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn actor_kind_name(value: i32) -> &'static str {
    match pb::ActorKind::try_from(value) {
        Ok(pb::ActorKind::Anonymous) => "anonymous",
        Ok(pb::ActorKind::Human) => "human",
        Ok(pb::ActorKind::Agent) => "agent",
        Ok(pb::ActorKind::Service) => "service",
        Ok(pb::ActorKind::Unspecified) | Err(_) => "unspecified",
    }
}

fn status_name(value: i32) -> &'static str {
    match pb::ChangeStatus::try_from(value) {
        Ok(pb::ChangeStatus::Draft) => "draft",
        Ok(pb::ChangeStatus::Ready) => "ready",
        Ok(pb::ChangeStatus::Applying) => "applying",
        Ok(pb::ChangeStatus::Applied) => "applied",
        Ok(pb::ChangeStatus::Rejected) => "rejected",
        Ok(pb::ChangeStatus::Abandoned) => "abandoned",
        Ok(pb::ChangeStatus::Unspecified) | Err(_) => "unspecified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_repo_requires_exact_project_and_repo_segments() {
        // Arrange
        let valid = "acme/commerce";

        // Act
        let parsed = parse_repo(valid);

        // Assert
        assert_eq!(
            parsed.unwrap(),
            ("acme".to_string(), "commerce".to_string())
        );
        assert!(parse_repo("acme").is_err());
        assert!(parse_repo("acme/commerce/extra").is_err());
    }

    #[test]
    fn record_json_exposes_agent_delegation_as_stable_strings() {
        // Arrange
        let change = pb::ChangeRecord {
            name: "projects/acme/repos/commerce/changes/change-1".to_string(),
            title: "Agent note".to_string(),
            external_references: vec!["INC-2048".to_string()],
            status: pb::ChangeStatus::Draft as i32,
            created_by: Some(pb::Actor {
                identity: "schema-agent".to_string(),
                kind: pb::ActorKind::Agent as i32,
                delegated_by: "alice".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        // Act
        let output = record_json(&change);

        // Assert
        assert_eq!(output["status"], "draft");
        assert_eq!(output["created_by"]["kind"], "agent");
        assert_eq!(output["created_by"]["delegated_by"], "alice");
        assert_eq!(output["external_references"], json!(["INC-2048"]));
    }

    #[test]
    fn resolve_format_infers_supported_schema_extensions() {
        // Arrange
        let schema_paths = ["order.proto", "model.fbs", "api.yaml", "api.json"];

        // Act
        let formats: Vec<_> = schema_paths
            .iter()
            .map(|path| resolve_format(path, "").expect("infer format"))
            .collect();

        // Assert
        assert_eq!(formats, ["protobuf", "flatbuffers", "openapi", "openapi"]);
        assert!(resolve_format("schema.unknown", "").is_err());
        assert_eq!(
            resolve_format("schema.custom", "protobuf").unwrap(),
            "protobuf"
        );
    }
}
