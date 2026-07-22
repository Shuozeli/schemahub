mod common;

use std::collections::BTreeSet;

use common::*;
use schemahub_api::schemahub_v1 as pb;

fn tx_protobuf(
    schema_path: &str,
    operation: pb::protobuf_mutation::Operation,
) -> pb::TransactionOp {
    pb::TransactionOp {
        operation: Some(pb::transaction_op::Operation::ProtobufOp(
            pb::ProtobufMutation {
                schema_path: schema_path.to_string(),
                operation: Some(operation),
            },
        )),
    }
}

fn tx_flatbuffers(
    schema_path: &str,
    operation: pb::flat_buffers_mutation::Operation,
) -> pb::TransactionOp {
    pb::TransactionOp {
        operation: Some(pb::transaction_op::Operation::FbsOp(
            pb::FlatBuffersMutation {
                schema_path: schema_path.to_string(),
                operation: Some(operation),
            },
        )),
    }
}

fn tx_openapi(schema_path: &str, operation: pb::open_api_mutation::Operation) -> pb::TransactionOp {
    pb::TransactionOp {
        operation: Some(pb::transaction_op::Operation::OpenapiOp(
            pb::OpenApiMutation {
                schema_path: schema_path.to_string(),
                operation: Some(operation),
            },
        )),
    }
}

async fn descriptor_artifact(
    clients: &mut Clients,
    project: &str,
    repo: &str,
    schema_path: &str,
) -> pb::SchemaArtifact {
    let revision = clients
        .serving
        .resolve_revision(pb::ResolveRevisionRequest {
            parent: format!("projects/{project}/repos/{repo}"),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("resolve immutable revision")
        .into_inner();
    clients
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: revision.name,
            schema_path: schema_path.to_string(),
            kind: pb::SchemaArtifactKind::Descriptors as i32,
            language: pb::Language::Unspecified as i32,
            rust_pluggable_buffer: false,
            if_none_match: String::new(),
        })
        .await
        .expect("fetch immutable descriptor artifact")
        .into_inner()
}

