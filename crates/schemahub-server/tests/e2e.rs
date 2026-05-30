//! End-to-end integration test exercising the real stack: an in-process tonic
//! server over a `MemoryObjectDb`, driven through the generated gRPC clients
//! (crate-structure.md §6). One flow: create schema → add field → pull source →
//! op log → undo.

use std::sync::Arc;
use std::time::Duration;

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::exploration_service_client::ExplorationServiceClient;
use schemahub_api::schemahub_v1::history_service_client::HistoryServiceClient;
use schemahub_api::schemahub_v1::ref_service_client::RefServiceClient;
use schemahub_api::schemahub_v1::schema_service_client::SchemaServiceClient;
use schemahub_server::{build_core, build_router, config::Config};
use schemahub_vcs::{MemoryObjectDb, ObjectDb};
use tokio::net::TcpListener;
use tonic::transport::Channel;

/// Start the server in-process on an ephemeral port; return its base URL.
async fn start_server() -> String {
    // Arrange: build a Core over an in-memory object store and bind a router.
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db, &Config::default());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        build_router(core)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    // Give the server a moment to begin accepting connections.
    tokio::time::sleep(Duration::from_millis(100)).await;
    format!("http://{addr}")
}

async fn connect(url: &str) -> Channel {
    Channel::from_shared(url.to_string())
        .unwrap()
        .connect()
        .await
        .unwrap()
}

const USER_PROTO: &str = r#"syntax = "proto3";
package user.v1;

message User {
  string id = 1;
  string name = 2;
}
"#;

#[tokio::test]
async fn create_add_field_pull_oplog_undo_roundtrip() {
    // Arrange: a running server and a connected channel.
    let url = start_server().await;
    let ch = connect(&url).await;
    let mut schema_client = SchemaServiceClient::new(ch.clone());
    let mut explore_client = ExplorationServiceClient::new(ch.clone());
    let mut history_client = HistoryServiceClient::new(ch.clone());

    // Act 1: create a schema on a fresh "main" bookmark.
    let create = schema_client
        .create_schema(pb::CreateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: USER_PROTO.into(),
            base_revision: String::new(),
            idempotency_key: "k1".into(),
        })
        .await
        .expect("create_schema")
        .into_inner();

    // Assert: a commit + a stable change id were produced, no conflicts.
    assert!(!create.new_commit.is_empty(), "commit id should be set");
    assert!(!create.change_id.is_empty(), "change id should be set");
    assert!(create.conflicted_decls.is_empty());

    // Act 2: add a field to the User message via a granular mutation. The CLI
    // sends a typed ProtoAddField; the SERVER encodes the op envelope.
    let add_field = schema_client
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "k2".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "user.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::AddField(
                        pb::ProtoAddField {
                            message_name: "User".into(),
                            field_name: "email".into(),
                            field_type: "string".into(),
                            field_number: 3,
                            repeated: false,
                            doc_comment: String::new(),
                        },
                    )),
                },
            )),
        })
        .await
        .expect("apply_mutation add field")
        .into_inner();
    assert!(!add_field.new_commit.is_empty());

    // Act 3: pull the reconstructed source.
    let pulled = explore_client
        .get_schema_source(pb::GetSchemaSourceRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "user.proto".into(),
            at: Some(pb::VersionRef {
                r#ref: Some(pb::version_ref::Ref::Branch("main".into())),
            }),
        })
        .await
        .expect("get_schema_source")
        .into_inner();
    let source = String::from_utf8(pulled.source).unwrap();

    // Assert: the new field round-tripped into the printed source.
    assert!(
        source.contains("email"),
        "pulled source should contain the added field, got:\n{source}"
    );
    assert!(source.contains("message User"));

    // Act 4: read the operation log.
    let oplog = history_client
        .op_log(pb::OpLogRequest {
            project: "acme".into(),
            repo: "core".into(),
            limit: 0,
        })
        .await
        .expect("op_log")
        .into_inner();

    // Assert: two writes recorded (create + add-field).
    assert!(
        oplog.operations.len() >= 2,
        "expected at least 2 operations, got {}",
        oplog.operations.len()
    );

    // Act 5: undo the last operation (the add-field).
    let undo = history_client
        .undo(pb::UndoRequest {
            project: "acme".into(),
            repo: "core".into(),
            author: "tester".into(),
        })
        .await
        .expect("undo")
        .into_inner();
    assert!(!undo.undone_op_id.is_empty());

    // Assert: after undo, the field is gone from the reconstructed source.
    let after = explore_client
        .get_schema_source(pb::GetSchemaSourceRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "user.proto".into(),
            at: Some(pb::VersionRef {
                r#ref: Some(pb::version_ref::Ref::Branch("main".into())),
            }),
        })
        .await
        .expect("get_schema_source after undo")
        .into_inner();
    let after_source = String::from_utf8(after.source).unwrap();
    assert!(
        !after_source.contains("email"),
        "undo should have removed the added field, got:\n{after_source}"
    );
}

