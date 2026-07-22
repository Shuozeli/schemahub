//! End-to-end coverage for the first durable ChangeRecord API slice.
//!
//! The tests exercise the same tonic router and authentication configuration
//! as a deployed server. Every case follows Arrange-Act-Assert.

mod common;

use std::collections::HashMap;
use std::path::Path;

use common::*;
use prost_types::FieldMask;
use schemahub_api::schemahub_v1 as pb;
use schemahub_server::config::{AuthConfig, Config, ProjectSection, TokenIdentity};
use schemahub_types::IdentityKind;
use tonic::metadata::MetadataValue;
use tonic::Request;

fn config_with_change_actors(data_dir: &Path) -> Config {
    Config {
        auth: AuthConfig {
            data_dir: data_dir.to_string_lossy().to_string(),
            tokens: HashMap::from([
                (
                    "human-token".to_string(),
                    TokenIdentity {
                        id: "alice".to_string(),
                        display: Some("Alice".to_string()),
                        kind: IdentityKind::Human,
                        delegated_by: None,
                    },
                ),
                (
                    "agent-token".to_string(),
                    TokenIdentity {
                        id: "schema-agent".to_string(),
                        display: Some("Schema Agent".to_string()),
                        kind: IdentityKind::Agent,
                        delegated_by: Some("alice".to_string()),
                    },
                ),
                (
                    "reader-token".to_string(),
                    TokenIdentity {
                        id: "ron".to_string(),
                        display: Some("Ron".to_string()),
                        kind: IdentityKind::Human,
                        delegated_by: None,
                    },
                ),
            ]),
            jwt: None,
        },
        projects: HashMap::from([(
            "acme".to_string(),
            ProjectSection {
                visibility: Some("private".to_string()),
                owners: vec!["alice".to_string()],
                members: HashMap::from([
                    ("schema-agent".to_string(), "writer".to_string()),
                    ("ron".to_string(), "reader".to_string()),
                ]),
            },
        )]),
        ..Default::default()
    }
}

fn with_token<T>(mut request: Request<T>, token: &str) -> Request<T> {
    let value: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    request.metadata_mut().insert("authorization", value);
    request
}

fn note(title: &str, description: &str) -> pb::ChangeRecord {
    pb::ChangeRecord {
        target_bookmark: "main".to_string(),
        title: title.to_string(),
        description: description.to_string(),
        external_references: vec!["TEST-CHANGE".to_string()],
        ..Default::default()
    }
}

async fn create_note(
    client: &mut pb::change_service_client::ChangeServiceClient<tonic::transport::Channel>,
    token: &str,
    change_id: &str,
    title: &str,
) -> pb::ChangeRecord {
    client
        .create_change(with_token(
            Request::new(pb::CreateChangeRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                change: Some(note(title, "Captured before implementation")),
                change_id: change_id.to_string(),
            }),
            token,
        ))
        .await
        .expect("create change note")
        .into_inner()
}

struct ChangeToArtifactOutcome {
    created: pb::ChangeRecord,
    approved: pb::ChangeRecord,
    applied: pb::ChangeRecord,
    descriptors: pb::SchemaArtifact,
    generated: pb::SchemaArtifact,
}