#[tokio::test]
async fn capability_matrix_is_versioned_and_matches_reachable_operations() {
    // Arrange
    let url = start_server().await;
    let channel = connect(&url).await;
    let mut admin = pb::admin_service_client::AdminServiceClient::new(channel);

    // Act
    let response = admin
        .get_format_capabilities(pb::GetFormatCapabilitiesRequest {})
        .await
        .expect("get format capabilities")
        .into_inner();

    // Assert
    assert_eq!(response.matrix_version, "1.0");
    assert_eq!(response.formats.len(), 3);
    let protobuf = response
        .formats
        .iter()
        .find(|format| format.format_id == "protobuf")
        .expect("protobuf capabilities");
    let protobuf_operations: BTreeSet<_> = protobuf
        .operations
        .iter()
        .map(|operation| operation.operation.as_str())
        .collect();
    assert_eq!(
        protobuf_operations,
        BTreeSet::from([
            "add_field",
            "remove_field",
            "rename_field",
            "change_field_type",
            "change_field_label",
            "reorder_fields",
            "add_message",
            "remove_message",
            "rename_message",
            "add_enum",
            "remove_enum",
            "add_enum_value",
            "remove_enum_value",
            "rename_enum_value",
            "add_service",
            "remove_service",
            "rename_service",
            "add_rpc",
            "remove_rpc",
            "rename_rpc",
            "change_rpc_type",
            "update_import",
        ])
    );
    assert!(protobuf.operations.iter().all(|operation| {
        operation.status == pb::CapabilityStatus::Supported as i32
            && operation.apply_mutation
            && operation.apply_transaction
    }));

    let flatbuffers = response
        .formats
        .iter()
        .find(|format| format.format_id == "flatbuffers")
        .expect("flatbuffers capabilities");
    let flatbuffers_operations: BTreeSet<_> = flatbuffers
        .operations
        .iter()
        .map(|operation| operation.operation.as_str())
        .collect();
    assert_eq!(
        flatbuffers_operations,
        BTreeSet::from([
            "add_field",
            "deprecate_field",
            "rename_field",
            "change_field_type",
            "add_table",
            "remove_table",
            "rename_table",
            "add_enum",
            "remove_enum",
            "rename_enum",
            "add_enum_value",
            "remove_enum_value",
            "rename_enum_value",
            "add_union",
            "remove_union",
            "rename_union",
            "add_union_member",
            "remove_union_member",
            "update_import",
            "remove_field",
            "reorder_fields",
        ])
    );
    for operation_name in [
        "change_field_type",
        "remove_enum",
        "rename_enum",
        "remove_enum_value",
        "rename_enum_value",
        "add_union_member",
        "remove_union_member",
        "remove_union",
        "rename_union",
    ] {
        assert!(flatbuffers.operations.iter().any(|operation| {
            operation.operation == operation_name
                && operation.status == pb::CapabilityStatus::Supported as i32
                && operation.apply_mutation
                && operation.apply_transaction
        }));
    }
    for operation_name in ["remove_field", "reorder_fields"] {
        assert!(flatbuffers.operations.iter().any(|operation| {
            operation.operation == operation_name
                && operation.status == pb::CapabilityStatus::Rejected as i32
                && !operation.apply_mutation
                && !operation.apply_transaction
        }));
    }

    let openapi = response
        .formats
        .iter()
        .find(|format| format.format_id == "openapi")
        .expect("openapi capabilities");
    let openapi_operations: BTreeSet<_> = openapi
        .operations
        .iter()
        .map(|operation| operation.operation.as_str())
        .collect();
    assert_eq!(
        openapi_operations,
        BTreeSet::from([
            "push_document",
            "add_path",
            "remove_path",
            "add_operation",
            "remove_operation",
            "add_component_schema",
            "remove_component_schema",
        ])
    );
    assert!(openapi.operations.iter().all(|operation| {
        operation.status == pb::CapabilityStatus::Supported as i32
            && operation.apply_mutation
            && operation.apply_transaction
    }));
}