#[tokio::test]
async fn list_declarations_reports_message_enum_service() {
    // Arrange.
    let url = start_server().await;
    let ch = connect(&url).await;
    let mut schema_client = SchemaServiceClient::new(ch.clone());
    let mut explore_client = ExplorationServiceClient::new(ch.clone());

    let full_proto = r#"syntax = "proto3";
package user.v1;

message User { string id = 1; }
enum Status { STATUS_UNSPECIFIED = 0; }
service UserService { rpc Get(User) returns (User); }
"#;

    // Act: create the schema, then list its declarations.
    schema_client
        .create_schema(pb::CreateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: full_proto.into(),
            base_revision: String::new(),
            idempotency_key: "c1".into(),
        })
        .await
        .expect("create_schema")
        .into_inner();

    let decls = explore_client
        .list_declarations(pb::ListDeclarationsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "user.proto".into(),
            at: Some(pb::VersionRef {
                r#ref: Some(pb::version_ref::Ref::Branch("main".into())),
            }),
            kind_filter: pb::DeclKind::Unspecified as i32,
        })
        .await
        .expect("list_declarations")
        .into_inner();

    // Assert: all three top-level declarations are present.
    let names: Vec<&str> = decls.declarations.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"User"), "names: {names:?}");
    assert!(names.contains(&"Status"), "names: {names:?}");
    assert!(names.contains(&"UserService"), "names: {names:?}");
}

#[tokio::test]
async fn create_then_delete_branch_roundtrip() {
    // Arrange: a server with a schema committed on `main` so a branch has a base.
    let url = start_server().await;
    let ch = connect(&url).await;
    let mut schema_client = SchemaServiceClient::new(ch.clone());
    let mut ref_client = RefServiceClient::new(ch.clone());

    schema_client
        .create_schema(pb::CreateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: USER_PROTO.into(),
            base_revision: String::new(),
            idempotency_key: "b1".into(),
        })
        .await
        .expect("create_schema")
        .into_inner();

    // Act 1: create a feature branch off main.
    ref_client
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "feature/x".into(),
            from: Some(pb::VersionRef {
                r#ref: Some(pb::version_ref::Ref::Branch("main".into())),
            }),
        })
        .await
        .expect("create_branch")
        .into_inner();

    // Assert: it is listed.
    let before = ref_client
        .list_branches(pb::ListBranchesRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: String::new(),
        })
        .await
        .expect("list_branches")
        .into_inner();
    assert!(
        before.branches.iter().any(|b| b.name == "feature/x"),
        "feature/x should be listed, got {:?}",
        before.branches.iter().map(|b| &b.name).collect::<Vec<_>>()
    );

    // Act 2: delete the branch (previously UNIMPLEMENTED).
    ref_client
        .delete_branch(pb::DeleteBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "feature/x".into(),
        })
        .await
        .expect("delete_branch");

    // Assert: it is gone, but `main` remains.
    let after = ref_client
        .list_branches(pb::ListBranchesRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: String::new(),
        })
        .await
        .expect("list_branches after delete")
        .into_inner();
    let names: Vec<&str> = after.branches.iter().map(|b| b.name.as_str()).collect();
    assert!(!names.contains(&"feature/x"), "branch should be deleted: {names:?}");
    assert!(names.contains(&"main"), "main should remain: {names:?}");
}

#[tokio::test]
async fn log_reports_real_commit_graph() {
    // Arrange: two writes on `main` (create schema + add field).
    let url = start_server().await;
    let ch = connect(&url).await;
    let mut schema_client = SchemaServiceClient::new(ch.clone());
    let mut history_client = HistoryServiceClient::new(ch.clone());

    let create = schema_client
        .create_schema(pb::CreateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: USER_PROTO.into(),
            base_revision: String::new(),
            idempotency_key: "l1".into(),
        })
        .await
        .expect("create_schema")
        .into_inner();
    let add = schema_client
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "l2".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "user.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::AddField(
                        pb::ProtoAddField {
                            message_name: "User".into(),
                            field_name: "email".into(),
                            field_type: "string".into(),
                            field_number: 3,
                            repeated: false,
                            doc_comment: String::new(),
                        },
                    )),
                },
            )),
        })
        .await
        .expect("apply_mutation")
        .into_inner();

    // Act: read the real commit/change log.
    let log = history_client
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "core".into(),
            at: None,
            limit: 0,
        })
        .await
        .expect("log")
        .into_inner();

    // Assert: two commits, newest first, carrying the real commit ids returned by
    // the write RPCs, each with a distinct stable change id.
    assert_eq!(log.entries.len(), 2, "expected 2 commits in the graph");
    let head = &log.entries[0];
    let base = &log.entries[1];
    assert_eq!(head.commit_id, add.new_commit, "head is the add-field commit");
    assert_eq!(base.commit_id, create.new_commit, "base is the create commit");
    assert_ne!(head.commit_id, head.change_id, "commit id != change id");
    assert_eq!(head.parents, vec![base.commit_id.clone()], "linear parent link");
    // The base write's parent is the jj root commit (not the add-field commit),
    // so it never points back at the head.
    assert!(
        !base.parents.contains(&head.commit_id),
        "base must not list head as a parent: {:?}",
        base.parents
    );
}