async fn apply_agent_change_and_fetch_artifacts(
    clients: &mut Clients,
    change_id: &str,
    schema_path: &str,
    format_id: &str,
    source: &str,
) -> ChangeToArtifactOutcome {
    let created = clients
        .change
        .create_change(with_token(
            Request::new(pb::CreateChangeRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                change: Some(pb::ChangeRecord {
                    target_bookmark: "main".to_string(),
                    title: format!("Add {schema_path}"),
                    description: "Proposed by the delegated schema agent".to_string(),
                    external_references: vec![format!("GA-{change_id}")],
                    edits: vec![pb::ChangeEdit {
                        edit: Some(pb::change_edit::Edit::ReplaceSource(
                            pb::ReplaceSchemaSourceEdit {
                                schema_path: schema_path.to_string(),
                                format_id: format_id.to_string(),
                                source: source.to_string(),
                            },
                        )),
                    }],
                    ..Default::default()
                }),
                change_id: change_id.to_string(),
            }),
            "agent-token",
        ))
        .await
        .expect("agent creates executable change")
        .into_inner();
    let validated = clients
        .change
        .validate_change(with_token(
            Request::new(pb::ValidateChangeRequest {
                name: created.name.clone(),
                etag: created.etag.clone(),
            }),
            "agent-token",
        ))
        .await
        .expect("compiler validates agent change")
        .into_inner();
    let ready = clients
        .change
        .mark_change_ready(with_token(
            Request::new(pb::MarkChangeReadyRequest {
                name: validated.name,
                etag: validated.etag,
            }),
            "agent-token",
        ))
        .await
        .expect("agent marks validated change ready")
        .into_inner();
    let approved = clients
        .change
        .approve_change(with_token(
            Request::new(pb::ApproveChangeRequest {
                name: ready.name,
                etag: ready.etag,
                reason: "Human reviewed the compiler result".to_string(),
            }),
            "human-token",
        ))
        .await
        .expect("human approves agent change")
        .into_inner();
    let applied = clients
        .change
        .apply_change(with_token(
            Request::new(pb::ApplyChangeRequest {
                name: approved.name.clone(),
                etag: approved.etag.clone(),
                request_id: format!("apply-{change_id}"),
            }),
            "agent-token",
        ))
        .await
        .expect("agent applies approved change")
        .into_inner();
    let commit_id = applied
        .apply_result
        .as_ref()
        .expect("applied change has a receipt")
        .commit_id
        .clone();
    let revision = format!("projects/acme/repos/commerce/revisions/{commit_id}");
    let descriptors = clients
        .serving
        .get_schema_artifact(with_token(
            Request::new(pb::GetSchemaArtifactRequest {
                revision: revision.clone(),
                schema_path: schema_path.to_string(),
                kind: pb::SchemaArtifactKind::Descriptors as i32,
                ..Default::default()
            }),
            "agent-token",
        ))
        .await
        .expect("consumer fetches immutable descriptors")
        .into_inner();
    let generated = clients
        .serving
        .get_schema_artifact(with_token(
            Request::new(pb::GetSchemaArtifactRequest {
                revision,
                schema_path: schema_path.to_string(),
                kind: pb::SchemaArtifactKind::GeneratedCode as i32,
                language: pb::Language::Rust as i32,
                ..Default::default()
            }),
            "agent-token",
        ))
        .await
        .expect("consumer fetches immutable generated code")
        .into_inner();

    ChangeToArtifactOutcome {
        created,
        approved,
        applied,
        descriptors,
        generated,
    }
}

#[tokio::test]
async fn human_and_agent_notes_preserve_server_derived_actor_metadata() {
    // Arrange
    let auth_dir = tempfile::tempdir().unwrap();
    let url = start_server_with(config_with_change_actors(auth_dir.path())).await;
    let mut clients = clients(&url).await;

    // Act
    let human = create_note(
        &mut clients.change,
        "human-token",
        "human-note",
        "Add settlement currency",
    )
    .await;
    let agent = create_note(
        &mut clients.change,
        "agent-token",
        "agent-note",
        "Observed nullable identifier drift",
    )
    .await;

    // Assert
    let human_actor = human.created_by.expect("human actor");
    assert_eq!(human_actor.identity, "alice");
    assert_eq!(human_actor.kind, pb::ActorKind::Human as i32);
    assert_eq!(human_actor.display_name, "Alice");

    let agent_actor = agent.created_by.expect("agent actor");
    assert_eq!(agent_actor.identity, "schema-agent");
    assert_eq!(agent_actor.kind, pb::ActorKind::Agent as i32);
    assert_eq!(agent_actor.display_name, "Schema Agent");
    assert_eq!(agent_actor.delegated_by, "alice");
    assert_eq!(agent.status, pb::ChangeStatus::Draft as i32);
    assert_eq!(agent.external_references, ["TEST-CHANGE"]);
    assert_eq!(agent.etag, "v1");
    assert!(agent.create_time.is_some());

    // Act: page through the repo ledger using a stable cursor.
    let first_page = clients
        .change
        .list_changes(with_token(
            Request::new(pb::ListChangesRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                page_size: 1,
                page_token: String::new(),
                status_filter: pb::ChangeStatus::Draft as i32,
            }),
            "human-token",
        ))
        .await
        .expect("first page")
        .into_inner();
    let second_page = clients
        .change
        .list_changes(with_token(
            Request::new(pb::ListChangesRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                page_size: 1,
                page_token: first_page.next_page_token.clone(),
                status_filter: pb::ChangeStatus::Draft as i32,
            }),
            "human-token",
        ))
        .await
        .expect("second page")
        .into_inner();

    // Assert
    assert_eq!(first_page.changes.len(), 1);
    assert!(!first_page.next_page_token.is_empty());
    assert_eq!(second_page.changes.len(), 1);
    assert!(second_page.next_page_token.is_empty());
    assert_ne!(first_page.changes[0].name, second_page.changes[0].name);
}