#[tokio::test]
async fn protobuf_service_and_pinned_import_transaction_round_trips() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let common = create_schema(
        &mut clients.schema,
        "workspace",
        "schemas",
        "main",
        "common.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\";\nmessage Common {}\n",
        "format-proto-common",
    )
    .await;
    clients
        .refs
        .create_tag(pb::CreateTagRequest {
            project: "workspace".into(),
            repo: "schemas".into(),
            name: "common-v1".into(),
            target: Some(vref_commit(&common.new_commit)),
            message: String::new(),
        })
        .await
        .expect("create dependency tag");
    create_schema(
        &mut clients.schema,
        "workspace",
        "schemas",
        "main",
        "service.proto",
        pb::SchemaFormat::Protobuf,
        r#"syntax = "proto3";
import "legacy.proto";
message Request {}
message RequestV2 {}
message Response {}
message ResponseV2 {}
service LegacyCatalog { rpc Get(Request) returns (Response); }
"#,
        "format-proto-service",
    )
    .await;
    let operations = vec![
        tx_protobuf(
            "service.proto",
            pb::protobuf_mutation::Operation::RenameService(pb::ProtoRenameService {
                old_name: "LegacyCatalog".into(),
                new_name: "Catalog".into(),
            }),
        ),
        tx_protobuf(
            "service.proto",
            pb::protobuf_mutation::Operation::ChangeRpcType(pb::ProtoChangeRpcType {
                service_name: "Catalog".into(),
                rpc_name: "Get".into(),
                new_request_type: "RequestV2".into(),
                new_response_type: "ResponseV2".into(),
            }),
        ),
        tx_protobuf(
            "service.proto",
            pb::protobuf_mutation::Operation::UpdateImport(pb::ProtoUpdateImport {
                import_path: "legacy.proto".into(),
                to_commit: String::new(),
                to_tag: String::new(),
                remove: true,
            }),
        ),
        tx_protobuf(
            "service.proto",
            pb::protobuf_mutation::Operation::UpdateImport(pb::ProtoUpdateImport {
                import_path: "workspace/schemas/common.proto".into(),
                to_commit: String::new(),
                to_tag: "common-v1".into(),
                remove: false,
            }),
        ),
    ];

    // Act
    clients
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "workspace".into(),
            repo: "schemas".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "format-proto-transaction".into(),
            force: true,
            operations,
        })
        .await
        .expect("apply protobuf transaction");

    // Assert
    let source = pull_source(
        &mut clients.explore,
        "workspace",
        "schemas",
        "service.proto",
        vref_branch("main"),
    )
    .await;
    assert!(source.contains("service Catalog"), "{source}");
    assert!(
        source.contains("rpc Get(RequestV2) returns (ResponseV2)"),
        "{source}"
    );
    assert!(!source.contains("legacy.proto"), "{source}");
    assert!(
        source.contains("workspace/schemas/common.proto"),
        "{source}"
    );
    let dependencies = clients
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "workspace".into(),
            repo: "schemas".into(),
            schema_path: "service.proto".into(),
            at: Some(vref_branch("main")),
            transitive: false,
        })
        .await
        .expect("list dependencies")
        .into_inner()
        .dependencies;
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].imported_project, "workspace");
    assert_eq!(dependencies[0].imported_repo, "schemas");
    assert_eq!(dependencies[0].imported_schema, "common.proto");
    assert_eq!(
        dependencies[0].import_path,
        "workspace/schemas/common.proto"
    );
    assert_eq!(dependencies[0].resolved_commit, common.new_commit);
    assert_eq!(
        dependencies[0].target_commit,
        dependencies[0].resolved_commit
    );
    assert!(dependencies[0].pinned);
    assert!(dependencies[0].resolved);
}

#[tokio::test]
async fn flatbuffers_selected_workflows_apply_atomically() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "warehouse",
        "catalog",
        "main",
        "catalog.fbs",
        pb::SchemaFormat::Flatbuffers,
        r#"include "legacy.fbs";
