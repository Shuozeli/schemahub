//! Whole-schema lifecycle contract tests.

mod common;

use common::*;
use schemahub_api::schemahub_v1 as pb;
use tonic::Code;

const USER_V1: &str = r#"syntax = "proto3";
package demo.v1;
message User {
  string id = 1;
  string email = 2;
}
"#;

const USER_WITHOUT_EMAIL: &str = r#"syntax = "proto3";
package demo.v1;
message User {
  string id = 1;
}
"#;

fn create_request(format: pb::SchemaFormat) -> pb::CreateSchemaRequest {
    pb::CreateSchemaRequest {
        project: "acme".into(),
        repo: "core".into(),
        branch: "main".into(),
        schema_name: "user.proto".into(),
        format: format as i32,
        source: USER_V1.into(),
        base_revision: String::new(),
        idempotency_key: "create-user".into(),
    }
}

#[tokio::test]
async fn create_requires_an_explicit_format() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;

    // Act
    let error = clients
        .schema
        .create_schema(create_request(pb::SchemaFormat::Unspecified))
        .await
        .expect_err("unspecified format must fail");

    // Assert
    assert_eq!(error.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn create_rejects_a_format_that_disagrees_with_the_extension() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;

    // Act
    let error = clients
        .schema
        .create_schema(create_request(pb::SchemaFormat::Flatbuffers))
        .await
        .expect_err("format mismatch must fail");

    // Assert
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(error.message().contains("extension selects format"));
}

#[tokio::test]
async fn create_rejects_an_existing_schema() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    clients
        .schema
        .create_schema(create_request(pb::SchemaFormat::Protobuf))
        .await
        .expect("seed schema");
    let mut duplicate = create_request(pb::SchemaFormat::Protobuf);
    duplicate.idempotency_key = "second-create".into();

    // Act
    let error = clients
        .schema
        .create_schema(duplicate)
        .await
        .expect_err("duplicate create must fail");

    // Assert
    assert_eq!(error.code(), Code::AlreadyExists);
}

#[tokio::test]
async fn update_rejects_a_missing_schema() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;

    // Act
    let error = clients
        .schema
        .update_schema(pb::UpdateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "missing.proto".into(),
            source: USER_V1.into(),
            base_revision: String::new(),
            idempotency_key: "update-missing".into(),
            force: false,
        })
        .await
        .expect_err("missing update must fail");

    // Assert
    assert_eq!(error.code(), Code::NotFound);
}

#[tokio::test]
async fn whole_source_update_runs_compatibility_on_a_protected_bookmark() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    clients
        .schema
        .create_schema(create_request(pb::SchemaFormat::Protobuf))
        .await
        .expect("seed schema");

    // Act
    let error = clients
        .schema
        .update_schema(pb::UpdateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            source: USER_WITHOUT_EMAIL.into(),
            base_revision: String::new(),
            idempotency_key: "breaking-update".into(),
            force: false,
        })
        .await
        .expect_err("breaking update must fail");

    // Assert
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert!(error.message().contains("compatibility violation"));
}

#[tokio::test]
async fn authorized_force_allows_a_whole_source_compatibility_override() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    clients
        .schema
        .create_schema(create_request(pb::SchemaFormat::Protobuf))
        .await
        .expect("seed schema");

    // Act
    let response = clients
        .schema
        .update_schema(pb::UpdateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            source: USER_WITHOUT_EMAIL.into(),
            base_revision: String::new(),
            idempotency_key: "forced-update".into(),
            force: true,
        })
        .await
        .expect("Noop policy authorizes force")
        .into_inner();

    // Assert
    assert!(!response.new_commit.is_empty());
    assert!(response.conflicted_decls.is_empty());
}

#[tokio::test]
async fn delete_force_does_not_bypass_live_reference_integrity() {
    // Arrange: `dev` is unprotected, so only the reference-integrity rule can
    // reject the forced delete. Nested schema names also exercise JJ listing.
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "acme",
        "core",
        "dev",
        "common/types.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\"; package common; message Shared { string id = 1; }",
        "create-common",
    )
    .await;
    create_schema(
        &mut clients.schema,
        "acme",
        "core",
        "dev",
        "orders/order.proto",
        pb::SchemaFormat::Protobuf,
        r#"syntax = "proto3";
package orders;
import "acme/core/common/types.proto";
message Order { common.Shared shared = 1; }
"#,
        "create-order",
    )
    .await;

    // Act
    let error = clients
        .schema
        .delete_schema(pb::DeleteSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "dev".into(),
            schema_name: "common/types.proto".into(),
            base_revision: String::new(),
            idempotency_key: "delete-common".into(),
            force: true,
        })
        .await
        .expect_err("live dependent must block forced delete");

    // Assert
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert!(error.message().contains("orders/order.proto"));
}

#[tokio::test]
async fn delete_allows_an_import_pinned_to_a_retained_commit() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let provider = create_schema(
        &mut clients.schema,
        "acme",
        "core",
        "dev",
        "common/types.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\"; package common; message Shared { string id = 1; }",
        "pinned-create-common",
    )
    .await;
    create_schema(
        &mut clients.schema,
        "acme",
        "core",
        "dev",
        "orders/order.proto",
        pb::SchemaFormat::Protobuf,
        r#"syntax = "proto3";
package orders;
import "acme/core/common/types.proto";
message Order { common.Shared shared = 1; }
"#,
        "pinned-create-order",
    )
    .await;
    clients
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "dev".into(),
            base_revision: String::new(),
            idempotency_key: "pin-order-import".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "orders/order.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::UpdateImport(
                        pb::ProtoUpdateImport {
                            import_path: "acme/core/common/types.proto".into(),
                            to_commit: provider.new_commit,
                            to_tag: String::new(),
                            remove: false,
                        },
                    )),
                },
            )),
        })
        .await
        .expect("pin consumer import");

    // Act
    let response = clients
        .schema
        .delete_schema(pb::DeleteSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "dev".into(),
            schema_name: "common/types.proto".into(),
            base_revision: String::new(),
            idempotency_key: "delete-pinned-common".into(),
            force: false,
        })
        .await
        .expect("retained pin must allow bookmark deletion")
        .into_inner();

    // Assert
    assert!(!response.new_commit.is_empty());
    assert!(response.conflicted_decls.is_empty());
}

#[tokio::test]
async fn delete_on_a_protected_bookmark_requires_force() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    clients
        .schema
        .create_schema(create_request(pb::SchemaFormat::Protobuf))
        .await
        .expect("seed schema");

    // Act
    let error = clients
        .schema
        .delete_schema(pb::DeleteSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            base_revision: String::new(),
            idempotency_key: "unforced-delete".into(),
            force: false,
        })
        .await
        .expect_err("protected delete must require force");

    // Assert
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert!(error.message().contains("compatibility violation"));
}

#[tokio::test]
async fn delete_force_can_override_protected_bookmark_compatibility() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    clients
        .schema
        .create_schema(create_request(pb::SchemaFormat::Protobuf))
        .await
        .expect("seed schema");

    // Act
    let response = clients
        .schema
        .delete_schema(pb::DeleteSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            schema_name: "user.proto".into(),
            base_revision: String::new(),
            idempotency_key: "forced-delete".into(),
            force: true,
        })
        .await
        .expect("Noop policy authorizes force")
        .into_inner();

    // Assert
    assert!(!response.new_commit.is_empty());
    assert!(response.conflicted_decls.is_empty());
}