#[tokio::test]
async fn human_reviewed_agent_changes_serve_artifacts_for_both_native_compilers() {
    // Arrange
    let auth_dir = tempfile::tempdir().unwrap();
    let url = start_server_with(config_with_change_actors(auth_dir.path())).await;
    let mut clients = clients(&url).await;
    let cases = [
        (
            "ga-protobuf",
            "ga_order.proto",
            "protobuf",
            pb::SchemaFormat::Protobuf,
            "OrderRecord",
            r#"syntax = "proto3";
package ga.acceptance;
message OrderRecord { string id = 1; }
"#,
        ),
        (
            "ga-flatbuffers",
            "ga_event.fbs",
            "flatbuffers",
            pb::SchemaFormat::Flatbuffers,
            "EventRecord",
            r#"namespace ga.acceptance;

table EventRecord {
  id: string;
}

root_type EventRecord;
"#,
        ),
    ];

    // Act
    let mut outcomes = Vec::new();
    for (change_id, schema_path, format_id, format, generated_symbol, source) in cases {
        let outcome = apply_agent_change_and_fetch_artifacts(
            &mut clients,
            change_id,
            schema_path,
            format_id,
            source,
        )
        .await;
        outcomes.push((format, generated_symbol, outcome));
    }

    // Assert
    for (format, generated_symbol, outcome) in outcomes {
        let author = outcome.created.created_by.expect("server-derived author");
        assert_eq!(author.identity, "schema-agent");
        assert_eq!(author.kind, pb::ActorKind::Agent as i32);
        assert_eq!(author.delegated_by, "alice");

        assert_eq!(outcome.approved.reviews.len(), 1);
        let reviewer = outcome.approved.reviews[0]
            .reviewer
            .as_ref()
            .expect("server-derived reviewer");
        assert_eq!(reviewer.identity, "alice");
        assert_eq!(reviewer.kind, pb::ActorKind::Human as i32);

        assert_eq!(outcome.applied.status, pb::ChangeStatus::Applied as i32);
        let receipt = outcome
            .applied
            .apply_result
            .as_ref()
            .expect("durable apply receipt");
        assert!(!receipt.commit_id.is_empty());
        assert_eq!(outcome.descriptors.format, format as i32);
        assert_eq!(outcome.generated.format, format as i32);
        assert_eq!(outcome.descriptors.revision, outcome.generated.revision);
        assert!(outcome.descriptors.revision.ends_with(&receipt.commit_id));
        assert!(!outcome.descriptors.content.is_empty());
        assert!(outcome.descriptors.artifact_digest.starts_with("sha256:"));
        assert!(outcome.descriptors.closure_digest.starts_with("sha256:"));
        assert!(outcome.generated.artifact_digest.starts_with("sha256:"));
        assert_eq!(
            outcome.descriptors.closure_digest,
            outcome.generated.closure_digest
        );
        let generated =
            String::from_utf8(outcome.generated.content).expect("generated Rust artifact is UTF-8");
        assert!(
            generated.contains(generated_symbol),
            "generated code should contain {generated_symbol}:\n{generated}"
        );
    }
}

