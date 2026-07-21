//! Public reverse-dependency discovery contract over the real gRPC boundary.

mod common;

use common::{clients, create_schema, start_server};
use schemahub_api::schemahub_v1 as pb;
use tonic::Code;

#[tokio::test]
async fn list_dependents_returns_direct_live_and_pinned_edges_with_snapshot_manifest() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let provider = create_schema(
        &mut clients.schema,
        "acme",
        "provider",
        "main",
        "types.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Shared { string id = 1; }",
        "dependency-provider",
    )
    .await;
    let live = create_schema(
        &mut clients.schema,
        "acme",
        "live-consumer",
        "main",
        "orders.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; import \"acme/provider/types.proto\"; message Order { string id = 1; }",
        "dependency-live-consumer",
    )
    .await;
    create_schema(
        &mut clients.schema,
        "billing",
        "pinned-consumer",
        "main",
        "invoice.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Invoice { string id = 1; }",
        "dependency-pinned-consumer",
    )
    .await;
    let pinned = clients
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "billing".into(),
            repo: "pinned-consumer".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "dependency-pin".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "invoice.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::UpdateImport(
                        pb::ProtoUpdateImport {
                            import_path: "acme/provider/types.proto".into(),
                            to_commit: provider.new_commit.clone(),
                            to_tag: String::new(),
                            remove: false,
                        },
                    )),
                },
            )),
        })
        .await
        .expect("pin cross-repository import")
        .into_inner();

    // Act
    let response = clients
        .explore
        .list_dependents(pb::ListDependentsRequest {
            project: "acme".into(),
            repo: "provider".into(),
            schema_path: "types.proto".into(),
        })
        .await
        .expect("list direct dependents")
        .into_inner();

    // Assert
    assert_eq!(response.schemas_scanned, 3);
    assert_eq!(response.snapshots.len(), 3);
    assert_eq!(response.dependents.len(), 2);
    assert_eq!(response.dependents[0].importing_project, "acme");
    assert_eq!(response.dependents[0].importing_repo, "live-consumer");
    assert_eq!(response.dependents[0].importing_schema, "orders.proto");
    assert_eq!(response.dependents[0].importing_bookmark, "main");
    assert_eq!(response.dependents[0].importing_commit, live.new_commit);
    assert!(!response.dependents[0].pinned);
    assert!(response.dependents[0].resolved_commit.is_empty());
    assert_eq!(response.dependents[1].importing_project, "billing");
    assert_eq!(response.dependents[1].importing_repo, "pinned-consumer");
    assert_eq!(response.dependents[1].importing_commit, pinned.new_commit);
    assert!(response.dependents[1].pinned);
    assert_eq!(response.dependents[1].resolved_commit, provider.new_commit);
    assert!(response.snapshots.iter().all(|snapshot| {
        !snapshot.project.is_empty()
            && !snapshot.repo.is_empty()
            && snapshot.bookmark == "main"
            && !snapshot.commit_id.is_empty()
    }));
}

