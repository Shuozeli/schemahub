//! Executable SchemaHub 1.0 acceptance journey.
//!
//! One delegated agent proposes compiler-backed schema changes, a human owner
//! reviews them, the agent applies them through repository policy, and a
//! consumer retrieves byte-identical immutable artifacts after a redb restart.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{clients, Clients};
use schemahub_api::schemahub_v1 as pb;
use schemahub_jj::{ObjectDb, RedbObjectDb};
use schemahub_server::config::{
    AuthConfig, Config, ProjectSection, RepoReviewSection, RepoSection, RepoServingSection,
    TokenIdentity,
};
use schemahub_server::{build_core, build_router};
use schemahub_types::IdentityKind;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::metadata::MetadataValue;
use tonic::Request;

const HUMAN_TOKEN: &str = "ga-human-token";
const AGENT_TOKEN: &str = "ga-agent-token";

struct RunningServer {
    url: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl RunningServer {
    async fn stop(mut self) {
        self.shutdown
            .take()
            .expect("shutdown sender")
            .send(())
            .expect("request graceful shutdown");
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("server stops before timeout")
            .expect("server task joins");
    }
}

struct FormatCase {
    change_id: &'static str,
    schema_path: &'static str,
    format_id: &'static str,
    format: pb::SchemaFormat,
    generated_symbol: &'static str,
    source: &'static str,
}

struct BeforeRestart {
    change_name: String,
    commit_id: String,
    descriptors: pb::SchemaArtifact,
    generated: pb::SchemaArtifact,
}

struct AcceptanceResult {
    format: pb::SchemaFormat,
    generated_symbol: &'static str,
    before: BeforeRestart,
    restored_change: pb::ChangeRecord,
    restored_descriptors: pb::SchemaArtifact,
    restored_generated: pb::SchemaArtifact,
}

fn ga_config(data_dir: &Path) -> Config {
    Config {
        auth: AuthConfig {
            data_dir: data_dir.to_string_lossy().to_string(),
            tokens: HashMap::from([
                (
                    HUMAN_TOKEN.to_string(),
                    TokenIdentity {
                        id: "alice".to_string(),
                        display: Some("Alice".to_string()),
                        kind: IdentityKind::Human,
                        delegated_by: None,
                    },
                ),
                (
                    AGENT_TOKEN.to_string(),
                    TokenIdentity {
                        id: "schema-agent".to_string(),
                        display: Some("Schema Agent".to_string()),
                        kind: IdentityKind::Agent,
                        delegated_by: Some("alice".to_string()),
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
                members: HashMap::from([("schema-agent".to_string(), "writer".to_string())]),
            },
        )]),
        repos: HashMap::from([(
            "acme/registry".to_string(),
            RepoSection {
                default_bookmark: Some("main".to_string()),
                compatibility: Some("full".to_string()),
                protected_bookmarks: Some(vec!["main".to_string()]),
                review: Some(RepoReviewSection {
                    required_approvals: Some(1),
                    require_change_record: Some(true),
                }),
                serving: Some(RepoServingSection {
                    source: Some(true),
                    descriptors: Some(true),
                    generated_code: Some(true),
                }),
            },
        )]),
        ..Default::default()
    }
}

fn format_cases() -> [FormatCase; 2] {
    [
        FormatCase {
            change_id: "ga-protobuf",
            schema_path: "orders.proto",
            format_id: "protobuf",
            format: pb::SchemaFormat::Protobuf,
            generated_symbol: "OrderRecord",
            source: r#"syntax = "proto3";
package ga.acceptance;
message OrderRecord { string id = 1; }
"#,
        },
        FormatCase {
            change_id: "ga-flatbuffers",
            schema_path: "events.fbs",
            format_id: "flatbuffers",
            format: pb::SchemaFormat::Flatbuffers,
            generated_symbol: "EventRecord",
            source: r#"namespace ga.acceptance;

table EventRecord {
  id: string;
}

root_type EventRecord;
"#,
        },
    ]
}

fn with_token<T>(mut request: Request<T>, token: &str) -> Request<T> {
    let value: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid token");
    request.metadata_mut().insert("authorization", value);
    request
}

async fn start_redb_server(db_path: &Path, config: &Config) -> RunningServer {
    let db: Arc<dyn ObjectDb> = Arc::new(RedbObjectDb::open(db_path).expect("open redb"));
    let core = build_core(db, config);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind acceptance server");
    let address = listener.local_addr().expect("acceptance server address");
    let incoming = TcpListenerStream::new(listener);
    let (shutdown, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        build_router(core, "redb")
            .serve_with_incoming_shutdown(incoming, async {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("serve acceptance API");
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    RunningServer {
        url: format!("http://{address}"),
        shutdown: Some(shutdown),
        task,
    }
}

async fn apply_and_materialize(clients: &mut Clients, case: &FormatCase) -> BeforeRestart {
    let created = clients
        .change
        .create_change(with_token(
            Request::new(pb::CreateChangeRequest {
                parent: "projects/acme/repos/registry".to_string(),
                change: Some(pb::ChangeRecord {
                    target_bookmark: "main".to_string(),
                    title: format!("Add {}", case.schema_path),
                    description: "Delegated agent proposal for the GA acceptance run".to_string(),
                    external_references: vec![format!("GA-{}", case.change_id)],
                    edits: vec![pb::ChangeEdit {
                        edit: Some(pb::change_edit::Edit::ReplaceSource(
                            pb::ReplaceSchemaSourceEdit {
                                schema_path: case.schema_path.to_string(),
                                format_id: case.format_id.to_string(),
                                source: case.source.to_string(),
                            },
                        )),
                    }],
                    ..Default::default()
                }),
                change_id: case.change_id.to_string(),
            }),
            AGENT_TOKEN,
        ))
        .await
        .expect("agent creates change")
        .into_inner();
    let validated = clients
        .change
        .validate_change(with_token(
            Request::new(pb::ValidateChangeRequest {
                name: created.name,
                etag: created.etag,
            }),
            AGENT_TOKEN,
        ))
        .await
        .expect("compiler validates change")
        .into_inner();
    let ready = clients
        .change
        .mark_change_ready(with_token(
            Request::new(pb::MarkChangeReadyRequest {
                name: validated.name,
                etag: validated.etag,
            }),
            AGENT_TOKEN,
        ))
        .await
        .expect("agent marks change ready")
        .into_inner();
    let approved = clients
        .change
        .approve_change(with_token(
            Request::new(pb::ApproveChangeRequest {
                name: ready.name,
                etag: ready.etag,
                reason: "Human approved compiler validation".to_string(),
            }),
            HUMAN_TOKEN,
        ))
        .await
        .expect("human approves change")
        .into_inner();
    let applied = clients
        .change
        .apply_change(with_token(
            Request::new(pb::ApplyChangeRequest {
                name: approved.name,
                etag: approved.etag,
                request_id: format!("apply-{}", case.change_id),
            }),
            AGENT_TOKEN,
        ))
        .await
        .expect("agent applies approved change")
        .into_inner();
    let receipt = applied
        .apply_result
        .as_ref()
        .expect("applied change receipt");
    let revision = format!(
        "projects/acme/repos/registry/revisions/{}",
        receipt.commit_id
    );
    let descriptors = fetch_artifact(
        clients,
        &revision,
        case.schema_path,
        pb::SchemaArtifactKind::Descriptors,
    )
    .await;
    let generated = fetch_artifact(
        clients,
        &revision,
        case.schema_path,
        pb::SchemaArtifactKind::GeneratedCode,
    )
    .await;

    BeforeRestart {
        change_name: applied.name,
        commit_id: receipt.commit_id.clone(),
        descriptors,
        generated,
    }
}

async fn fetch_artifact(
    clients: &mut Clients,
    revision: &str,
    schema_path: &str,
    kind: pb::SchemaArtifactKind,
) -> pb::SchemaArtifact {
    clients
        .serving
        .get_schema_artifact(with_token(
            Request::new(pb::GetSchemaArtifactRequest {
                revision: revision.to_string(),
                schema_path: schema_path.to_string(),
                kind: kind as i32,
                language: if kind == pb::SchemaArtifactKind::GeneratedCode {
                    pb::Language::Rust as i32
                } else {
                    pb::Language::Unspecified as i32
                },
                ..Default::default()
            }),
            AGENT_TOKEN,
        ))
        .await
        .expect("fetch immutable artifact")
        .into_inner()
}

async fn execute_ga_acceptance(db_path: PathBuf, config: Config) -> Vec<AcceptanceResult> {
    let first_server = start_redb_server(&db_path, &config).await;
    let mut first_clients = clients(&first_server.url).await;
    let cases = format_cases();
    let mut before_restart = Vec::new();
    for case in &cases {
        before_restart.push(apply_and_materialize(&mut first_clients, case).await);
    }
    drop(first_clients);
    first_server.stop().await;

    let restarted_server = start_redb_server(&db_path, &config).await;
    let mut restarted_clients = clients(&restarted_server.url).await;
    let mut results = Vec::new();
    for (case, before) in cases.into_iter().zip(before_restart) {
        let restored_change = restarted_clients
            .change
            .get_change(with_token(
                Request::new(pb::GetChangeRequest {
                    name: before.change_name.clone(),
                }),
                HUMAN_TOKEN,
            ))
            .await
            .expect("read durable change after restart")
            .into_inner();
        let revision = format!(
            "projects/acme/repos/registry/revisions/{}",
            before.commit_id
        );
        let restored_descriptors = fetch_artifact(
            &mut restarted_clients,
            &revision,
            case.schema_path,
            pb::SchemaArtifactKind::Descriptors,
        )
        .await;
        let restored_generated = fetch_artifact(
            &mut restarted_clients,
            &revision,
            case.schema_path,
            pb::SchemaArtifactKind::GeneratedCode,
        )
        .await;
        results.push(AcceptanceResult {
            format: case.format,
            generated_symbol: case.generated_symbol,
            before,
            restored_change,
            restored_descriptors,
            restored_generated,
        });
    }
    drop(restarted_clients);
    restarted_server.stop().await;
    results
}

#[tokio::test]
async fn delegated_agent_to_immutable_artifact_journey_survives_restart() {
    // Arrange
    let temp = tempfile::tempdir().expect("acceptance tempdir");
    let db_path = temp.path().join("schemahub-ga.redb");
    let config = ga_config(temp.path());

    // Act
    let results = execute_ga_acceptance(db_path, config).await;

    // Assert
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(
            result.restored_change.status,
            pb::ChangeStatus::Applied as i32
        );
        let author = result
            .restored_change
            .created_by
            .as_ref()
            .expect("durable agent author");
        assert_eq!(author.identity, "schema-agent");
        assert_eq!(author.kind, pb::ActorKind::Agent as i32);
        assert_eq!(author.delegated_by, "alice");
        let reviewer = result.restored_change.reviews[0]
            .reviewer
            .as_ref()
            .expect("durable human reviewer");
        assert_eq!(reviewer.identity, "alice");
        assert_eq!(reviewer.kind, pb::ActorKind::Human as i32);
        assert_eq!(
            result
                .restored_change
                .apply_result
                .as_ref()
                .expect("durable apply receipt")
                .commit_id,
            result.before.commit_id
        );

        assert_eq!(result.before.descriptors.format, result.format as i32);
        assert_eq!(result.before.generated.format, result.format as i32);
        assert_eq!(
            result.restored_descriptors.content,
            result.before.descriptors.content
        );
        assert_eq!(
            result.restored_descriptors.artifact_digest,
            result.before.descriptors.artifact_digest
        );
        assert_eq!(
            result.restored_descriptors.closure_digest,
            result.before.descriptors.closure_digest
        );
        assert_eq!(
            result.restored_generated.content,
            result.before.generated.content
        );
        assert_eq!(
            result.restored_generated.artifact_digest,
            result.before.generated.artifact_digest
        );
        assert_eq!(
            result.restored_generated.closure_digest,
            result.before.generated.closure_digest
        );
        assert!(result
            .restored_descriptors
            .artifact_digest
            .starts_with("sha256:"));
        assert!(result
            .restored_generated
            .artifact_digest
            .starts_with("sha256:"));
        let generated = String::from_utf8(result.restored_generated.content)
            .expect("restored generated Rust is UTF-8");
        assert!(
            generated.contains(result.generated_symbol),
            "generated code should contain {}:\n{generated}",
            result.generated_symbol
        );
    }
}