#[tokio::test]
async fn draft_update_and_abandon_enforce_etags_roles_and_terminal_state() {
    // Arrange
    let auth_dir = tempfile::tempdir().unwrap();
    let url = start_server_with(config_with_change_actors(auth_dir.path())).await;
    let mut clients = clients(&url).await;
    let created = create_note(
        &mut clients.change,
        "agent-token",
        "agent-update",
        "Initial observation",
    )
    .await;

    // Act: the author patches selected draft fields.
    let mut patch = created.clone();
    patch.title = "Confirmed identifier drift".to_string();
    patch.description = "Confirmed from two producers".to_string();
    patch.external_references = vec![
        "INC-2048".to_string(),
        "https://tracker.example.test/issues/2048".to_string(),
    ];
    let updated = clients
        .change
        .update_change(with_token(
            Request::new(pb::UpdateChangeRequest {
                change: Some(patch.clone()),
                update_mask: Some(FieldMask {
                    paths: vec![
                        "title".to_string(),
                        "description".to_string(),
                        "external_references".to_string(),
                    ],
                }),
            }),
            "agent-token",
        ))
        .await
        .expect("update own draft")
        .into_inner();

    // Assert
    assert_eq!(updated.title, "Confirmed identifier drift");
    assert_eq!(updated.description, "Confirmed from two producers");
    assert_eq!(
        updated.external_references,
        ["INC-2048", "https://tracker.example.test/issues/2048"]
    );
    assert_eq!(updated.etag, "v2");

    // Act: retrying with the stale v1 ETag is an optimistic-concurrency abort.
    let stale = clients
        .change
        .update_change(with_token(
            Request::new(pb::UpdateChangeRequest {
                change: Some(patch),
                update_mask: Some(FieldMask {
                    paths: vec!["title".to_string()],
                }),
            }),
            "agent-token",
        ))
        .await
        .expect_err("stale etag must fail");

    // Assert
    assert_eq!(stale.code(), tonic::Code::Aborted);

    // Act: a Reader attempts to patch the same record.
    let mut reader_patch = updated.clone();
    reader_patch.title = "Reader rewrite".to_string();
    let denied = clients
        .change
        .update_change(with_token(
            Request::new(pb::UpdateChangeRequest {
                change: Some(reader_patch),
                update_mask: Some(FieldMask {
                    paths: vec!["title".to_string()],
                }),
            }),
            "reader-token",
        ))
        .await
        .expect_err("reader cannot update");

    // Assert
    assert_eq!(denied.code(), tonic::Code::PermissionDenied);

    // Act: the agent abandons its own record.
    let abandoned = clients
        .change
        .abandon_change(with_token(
            Request::new(pb::AbandonChangeRequest {
                name: updated.name.clone(),
                etag: updated.etag.clone(),
            }),
            "agent-token",
        ))
        .await
        .expect("abandon own draft")
        .into_inner();

    // Assert
    assert_eq!(abandoned.status, pb::ChangeStatus::Abandoned as i32);
    assert_eq!(abandoned.etag, "v3");
    let terminal = clients
        .change
        .update_change(with_token(
            Request::new(pb::UpdateChangeRequest {
                change: Some(abandoned),
                update_mask: Some(FieldMask {
                    paths: vec!["title".to_string()],
                }),
            }),
            "agent-token",
        ))
        .await
        .expect_err("terminal record is immutable");
    assert_eq!(terminal.code(), tonic::Code::FailedPrecondition);

    // Act: the standard Delete method is also a soft abandon.
    let deletable = create_note(
        &mut clients.change,
        "agent-token",
        "agent-delete",
        "Delete compatibility shape",
    )
    .await;
    clients
        .change
        .delete_change(with_token(
            Request::new(pb::DeleteChangeRequest {
                name: deletable.name.clone(),
                etag: deletable.etag,
            }),
            "agent-token",
        ))
        .await
        .expect("soft delete");
    let deleted = clients
        .change
        .get_change(with_token(
            Request::new(pb::GetChangeRequest {
                name: deletable.name,
            }),
            "agent-token",
        ))
        .await
        .expect("read soft-deleted audit record")
        .into_inner();

    // Assert
    assert_eq!(deleted.status, pb::ChangeStatus::Abandoned as i32);
}