#[tokio::test]
async fn follow_type_selects_the_requested_field_and_honors_cross_repo_pin() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "acme",
        "decoy",
        "main",
        "types.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Decoy { string id = 1; }",
        "follow-decoy",
    )
    .await;
    let provider = create_schema(
        &mut clients.schema,
        "acme",
        "provider",
        "main",
        "types.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Shared { string legacy = 1; }",
        "follow-provider",
    )
    .await;
    create_schema(
        &mut clients.schema,
        "billing",
        "consumer",
        "main",
        "invoice.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; import \"acme/decoy/types.proto\"; import \"acme/provider/types.proto\"; message Invoice { Shared shared = 1; }",
        "follow-consumer",
    )
    .await;
    let pinned_source = clients
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "billing".into(),
            repo: "consumer".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "follow-pin-provider".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "invoice.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::UpdateImport(
                        pb::ProtoUpdateImport {
                            import_path: "acme/provider/types.proto".into(),
                            to_commit: provider.new_commit.clone(),
                            to_tag: String::new(),
                            remove: false,
                        },
                    )),
                },
            )),
        })
        .await
        .expect("pin provider import")
        .into_inner();
    clients
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "provider".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "follow-advance-provider".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "types.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::AddField(
                        pb::ProtoAddField {
                            message_name: "Shared".into(),
                            field_name: "current".into(),
                            field_type: "string".into(),
                            field_number: 2,
                            repeated: false,
                            doc_comment: String::new(),
                        },
                    )),
                },
            )),
        })
        .await
        .expect("advance provider after pin");

    // Act
    let followed = clients
        .explore
        .follow_type(pb::FollowTypeRequest {
            project: "billing".into(),
            repo: "consumer".into(),
            schema_path: "invoice.proto".into(),
            declaration_name: "Invoice".into(),
            field_name: "shared".into(),
            at: Some(common::vref_branch("main")),
        })
        .await
        .expect("follow pinned field type")
        .into_inner();

    // Assert
    assert_eq!(followed.source_commit, pinned_source.new_commit);
    assert_eq!(followed.resolved_project, "acme");
    assert_eq!(followed.resolved_repo, "provider");
    assert_eq!(followed.resolved_schema_path, "types.proto");
    assert_eq!(followed.resolved_commit, provider.new_commit);
    assert!(followed.pinned);
    assert_eq!(followed.import_path, "acme/provider/types.proto");
    assert_eq!(
        followed
            .summary
            .as_ref()
            .map(|summary| summary.name.as_str()),
        Some("Shared")
    );
    let detail = String::from_utf8(followed.detail).expect("protobuf detail is UTF-8");
    assert!(detail.contains("legacy"), "{detail}");
    assert!(
        !detail.contains("current"),
        "pin must retain historical detail: {detail}"
    );
}

#[tokio::test]
async fn openapi_external_ref_is_snapshot_safe_across_public_dependency_apis() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let provider_v1 = r#"openapi: "3.1.0"
info: { title: Contracts, version: "1.0.0" }
paths: {}
components:
  schemas:
    Order:
      type: object
      description: v1 contract