enum Status : byte { Unknown = 0, Ready = 1, Retired = 2 }
enum Obsolete : byte { None = 0 }
table Item { id: int; status: Status = Ready; }
table Bundle { id: int; }
table Old { id: int; }
union Entity { Item }
union OldUnion { Old }
root_type Item;
"#,
        "format-fbs-base",
    )
    .await;
    let operations = vec![
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::ChangeFieldType(pb::FbsChangeFieldType {
                table_name: "Item".into(),
                field_name: "id".into(),
                new_type: "long".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::RenameEnum(pb::FbsRenameEnum {
                old_name: "Status".into(),
                new_name: "State".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::RenameEnumValue(pb::FbsRenameEnumValue {
                enum_name: "State".into(),
                old_value_name: "Ready".into(),
                new_value_name: "Active".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::RemoveEnumValue(pb::FbsRemoveEnumValue {
                enum_name: "State".into(),
                value_name: "Retired".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::RemoveEnum(pb::FbsRemoveEnum {
                enum_name: "Obsolete".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::AddUnionMember(pb::FbsAddUnionMember {
                union_name: "Entity".into(),
                member_type: "Bundle".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::RenameUnion(pb::FbsRenameUnion {
                old_name: "Entity".into(),
                new_name: "Payload".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::RemoveTable(pb::FbsRemoveTable {
                table_name: "Old".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::RemoveUnion(pb::FbsRemoveUnion {
                union_name: "OldUnion".into(),
            }),
        ),
        tx_flatbuffers(
            "catalog.fbs",
            pb::flat_buffers_mutation::Operation::UpdateImport(pb::FbsUpdateImport {
                import_path: "legacy.fbs".into(),
                to_commit: String::new(),
                to_tag: String::new(),
                remove: true,
            }),
        ),
    ];

    // Act
    clients
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "warehouse".into(),
            repo: "catalog".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "format-fbs-transaction".into(),
            force: true,
            operations,
        })
        .await
        .expect("apply flatbuffers transaction");

    // Assert
    let source = pull_source(
        &mut clients.explore,
        "warehouse",
        "catalog",
        "catalog.fbs",
        vref_branch("main"),
    )
    .await;
    assert!(source.contains("id: long"), "{source}");
    assert!(source.contains("status: State = Active"), "{source}");
    assert!(source.contains("union Payload"), "{source}");
    assert!(source.contains("Bundle"), "{source}");
    assert!(!source.contains("Obsolete"), "{source}");
    assert!(!source.contains("OldUnion"), "{source}");
    assert!(!source.contains("table Old"), "{source}");
    assert!(!source.contains("legacy.fbs"), "{source}");
}

#[tokio::test]
async fn flatbuffers_union_member_removal_is_reachable_through_api() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "warehouse",
        "events",
        "main",
        "event.fbs",
        pb::SchemaFormat::Flatbuffers,
        "table A { id: int; }\ntable B { id: int; }\nunion Event { A, B }\nroot_type A;\n",
        "format-fbs-remove-member-base",
    )
    .await;
    let operation = pb::apply_mutation_request::Operation::FbsOp(pb::FlatBuffersMutation {
        schema_path: "event.fbs".into(),
        operation: Some(pb::flat_buffers_mutation::Operation::RemoveUnionMember(
            pb::FbsRemoveUnionMember {
                union_name: "Event".into(),
                member_type: "B".into(),
            },
        )),
    });

    // Act
    clients
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "warehouse".into(),
            repo: "events".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "format-fbs-remove-member".into(),
            force: true,
            operation: Some(operation),
        })
        .await
        .expect("remove union member");

    // Assert
    let source = pull_source(
        &mut clients.explore,
        "warehouse",
        "events",
        "event.fbs",
        vref_branch("main"),
    )
    .await;
    let union = source
        .split("union Event")
        .nth(1)
        .and_then(|remainder| remainder.split('}').next())
        .expect("Event union body");
    assert!(union.contains('A'), "{source}");
    assert!(!union.contains('B'), "{source}");
}

#[tokio::test]
async fn openapi_declared_operations_are_transactional() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "commerce",
        "api",
        "main",
        "openapi.yaml",
        pb::SchemaFormat::Openapi,
        r#"openapi: "3.1.0"
info:
  title: Commerce
  version: "1.0"
paths: {}
"#,
        "format-openapi-base",
    )
    .await;
    let operations = vec![
        tx_openapi(
            "openapi.yaml",
            pb::open_api_mutation::Operation::AddComponentSchema(pb::OpenApiAddComponentSchema {
                schema_name: "Order".into(),
                schema_type: "object".into(),
                description: "An order".into(),
            }),
        ),
        tx_openapi(
            "openapi.yaml",
            pb::open_api_mutation::Operation::AddPath(pb::OpenApiAddPath {
                path_pattern: "/orders".into(),
                summary: "Orders".into(),
                description: String::new(),
            }),
        ),
        tx_openapi(
            "openapi.yaml",
            pb::open_api_mutation::Operation::AddOperation(pb::OpenApiAddOperation {
                path_pattern: "/orders".into(),
                method: "get".into(),
                operation_id: "listOrders".into(),
                summary: "List orders".into(),
                description: String::new(),
            }),
        ),
    ];

    // Act
    clients
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "commerce".into(),
            repo: "api".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "format-openapi-transaction".into(),
            force: true,
            operations,
        })
        .await
        .expect("apply OpenAPI transaction");

    // Assert
    let source = pull_source(
        &mut clients.explore,
        "commerce",
        "api",
        "openapi.yaml",
        vref_branch("main"),
    )
    .await;
    assert!(source.contains("/orders"), "{source}");
    assert!(source.contains("operationId: listOrders"), "{source}");
    assert!(source.contains("Order:"), "{source}");
}

#[tokio::test]
async fn every_advertised_protobuf_operation_round_trips_and_serves_descriptors() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "matrix",
        "protobuf",
        "main",
        "all.proto",
        pb::SchemaFormat::Protobuf,
        r#"syntax = "proto3";
import "legacy.proto";
message Primary { string first = 1; int32 count = 2; string obsolete = 3; }
message RemoveMessage {}
message RenameMessage {}
message RpcRequest {}
message RpcRequestV2 {}
message RpcResponse {}
message RpcResponseV2 {}
enum State { STATE_UNSPECIFIED = 0; STATE_OLD = 1; STATE_REMOVE = 2; }
enum RemoveEnum { REMOVE_UNSPECIFIED = 0; }
service MainService {
  rpc RenameRpc(RpcRequest) returns (RpcResponse);
  rpc RemoveRpc(RpcRequest) returns (RpcResponse);
  rpc ChangeRpc(RpcRequest) returns (RpcResponse);
}
service RemoveService {}
service RenameService {}
"#,
        "format-protobuf-all-base",
    )
    .await;
    let operations = vec![
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::AddField(pb::ProtoAddField {
                message_name: "Primary".into(),
                field_name: "added".into(),
                field_type: "string".into(),
                field_number: 4,
                repeated: false,
                doc_comment: String::new(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RemoveField(pb::ProtoRemoveField {
                message_name: "Primary".into(),
                field_name: "obsolete".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RenameField(pb::ProtoRenameField {
                message_name: "Primary".into(),
                old_field_name: "first".into(),
                new_field_name: "renamed".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::ChangeFieldType(pb::ProtoChangeFieldType {
                message_name: "Primary".into(),
                field_name: "count".into(),
                new_type: "int64".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::ChangeFieldLabel(pb::ProtoChangeFieldLabel {
                message_name: "Primary".into(),
                field_name: "added".into(),
                new_label: "repeated".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::ReorderFields(pb::ProtoReorderFields {
                message_name: "Primary".into(),
                field_order: vec!["renamed".into(), "count".into(), "added".into()],
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::AddMessage(pb::ProtoAddMessage {
                message_name: "AddedMessage".into(),
                doc_comment: String::new(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RemoveMessage(pb::ProtoRemoveMessage {
                message_name: "RemoveMessage".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RenameMessage(pb::ProtoRenameMessage {
                old_name: "RenameMessage".into(),
                new_name: "RenamedMessage".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::AddEnum(pb::ProtoAddEnum {
                enum_name: "AddedEnum".into(),
                doc_comment: String::new(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RemoveEnum(pb::ProtoRemoveEnum {
                enum_name: "RemoveEnum".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::AddEnumValue(pb::ProtoAddEnumValue {
                enum_name: "AddedEnum".into(),
                value_name: "ADDED_UNSPECIFIED".into(),
                number: 0,
                doc_comment: String::new(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RemoveEnumValue(pb::ProtoRemoveEnumValue {
                enum_name: "State".into(),
                value_name: "STATE_REMOVE".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RenameEnumValue(pb::ProtoRenameEnumValue {
                enum_name: "State".into(),
                old_value_name: "STATE_OLD".into(),
                new_value_name: "STATE_ACTIVE".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::AddService(pb::ProtoAddService {
                service_name: "AddedService".into(),
                doc_comment: String::new(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RemoveService(pb::ProtoRemoveService {
                service_name: "RemoveService".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RenameService(pb::ProtoRenameService {
                old_name: "RenameService".into(),
                new_name: "RenamedService".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::AddRpc(pb::ProtoAddRpc {
                service_name: "AddedService".into(),
                rpc_name: "AddedRpc".into(),
                request_type: "RpcRequest".into(),
                response_type: "RpcResponse".into(),
                client_streaming: false,
                server_streaming: false,
                doc_comment: String::new(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RemoveRpc(pb::ProtoRemoveRpc {
                service_name: "MainService".into(),
                rpc_name: "RemoveRpc".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::RenameRpc(pb::ProtoRenameRpc {
                service_name: "MainService".into(),
                old_rpc_name: "RenameRpc".into(),
                new_rpc_name: "RenamedRpc".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::ChangeRpcType(pb::ProtoChangeRpcType {
                service_name: "MainService".into(),
                rpc_name: "ChangeRpc".into(),
                new_request_type: "RpcRequestV2".into(),
                new_response_type: "RpcResponseV2".into(),
            }),
        ),
        tx_protobuf(
            "all.proto",
            pb::protobuf_mutation::Operation::UpdateImport(pb::ProtoUpdateImport {
                import_path: "legacy.proto".into(),
                to_commit: String::new(),
                to_tag: String::new(),
                remove: true,
            }),
        ),
    ];

    // Act
    clients
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "matrix".into(),
            repo: "protobuf".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "format-protobuf-all-ops".into(),
            force: true,
            operations,
        })
        .await
        .expect("all advertised protobuf operations are reachable");

    // Assert
    let source = pull_source(
        &mut clients.explore,
        "matrix",
        "protobuf",
        "all.proto",
        vref_branch("main"),
    )
    .await;
    for expected in [
        "repeated string added = 4",
        "int64 count = 2",
        "message AddedMessage",
        "message RenamedMessage",
        "enum AddedEnum",
        "STATE_ACTIVE = 1",
        "service AddedService",
        "service RenamedService",
        "rpc RenamedRpc",
        "rpc ChangeRpc(RpcRequestV2) returns (RpcResponseV2)",
    ] {
        assert!(
            source.contains(expected),
            "missing {expected:?} in:\n{source}"
        );
    }
    for removed in [
        "obsolete = 3",
        "message RemoveMessage",
        "message RenameMessage",
        "enum RemoveEnum",
        "STATE_REMOVE = 2",
        "service RemoveService",
        "service RenameService",
        "rpc RemoveRpc",
        "legacy.proto",
    ] {
        assert!(!source.contains(removed), "found {removed:?} in:\n{source}");
    }
    let artifact = descriptor_artifact(&mut clients, "matrix", "protobuf", "all.proto").await;
    assert_eq!(artifact.format, pb::SchemaFormat::Protobuf as i32);
    assert!(!artifact.content.is_empty());
    assert!(artifact.artifact_digest.starts_with("sha256:"));
}

#[tokio::test]
async fn every_advertised_flatbuffers_operation_round_trips_and_serves_descriptors() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "matrix",
        "flatbuffers",
        "main",
        "all.fbs",
        pb::SchemaFormat::Flatbuffers,
        r#"include "legacy.fbs";
enum State : byte { Unknown = 0, Old = 1, Remove = 2 }
enum RemoveEnum : byte { None = 0 }
enum RenameEnum : byte { None = 0 }
table Primary { old_field: int; rename_field: int; change_field: int; }
table RemoveTable { id: int; }
table RenameTable { id: int; }
table MemberA { id: int; }
table MemberB { id: int; }
table MemberC { id: int; }
union ExistingUnion { MemberA, MemberB }
union RemoveUnion { MemberA }
union RenameUnion { MemberA }
root_type Primary;
"#,
        "format-flatbuffers-all-base",
    )
    .await;
    let operations = vec![
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::AddField(pb::FbsAddField {
                table_name: "Primary".into(),
                field_name: "added".into(),
                field_type: "string".into(),
                default_value: String::new(),
                doc_comment: String::new(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::DeprecateField(pb::FbsDeprecateField {
                table_name: "Primary".into(),
                field_name: "old_field".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RenameField(pb::FbsRenameField {
                table_name: "Primary".into(),
                old_field_name: "rename_field".into(),
                new_field_name: "renamed_field".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::ChangeFieldType(pb::FbsChangeFieldType {
                table_name: "Primary".into(),
                field_name: "change_field".into(),
                new_type: "long".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::AddTable(pb::FbsAddTable {
                table_name: "AddedTable".into(),
                doc_comment: String::new(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RemoveTable(pb::FbsRemoveTable {
                table_name: "RemoveTable".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RenameTable(pb::FbsRenameTable {
                old_name: "RenameTable".into(),
                new_name: "RenamedTable".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::AddEnum(pb::FbsAddEnum {
                enum_name: "AddedEnum".into(),
                base_type: "byte".into(),
                doc_comment: String::new(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RemoveEnum(pb::FbsRemoveEnum {
                enum_name: "RemoveEnum".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RenameEnum(pb::FbsRenameEnum {
                old_name: "RenameEnum".into(),
                new_name: "RenamedEnum".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::AddEnumValue(pb::FbsAddEnumValue {
                enum_name: "AddedEnum".into(),
                value_name: "Zero".into(),
                value: 0,
                doc_comment: String::new(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RemoveEnumValue(pb::FbsRemoveEnumValue {
                enum_name: "State".into(),
                value_name: "Remove".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RenameEnumValue(pb::FbsRenameEnumValue {
                enum_name: "State".into(),
                old_value_name: "Old".into(),
                new_value_name: "Active".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::AddUnion(pb::FbsAddUnion {
                union_name: "AddedUnion".into(),
                member_types: vec!["MemberA".into()],
                doc_comment: String::new(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RemoveUnion(pb::FbsRemoveUnion {
                union_name: "RemoveUnion".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RenameUnion(pb::FbsRenameUnion {
                old_name: "RenameUnion".into(),
                new_name: "RenamedUnion".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::AddUnionMember(pb::FbsAddUnionMember {
                union_name: "ExistingUnion".into(),
                member_type: "MemberC".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::RemoveUnionMember(pb::FbsRemoveUnionMember {
                union_name: "ExistingUnion".into(),
                member_type: "MemberB".into(),
            }),
        ),
        tx_flatbuffers(
            "all.fbs",
            pb::flat_buffers_mutation::Operation::UpdateImport(pb::FbsUpdateImport {
                import_path: "legacy.fbs".into(),
                to_commit: String::new(),
                to_tag: String::new(),
                remove: true,
            }),
        ),
    ];

    // Act
    clients
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "matrix".into(),
            repo: "flatbuffers".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "format-flatbuffers-all-ops".into(),
            force: true,
            operations,
        })
        .await
        .expect("all advertised flatbuffers operations are reachable");

    // Assert
    let source = pull_source(
        &mut clients.explore,
        "matrix",
        "flatbuffers",
        "all.fbs",
        vref_branch("main"),
    )
    .await;
    for expected in [
        "old_field: int (deprecated)",
        "renamed_field: int",
        "change_field: long",
        "added: string",
        "table AddedTable",
        "table RenamedTable",
        "enum AddedEnum",
        "enum RenamedEnum",
        "Active = 1",
        "union AddedUnion",
        "union RenamedUnion",
        "MemberC",
    ] {
        assert!(
            source.contains(expected),
            "missing {expected:?} in:\n{source}"
        );
    }
    for removed in [
        "table RemoveTable",
        "table RenameTable",
        "enum RemoveEnum",
        "enum RenameEnum",
        "Remove = 2",
        "union RemoveUnion",
        "union RenameUnion",
        "legacy.fbs",
    ] {
        assert!(!source.contains(removed), "found {removed:?} in:\n{source}");
    }
    let existing_union = source
        .split("union ExistingUnion")
        .nth(1)
        .and_then(|remainder| remainder.split('}').next())
        .expect("ExistingUnion body");
    assert!(existing_union.contains("MemberA"), "{source}");
    assert!(existing_union.contains("MemberC"), "{source}");
    assert!(!existing_union.contains("MemberB"), "{source}");
    let artifact = descriptor_artifact(&mut clients, "matrix", "flatbuffers", "all.fbs").await;
    assert_eq!(artifact.format, pb::SchemaFormat::Flatbuffers as i32);
    assert!(!artifact.content.is_empty());
    assert!(artifact.artifact_digest.starts_with("sha256:"));
}

#[tokio::test]
async fn every_advertised_openapi_operation_round_trips_and_serves_descriptors() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "matrix",
        "openapi",
        "main",
        "all.yaml",
        pb::SchemaFormat::Openapi,
        r#"openapi: "3.1.0"
info:
  title: Matrix
  version: "1.0"
paths: {}
"#,
        "format-openapi-all-base",
    )
    .await;
    let replacement = r#"openapi: "3.1.0"
info:
  title: Matrix Final
  version: "2.0"
paths:
  /final:
    get:
      operationId: getFinal
      responses:
        "204":
          description: No content
"#;
    let operations = vec![
        tx_openapi(
            "all.yaml",
            pb::open_api_mutation::Operation::AddComponentSchema(pb::OpenApiAddComponentSchema {
                schema_name: "Temporary".into(),
                schema_type: "object".into(),
                description: String::new(),
            }),
        ),
        tx_openapi(
            "all.yaml",
            pb::open_api_mutation::Operation::AddPath(pb::OpenApiAddPath {
                path_pattern: "/temporary".into(),
                summary: String::new(),
                description: String::new(),
            }),
        ),
        tx_openapi(
            "all.yaml",
            pb::open_api_mutation::Operation::AddOperation(pb::OpenApiAddOperation {
                path_pattern: "/temporary".into(),
                method: "get".into(),
                operation_id: "getTemporary".into(),
                summary: String::new(),
                description: String::new(),
            }),
        ),
        tx_openapi(
            "all.yaml",
            pb::open_api_mutation::Operation::RemoveOperation(pb::OpenApiRemoveOperation {
                path_pattern: "/temporary".into(),
                method: "get".into(),
            }),
        ),
        tx_openapi(
            "all.yaml",
            pb::open_api_mutation::Operation::RemovePath(pb::OpenApiRemovePath {
                path_pattern: "/temporary".into(),
            }),
        ),
        tx_openapi(
            "all.yaml",
            pb::open_api_mutation::Operation::RemoveComponentSchema(
                pb::OpenApiRemoveComponentSchema {
                    schema_name: "Temporary".into(),
                },
            ),
        ),
        tx_openapi(
            "all.yaml",
            pb::open_api_mutation::Operation::PushDocument(pb::OpenApiPushDocument {
                source: replacement.into(),
            }),
        ),
    ];

    // Act
    clients
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "matrix".into(),
            repo: "openapi".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "format-openapi-all-ops".into(),
            force: true,
            operations,
        })
        .await
        .expect("all advertised OpenAPI operations are reachable");

    // Assert
    let source = pull_source(
        &mut clients.explore,
        "matrix",
        "openapi",
        "all.yaml",
        vref_branch("main"),
    )
    .await;
    assert!(source.contains("title: Matrix Final"), "{source}");
    assert!(source.contains("/final:"), "{source}");
    assert!(source.contains("operationId: getFinal"), "{source}");
    assert!(!source.contains("/temporary"), "{source}");
    assert!(!source.contains("Temporary:"), "{source}");
    let artifact = descriptor_artifact(&mut clients, "matrix", "openapi", "all.yaml").await;
    assert_eq!(artifact.format, pb::SchemaFormat::Openapi as i32);
    assert!(!artifact.content.is_empty());
    assert!(artifact.artifact_digest.starts_with("sha256:"));
}
