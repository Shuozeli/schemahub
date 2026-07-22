//! Durable idempotency contract across every schema write surface.

mod common;

use common::*;
use schemahub_api::schemahub_v1 as pb;
use schemahub_core::{MAX_DEPENDENCY_SCAN_REPOSITORIES, MAX_DEPENDENCY_SCAN_SCHEMAS};
use schemahub_server::{BUILD_VERSION, TRANSACTION_TIMEOUT_SECS};
use tonic::Code;

const BASE: &str = r#"syntax = "proto3";
package demo.v1;
message User { string id = 1; }
"#;

const WITH_EMAIL: &str = r#"syntax = "proto3";
package demo.v1;
message User {
  string id = 1;
  string email = 2;
}
"#;

fn add_field(schema: &str, name: &str, number: u32) -> pb::ProtobufMutation {
    pb::ProtobufMutation {
        schema_path: schema.into(),
        operation: Some(pb::protobuf_mutation::Operation::AddField(
            pb::ProtoAddField {
                message_name: "User".into(),
                field_name: name.into(),
                field_type: "string".into(),
                field_number: number,
                repeated: false,
                doc_comment: String::new(),
            },
        )),
    }
}

#[tokio::test]
async fn whole_schema_create_update_delete_retries_share_receipts() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    let create = pb::CreateSchemaRequest {
        project: "acme".into(),
        repo: "core".into(),
        branch: "main".into(),
        schema_name: "user.proto".into(),
        format: pb::SchemaFormat::Protobuf as i32,
        source: BASE.into(),
        base_revision: String::new(),
        idempotency_key: "create-once".into(),
    };

    // Act
    let first_create = c
        .schema
        .create_schema(create.clone())
        .await
        .expect("first create")
        .into_inner();
    let replayed_create = c
        .schema
        .create_schema(create.clone())
        .await
        .expect("replayed create")
        .into_inner();
    let mut changed_create = create;
    changed_create.source = WITH_EMAIL.into();
    let reused_create_key = c
        .schema
        .create_schema(changed_create)
        .await
        .expect_err("changed create must not reuse the key");

    let update = pb::UpdateSchemaRequest {
        project: "acme".into(),
        repo: "core".into(),
        branch: "main".into(),
        schema_name: "user.proto".into(),
        source: WITH_EMAIL.into(),
        base_revision: String::new(),
        idempotency_key: "update-once".into(),
        force: false,
    };
    let first_update = c
        .schema
        .update_schema(update.clone())
        .await
        .expect("first update")
        .into_inner();
    let replayed_update = c
        .schema
        .update_schema(update)
        .await
        .expect("replayed update")
        .into_inner();

    let delete = pb::DeleteSchemaRequest {
        project: "acme".into(),
        repo: "core".into(),
        branch: "main".into(),
        schema_name: "user.proto".into(),
        base_revision: String::new(),
        idempotency_key: "delete-once".into(),
        force: true,
    };
    let first_delete = c
        .schema
        .delete_schema(delete.clone())
        .await
        .expect("first delete")
        .into_inner();
    let replayed_delete = c
        .schema
        .delete_schema(delete)
        .await
        .expect("replayed delete after the schema is gone")
        .into_inner();
    let operations = c
        .history
        .op_log(pb::OpLogRequest {
            project: "acme".into(),
            repo: "core".into(),
            limit: 0,
        })
        .await
        .expect("op log")
        .into_inner()
        .operations;

    // Assert
    assert_eq!(first_create.new_commit, replayed_create.new_commit);
    assert_eq!(first_update.new_commit, replayed_update.new_commit);
    assert_eq!(first_delete.new_commit, replayed_delete.new_commit);
    assert_eq!(reused_create_key.code(), Code::FailedPrecondition);
    assert_eq!(operations.len(), 3, "each lifecycle request writes once");
}

#[tokio::test]
async fn transaction_and_merge_retries_are_deduped_and_key_bound() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE,
        "seed",
    )
    .await;
    let transaction = pb::ApplyTransactionRequest {
        project: "acme".into(),
        repo: "core".into(),
        branch: "main".into(),
        base_revision: String::new(),
        idempotency_key: "transaction-once".into(),
        force: false,
        operations: vec![pb::TransactionOp {
            operation: Some(pb::transaction_op::Operation::ProtobufOp(add_field(
                "user.proto",
                "email",
                2,
            ))),
        }],
    };

    // Act
    let first_transaction = c
        .schema
        .apply_transaction(transaction.clone())
        .await
        .expect("first transaction")
        .into_inner();
    let replayed_transaction = c
        .schema
        .apply_transaction(transaction.clone())
        .await
        .expect("replayed transaction")
        .into_inner();
    let mut changed_transaction = transaction;
    changed_transaction.operations = vec![pb::TransactionOp {
        operation: Some(pb::transaction_op::Operation::ProtobufOp(add_field(
            "user.proto",
            "phone",
            3,
        ))),
    }];
    let reused_transaction_key = c
        .schema
        .apply_transaction(changed_transaction)
        .await
        .expect_err("changed transaction must not reuse the key");

    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "feature/phone".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create feature branch");
    c.schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "feature/phone".into(),
            base_revision: String::new(),
            idempotency_key: "feature-phone".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                add_field("user.proto", "phone", 3),
            )),
        })
        .await
        .expect("write feature branch");
    let merge = pb::MergeRequest {
        project: "acme".into(),
        repo: "core".into(),
        source_branch: "feature/phone".into(),
        target_branch: "main".into(),
        base_revision: String::new(),
        idempotency_key: "merge-once".into(),
        message: "merge phone".into(),
    };
    let first_merge = c
        .refs
        .merge(merge.clone())
        .await
        .expect("first merge")
        .into_inner();
    let replayed_merge = c
        .refs
        .merge(merge.clone())
        .await
        .expect("replayed merge")
        .into_inner();
    let mut changed_merge = merge;
    changed_merge.message = "different merge request".into();
    let reused_merge_key = c
        .refs
        .merge(changed_merge)
        .await
        .expect_err("changed merge must not reuse the key");

    // Assert
    assert_eq!(
        first_transaction.new_commit,
        replayed_transaction.new_commit
    );
    assert_eq!(first_merge.new_commit, replayed_merge.new_commit);
    assert_eq!(reused_transaction_key.code(), Code::FailedPrecondition);
    assert_eq!(reused_merge_key.code(), Code::FailedPrecondition);
}

#[tokio::test]
async fn server_config_reports_durable_receipt_transaction_and_scan_limits() {
    // Arrange
    let url = start_server().await;
    let channel = connect(&url).await;
    let mut admin = pb::admin_service_client::AdminServiceClient::new(channel);

    // Act
    let config = admin
        .get_server_config(pb::GetServerConfigRequest {})
        .await
        .expect("server config")
        .into_inner();

    // Assert
    assert_eq!(config.idempotency_ttl_hours, 24);
    assert_eq!(
        u64::from(config.transaction_timeout_secs),
        TRANSACTION_TIMEOUT_SECS
    );
    assert_eq!(config.server_version, BUILD_VERSION);
    assert_eq!(
        config.max_dependency_scan_repositories,
        MAX_DEPENDENCY_SCAN_REPOSITORIES as u32
    );
    assert_eq!(
        config.max_dependency_scan_schemas,
        MAX_DEPENDENCY_SCAN_SCHEMAS as u32
    );
}