#[tokio::test]
async fn executable_change_validates_transitions_to_ready_and_records_review() {
    // Arrange
    let auth_dir = tempfile::tempdir().unwrap();
    let url = start_server_with(config_with_change_actors(auth_dir.path())).await;
    let mut clients = clients(&url).await;
    let executable = pb::ChangeRecord {
        target_bookmark: "main".to_string(),
        title: "Add order schema".to_string(),
        edits: vec![pb::ChangeEdit {
            edit: Some(pb::change_edit::Edit::ReplaceSource(
                pb::ReplaceSchemaSourceEdit {
                    schema_path: "order.proto".to_string(),
                    format_id: "protobuf".to_string(),
                    source: "syntax = \"proto3\"; message Order { string id = 1; }".to_string(),
                },
            )),
        }],
        ..Default::default()
    };
    let draft = clients
        .change
        .create_change(with_token(
            Request::new(pb::CreateChangeRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                change: Some(executable),
                change_id: "executable-change".to_string(),
            }),
            "agent-token",
        ))
        .await
        .expect("create executable change")
        .into_inner();

    // Act
    let validated = clients
        .change
        .validate_change(with_token(
            Request::new(pb::ValidateChangeRequest {
                name: draft.name.clone(),
                etag: draft.etag,
            }),
            "agent-token",
        ))
        .await
        .expect("validate change")
        .into_inner();
    let ready = clients
        .change
        .mark_change_ready(with_token(
            Request::new(pb::MarkChangeReadyRequest {
                name: validated.name.clone(),
                etag: validated.etag.clone(),
            }),
            "agent-token",
        ))
        .await
        .expect("mark change ready")
        .into_inner();
    let writer_review = clients
        .change
        .approve_change(with_token(
            Request::new(pb::ApproveChangeRequest {
                name: ready.name.clone(),
                etag: ready.etag.clone(),
                reason: "self-selected approval".to_string(),
            }),
            "agent-token",
        ))
        .await
        .expect_err("writer cannot approve");
    let approved = clients
        .change
        .approve_change(with_token(
            Request::new(pb::ApproveChangeRequest {
                name: ready.name,
                etag: ready.etag,
                reason: "Reviewed compiler validation".to_string(),
            }),
            "human-token",
        ))
        .await
        .expect("maintainer approves")
        .into_inner();
    let applied = clients
        .change
        .apply_change(with_token(
            Request::new(pb::ApplyChangeRequest {
                name: approved.name.clone(),
                etag: approved.etag.clone(),
                request_id: "apply-executable-change".to_string(),
            }),
            "agent-token",
        ))
        .await
        .expect("apply ready change")
        .into_inner();
    let retried = clients
        .change
        .apply_change(with_token(
            Request::new(pb::ApplyChangeRequest {
                name: approved.name.clone(),
                etag: approved.etag.clone(),
                request_id: "apply-executable-change".to_string(),
            }),
            "agent-token",
        ))
        .await
        .expect("retry applied change")
        .into_inner();
    let source = clients
        .explore
        .get_schema_source(with_token(
            Request::new(pb::GetSchemaSourceRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                schema_path: "order.proto".to_string(),
                at: Some(vref_branch("main")),
            }),
            "agent-token",
        ))
        .await
        .expect("read applied schema")
        .into_inner();
    let source = String::from_utf8(source.source).expect("schema source is UTF-8");

    // Assert
    let validation = validated.validation.expect("stored validation");
    assert!(validation.valid);
    assert!(validation.issues.is_empty());
    assert!(!validation.resolved_base_commit.is_empty());
    assert!(validation.edit_digest.starts_with("sha256:"));
    assert_eq!(ready.status, pb::ChangeStatus::Ready as i32);
    assert_eq!(writer_review.code(), tonic::Code::PermissionDenied);
    assert_eq!(approved.status, pb::ChangeStatus::Ready as i32);
    assert_eq!(approved.reviews.len(), 1);
    let review = &approved.reviews[0];
    assert_eq!(review.decision, pb::ReviewDecision::Approved as i32);
    assert_eq!(
        review
            .reviewer
            .as_ref()
            .map(|actor| actor.identity.as_str()),
        Some("alice")
    );
    assert_eq!(applied.status, pb::ChangeStatus::Applied as i32);
    assert!(applied.apply_attempt.is_some());
    let receipt = applied.apply_result.as_ref().expect("apply receipt");
    assert!(!receipt.commit_id.is_empty());
    assert!(!receipt.operation_id.is_empty());
    assert_eq!(retried.apply_result, applied.apply_result);
    assert!(source.contains("message Order"));
}