"#;
    create_schema(
        &mut clients.schema,
        "acme",
        "openapi",
        "main",
        "contracts.yaml",
        pb::SchemaFormat::Openapi,
        provider_v1,
        "openapi-dependency-provider",
    )
    .await;
    let consumer = create_schema(
        &mut clients.schema,
        "acme",
        "openapi",
        "main",
        "apis/api.yaml",
        pb::SchemaFormat::Openapi,
        r#"openapi: "3.1.0"
info: { title: Orders, version: "1.0.0" }
paths:
  /orders:
    get:
      responses:
        '200':
          description: Success
          content:
            application/json:
              schema:
                $ref: '../contracts.yaml#/components/schemas/Order'
components:
  schemas:
    Envelope:
      type: object
      properties:
        order:
          $ref: '../contracts.yaml#/components/schemas/Order'
"#,
        "openapi-dependency-consumer",
    )
    .await;
    let provider_v2 = provider_v1.replace("v1 contract", "v2 contract");
    clients
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "openapi".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "openapi-dependency-advance-provider".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::OpenapiOp(
                pb::OpenApiMutation {
                    schema_path: "contracts.yaml".into(),
                    operation: Some(pb::open_api_mutation::Operation::PushDocument(
                        pb::OpenApiPushDocument {
                            source: provider_v2,
                        },
                    )),
                },
            )),
        })
        .await
        .expect("advance OpenAPI provider after consumer snapshot");

    // Act
    let dependencies = clients
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "acme".into(),
            repo: "openapi".into(),
            schema_path: "apis/api.yaml".into(),
            at: Some(common::vref_commit(&consumer.new_commit)),
            transitive: false,
        })
        .await
        .expect("list OpenAPI dependencies")
        .into_inner();
    let descriptors = clients
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "openapi".into(),
            schema_path: "apis/api.yaml".into(),
            at: Some(common::vref_commit(&consumer.new_commit)),
        })
        .await
        .expect("build OpenAPI descriptor closure")
        .into_inner();
    let followed = clients
        .explore
        .follow_type(pb::FollowTypeRequest {
            project: "acme".into(),
            repo: "openapi".into(),
            schema_path: "apis/api.yaml".into(),
            declaration_name: "schema:Envelope".into(),
            field_name: "order".into(),
            at: Some(common::vref_commit(&consumer.new_commit)),
        })
        .await
        .expect("follow external OpenAPI property")
        .into_inner();
    let dependents = clients
        .explore
        .list_dependents(pb::ListDependentsRequest {
            project: "acme".into(),
            repo: "openapi".into(),
            schema_path: "contracts.yaml".into(),
        })
        .await
        .expect("list OpenAPI dependents")
        .into_inner();
    let delete_error = clients
        .schema
        .delete_schema(pb::DeleteSchemaRequest {
            project: "acme".into(),
            repo: "openapi".into(),
            branch: "main".into(),
            schema_name: "contracts.yaml".into(),
            base_revision: String::new(),
            idempotency_key: "openapi-dependency-delete-provider".into(),
            force: true,
        })
        .await
        .expect_err("live OpenAPI ref must block forced provider deletion");

    // Assert
    assert_eq!(dependencies.at_commit, consumer.new_commit);
    assert_eq!(dependencies.dependencies.len(), 1);
    let dependency = &dependencies.dependencies[0];
    assert_eq!(dependency.importing_schema, "apis/api.yaml");
    assert_eq!(dependency.imported_project, "acme");
    assert_eq!(dependency.imported_repo, "openapi");
    assert_eq!(dependency.imported_schema, "contracts.yaml");
    assert_eq!(dependency.imported_decl, "schema:Order");
    assert_eq!(dependency.import_path, "../contracts.yaml");
    assert_eq!(dependency.importing_commit, consumer.new_commit);
    assert_eq!(dependency.target_commit, consumer.new_commit);
    assert!(dependency.resolved);
    assert!(!dependency.pinned);
    assert!(dependency.resolved_commit.is_empty());

    assert_eq!(descriptors.at_commit, consumer.new_commit);
    let descriptors = String::from_utf8(descriptors.descriptor_bytes)
        .expect("OpenAPI descriptor closure is UTF-8 YAML");
    assert!(descriptors.contains("v1 contract"), "{descriptors}");
    assert!(!descriptors.contains("v2 contract"), "{descriptors}");

    assert_eq!(followed.source_commit, consumer.new_commit);
    assert_eq!(followed.resolved_project, "acme");
    assert_eq!(followed.resolved_repo, "openapi");
    assert_eq!(followed.resolved_schema_path, "contracts.yaml");
    assert_eq!(followed.resolved_commit, consumer.new_commit);
    assert_eq!(followed.import_path, "../contracts.yaml");
    assert!(!followed.pinned);
    assert_eq!(
        followed
            .summary
            .as_ref()
            .map(|summary| summary.name.as_str()),
        Some("schema:Order")
    );
    let detail = String::from_utf8(followed.detail).expect("OpenAPI detail is UTF-8 YAML");
    assert!(detail.contains("v1 contract"), "{detail}");
    assert!(!detail.contains("v2 contract"), "{detail}");

    assert_eq!(dependents.dependents.len(), 1);
    assert_eq!(dependents.dependents[0].importing_schema, "apis/api.yaml");
    assert_eq!(dependents.dependents[0].imported_decl, "schema:Order");
    assert_eq!(dependents.dependents[0].import_path, "../contracts.yaml");
    assert!(!dependents.dependents[0].pinned);
    assert_eq!(delete_error.code(), Code::FailedPrecondition);
    assert!(delete_error.message().contains("apis/api.yaml"));
}