#[tokio::test]
async fn invalid_change_returns_findings_as_data_and_cannot_be_ready() {
    // Arrange
    let auth_dir = tempfile::tempdir().unwrap();
    let url = start_server_with(config_with_change_actors(auth_dir.path())).await;
    let mut clients = clients(&url).await;
    let draft = clients
        .change
        .create_change(with_token(
            Request::new(pb::CreateChangeRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                change: Some(pb::ChangeRecord {
                    target_bookmark: "main".to_string(),
                    title: "Invalid schema proposal".to_string(),
                    edits: vec![pb::ChangeEdit {
                        edit: Some(pb::change_edit::Edit::ReplaceSource(
                            pb::ReplaceSchemaSourceEdit {
                                schema_path: "broken.proto".to_string(),
                                format_id: "protobuf".to_string(),
                                source: "not protobuf".to_string(),
                            },
                        )),
                    }],
                    ..Default::default()
                }),
                change_id: "invalid-change".to_string(),
            }),
            "agent-token",
        ))
        .await
        .expect("create invalid proposal")
        .into_inner();

    // Act
    let validated = clients
        .change
        .validate_change(with_token(
            Request::new(pb::ValidateChangeRequest {
                name: draft.name,
                etag: draft.etag,
            }),
            "agent-token",
        ))
        .await
        .expect("validation findings are returned normally")
        .into_inner();
    let ready_error = clients
        .change
        .mark_change_ready(with_token(
            Request::new(pb::MarkChangeReadyRequest {
                name: validated.name.clone(),
                etag: validated.etag.clone(),
            }),
            "agent-token",
        ))
        .await
        .expect_err("invalid proposal cannot be ready");

    // Assert
    let validation = validated.validation.expect("stored validation");
    assert!(!validation.valid);
    assert!(validation
        .issues
        .iter()
        .any(|issue| issue.code == "source_invalid"));
    assert_eq!(ready_error.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test]
async fn create_rejects_client_supplied_audit_actor() {
    // Arrange
    let auth_dir = tempfile::tempdir().unwrap();
    let url = start_server_with(config_with_change_actors(auth_dir.path())).await;
    let mut clients = clients(&url).await;
    let mut forged = note("Forge actor", "This must be rejected");
    forged.created_by = Some(pb::Actor {
        identity: "admin".to_string(),
        kind: pb::ActorKind::Human as i32,
        display_name: "Forged".to_string(),
        delegated_by: String::new(),
    });

    // Act
    let error = clients
        .change
        .create_change(with_token(
            Request::new(pb::CreateChangeRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                change: Some(forged),
                change_id: "forged-actor".to_string(),
            }),
            "agent-token",
        ))
        .await
        .expect_err("client actor must be rejected");

    // Assert
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
}
