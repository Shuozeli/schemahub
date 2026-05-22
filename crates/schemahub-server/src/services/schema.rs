use std::sync::Arc;

use bytes::Bytes;
use prost::Message;
use schemahub_core::{Core, MutateRequest};
use schemahub_types::{Mutation, SchemaPath};
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    ApplyMutationRequest, ApplyMutationResponse, ApplyTransactionRequest,
    ApplyTransactionResponse, CreateSchemaRequest, CreateSchemaResponse, DeleteSchemaRequest,
    DeleteSchemaResponse, FlatBuffersMutation, ProtobufMutation,
    UpdateSchemaRequest, UpdateSchemaResponse,
    apply_mutation_request::Operation as MutationOperation,
    schema_service_server::SchemaService,
    transaction_op::Operation as TxOp,
};
use schemahub_core::mutation::batch::BatchMutateRequest;

use schemahub_plugin_protobuf::operations::{
    self as proto_ops, ProtoOperationEnvelope,
};
use schemahub_plugin_flatbuffers::operations::{
    self as fbs_ops, FbsOperationEnvelope,
};

use crate::error::core_to_status;

pub struct SchemaServiceImpl {
    core: Arc<Core>,
}

impl SchemaServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

/// Translate a `ProtobufMutation` API proto into `ProtoOperationEnvelope` bytes
/// suitable for `Mutation.operation`.
fn proto_to_envelope(m: &ProtobufMutation) -> Result<Bytes, Status> {
    use schemahub_api::schemahub_v1::protobuf_mutation::Operation as ApiOp;

    let (tag, payload) = match &m.operation {
        Some(ApiOp::AddField(o)) => {
            let inner = proto_ops::OpAddField {
                field_name: o.field_name.clone(),
                field_type: o.field_type.clone(),
                field_number: o.field_number,
                repeated: o.repeated,
                doc_comment: o.doc_comment.clone(),
            };
            (proto_ops::op_tag::ADD_FIELD, inner.encode_to_vec())
        }
        Some(ApiOp::RemoveField(o)) => {
            let inner = proto_ops::OpRemoveField {
                field_name: o.field_name.clone(),
            };
            (proto_ops::op_tag::REMOVE_FIELD, inner.encode_to_vec())
        }
        Some(ApiOp::RenameField(o)) => {
            let inner = proto_ops::OpRenameField {
                old_field_name: o.old_field_name.clone(),
                new_field_name: o.new_field_name.clone(),
            };
            (proto_ops::op_tag::RENAME_FIELD, inner.encode_to_vec())
        }
        Some(ApiOp::ChangeFieldType(o)) => {
            let inner = proto_ops::OpChangeFieldType {
                field_name: o.field_name.clone(),
                new_type: o.new_type.clone(),
            };
            (proto_ops::op_tag::CHANGE_FIELD_TYPE, inner.encode_to_vec())
        }
        Some(ApiOp::ChangeFieldLabel(o)) => {
            let inner = proto_ops::OpChangeFieldLabel {
                field_name: o.field_name.clone(),
                new_label: o.new_label.clone(),
            };
            (proto_ops::op_tag::CHANGE_FIELD_LABEL, inner.encode_to_vec())
        }
        Some(ApiOp::ReorderFields(o)) => {
            let inner = proto_ops::OpReorderFields {
                field_order: o.field_order.clone(),
            };
            (proto_ops::op_tag::REORDER_FIELDS, inner.encode_to_vec())
        }
        Some(ApiOp::AddMessage(o)) => {
            let inner = proto_ops::OpAddMessage {
                message_name: o.message_name.clone(),
                doc_comment: o.doc_comment.clone(),
            };
            (proto_ops::op_tag::ADD_MESSAGE, inner.encode_to_vec())
        }
        Some(ApiOp::RemoveMessage(o)) => {
            let inner = proto_ops::OpRemoveMessage {
                message_name: o.message_name.clone(),
            };
            (proto_ops::op_tag::REMOVE_MESSAGE, inner.encode_to_vec())
        }
        Some(ApiOp::RenameMessage(o)) => {
            let inner = proto_ops::OpRenameMessage {
                old_name: o.old_name.clone(),
                new_name: o.new_name.clone(),
            };
            (proto_ops::op_tag::RENAME_MESSAGE, inner.encode_to_vec())
        }
        Some(ApiOp::AddEnum(o)) => {
            let inner = proto_ops::OpAddEnum {
                enum_name: o.enum_name.clone(),
                doc_comment: o.doc_comment.clone(),
            };
            (proto_ops::op_tag::ADD_ENUM, inner.encode_to_vec())
        }
        Some(ApiOp::AddEnumValue(o)) => {
            let inner = proto_ops::OpAddEnumValue {
                enum_name: o.enum_name.clone(),
                value_name: o.value_name.clone(),
                number: o.number,
                doc_comment: o.doc_comment.clone(),
            };
            (proto_ops::op_tag::ADD_ENUM_VALUE, inner.encode_to_vec())
        }
        Some(ApiOp::RemoveEnum(o)) => {
            let inner = proto_ops::OpRemoveEnum {
                enum_name: o.enum_name.clone(),
            };
            (proto_ops::op_tag::REMOVE_ENUM, inner.encode_to_vec())
        }
        Some(ApiOp::AddService(o)) => {
            let inner = proto_ops::OpAddService {
                service_name: o.service_name.clone(),
                doc_comment: o.doc_comment.clone(),
            };
            (proto_ops::op_tag::ADD_SERVICE, inner.encode_to_vec())
        }
        Some(ApiOp::RemoveService(o)) => {
            let inner = proto_ops::OpRemoveService {
                service_name: o.service_name.clone(),
            };
            (proto_ops::op_tag::REMOVE_SERVICE, inner.encode_to_vec())
        }
        Some(ApiOp::AddRpc(o)) => {
            // service_name flows into mutation.declaration_name via extract_proto_decl_name
            let inner = proto_ops::OpAddRpc {
                rpc_name: o.rpc_name.clone(),
                request_type: o.request_type.clone(),
                response_type: o.response_type.clone(),
                client_streaming: o.client_streaming,
                server_streaming: o.server_streaming,
                doc_comment: o.doc_comment.clone(),
            };
            (proto_ops::op_tag::ADD_RPC, inner.encode_to_vec())
        }
        Some(ApiOp::RemoveRpc(o)) => {
            // service_name flows into mutation.declaration_name via extract_proto_decl_name
            let inner = proto_ops::OpRemoveRpc {
                rpc_name: o.rpc_name.clone(),
            };
            (proto_ops::op_tag::REMOVE_RPC, inner.encode_to_vec())
        }
        Some(ApiOp::RemoveEnumValue(_))
        | Some(ApiOp::RenameEnumValue(_))
        | Some(ApiOp::RenameRpc(_))
        | Some(ApiOp::UpdateImport(_)) => {
            return Err(Status::unimplemented(
                "this protobuf mutation operation is not yet implemented",
            ));
        }
        None => {
            return Err(Status::invalid_argument(
                "ProtobufMutation: operation oneof must be set",
            ));
        }
    };

    Ok(Bytes::from(ProtoOperationEnvelope::encode_op(tag, payload)))
}

/// Translate a `FlatBuffersMutation` API proto into `FbsOperationEnvelope` bytes.
fn fbs_to_envelope(m: &FlatBuffersMutation) -> Result<Bytes, Status> {
    use schemahub_api::schemahub_v1::flat_buffers_mutation::Operation as ApiOp;

    let (tag, payload) = match &m.operation {
        Some(ApiOp::AddField(o)) => {
            let inner = fbs_ops::OpAddField {
                field_name: o.field_name.clone(),
                field_type: o.field_type.clone(),
                default_value: o.default_value.clone(),
                doc_comment: o.doc_comment.clone(),
            };
            (fbs_ops::op_tag::ADD_FIELD, inner.encode_to_vec())
        }
        Some(ApiOp::DeprecateField(o)) => {
            let inner = fbs_ops::OpDeprecateField {
                field_name: o.field_name.clone(),
            };
            (fbs_ops::op_tag::DEPRECATE_FIELD, inner.encode_to_vec())
        }
        Some(ApiOp::RenameField(o)) => {
            let inner = fbs_ops::OpRenameField {
                old_field_name: o.old_field_name.clone(),
                new_field_name: o.new_field_name.clone(),
            };
            (fbs_ops::op_tag::RENAME_FIELD, inner.encode_to_vec())
        }
        Some(ApiOp::AddTable(o)) => {
            let inner = fbs_ops::OpAddTable {
                table_name: o.table_name.clone(),
                doc_comment: o.doc_comment.clone(),
            };
            (fbs_ops::op_tag::ADD_TABLE, inner.encode_to_vec())
        }
        Some(ApiOp::RemoveTable(o)) => {
            let inner = fbs_ops::OpRemoveTable {
                table_name: o.table_name.clone(),
            };
            (fbs_ops::op_tag::REMOVE_TABLE, inner.encode_to_vec())
        }
        Some(ApiOp::RenameTable(o)) => {
            let inner = fbs_ops::OpRenameTable {
                old_name: o.old_name.clone(),
                new_name: o.new_name.clone(),
            };
            (fbs_ops::op_tag::RENAME_TABLE, inner.encode_to_vec())
        }
        Some(ApiOp::AddEnum(o)) => {
            let inner = fbs_ops::OpAddEnum {
                enum_name: o.enum_name.clone(),
                base_type: o.base_type.clone(),
                doc_comment: o.doc_comment.clone(),
            };
            (fbs_ops::op_tag::ADD_ENUM, inner.encode_to_vec())
        }
        Some(ApiOp::AddEnumValue(o)) => {
            let inner = fbs_ops::OpAddEnumValue {
                enum_name: o.enum_name.clone(),
                value_name: o.value_name.clone(),
                value: o.value,
                doc_comment: o.doc_comment.clone(),
            };
            (fbs_ops::op_tag::ADD_ENUM_VALUE, inner.encode_to_vec())
        }
        Some(ApiOp::AddUnion(o)) => {
            let inner = fbs_ops::OpAddUnion {
                union_name: o.union_name.clone(),
                members: o.member_types.clone(),
                doc_comment: o.doc_comment.clone(),
            };
            (fbs_ops::op_tag::ADD_UNION, inner.encode_to_vec())
        }
        Some(ApiOp::UpdateImport(o)) => {
            // to_commit / to_tag are pinning hints not yet modelled in the plugin;
            // forward the import path with remove=false (add/update semantics).
            let inner = fbs_ops::OpUpdateImport {
                import_path: o.import_path.clone(),
                remove: false,
            };
            (fbs_ops::op_tag::UPDATE_IMPORT, inner.encode_to_vec())
        }
        None => {
            return Err(Status::invalid_argument(
                "FlatBuffersMutation: operation oneof must be set",
            ));
        }
    };

    Ok(Bytes::from(FbsOperationEnvelope::encode_op(tag, payload)))
}

/// Build a MutateRequest from its constituent parts.
fn make_mutate_request(
    project: String,
    repo: String,
    branch: String,
    base_revision: String,
    idempotency_key: String,
    force: bool,
    mutation: Mutation,
) -> MutateRequest {
    MutateRequest {
        project,
        repo,
        branch,
        base_revision,
        idempotency_key,
        force,
        mutation,
        token: None,
        author: "schemahub-server".to_string(),
    }
}

#[tonic::async_trait]
impl SchemaService for SchemaServiceImpl {
    async fn create_schema(
        &self,
        request: Request<CreateSchemaRequest>,
    ) -> Result<Response<CreateSchemaResponse>, Status> {
        let req = request.into_inner();

        // Map SchemaFormat to format_id string.
        let format_id = schema_format_to_id(req.format)?;

        let new_commit = self.core
            .create_schema(
                &req.project,
                &req.repo,
                &req.branch,
                &req.schema_name,
                req.source.as_bytes(),
                &format_id,
                &req.base_revision,
                &req.idempotency_key,
                "schemahub-server",
                None,
            )
            .map_err(core_to_status)?;

        Ok(Response::new(CreateSchemaResponse { new_commit }))
    }

    async fn update_schema(
        &self,
        request: Request<UpdateSchemaRequest>,
    ) -> Result<Response<UpdateSchemaResponse>, Status> {
        let req = request.into_inner();

        let new_commit = self.core
            .update_schema(
                &req.project,
                &req.repo,
                &req.branch,
                &req.schema_name,
                req.source.as_bytes(),
                &req.base_revision,
                &req.idempotency_key,
                req.force,
                "schemahub-server",
                None,
            )
            .map_err(core_to_status)?;

        Ok(Response::new(UpdateSchemaResponse { new_commit }))
    }

    async fn delete_schema(
        &self,
        request: Request<DeleteSchemaRequest>,
    ) -> Result<Response<DeleteSchemaResponse>, Status> {
        let req = request.into_inner();

        let new_commit = self.core
            .delete_schema(
                &req.project,
                &req.repo,
                &req.branch,
                &req.schema_name,
                &req.base_revision,
                &req.idempotency_key,
                req.force,
                "schemahub-server",
                None,
            )
            .map_err(core_to_status)?;

        Ok(Response::new(DeleteSchemaResponse { new_commit }))
    }

    async fn apply_mutation(
        &self,
        request: Request<ApplyMutationRequest>,
    ) -> Result<Response<ApplyMutationResponse>, Status> {
        let req = request.into_inner();

        // Determine format_id and translate the API-level operation to internal envelope bytes.
        let (format_id, schema_path_str, operation_bytes) = match req.operation {
            Some(MutationOperation::ProtobufOp(ref proto_mut)) => {
                let schema_path = proto_mut.schema_path.clone();
                let bytes = proto_to_envelope(proto_mut)?;
                ("protobuf".to_string(), schema_path, bytes)
            }
            Some(MutationOperation::FbsOp(ref fbs_mut)) => {
                let schema_path = fbs_mut.schema_path.clone();
                let bytes = fbs_to_envelope(fbs_mut)?;
                ("flatbuffers".to_string(), schema_path, bytes)
            }
            Some(MutationOperation::OpenapiOp(_)) => {
                return Err(Status::unimplemented(
                    "OpenAPI granular mutations are not yet supported; use UpdateSchema instead",
                ));
            }
            None => {
                return Err(Status::invalid_argument(
                    "ApplyMutationRequest: operation oneof must be set",
                ));
            }
        };

        // Extract the declaration name from the inner operation.
        let declaration_name = extract_declaration_name(&req.operation);

        let mutation = Mutation {
            schema_path: SchemaPath::new(req.project.clone(), req.repo.clone(), schema_path_str),
            format_id,
            declaration_name,
            operation: operation_bytes,
        };

        let mutate_req = make_mutate_request(
            req.project,
            req.repo,
            req.branch,
            req.base_revision,
            req.idempotency_key,
            req.force,
            mutation,
        );

        let new_commit = self.core.apply_mutation(mutate_req).map_err(core_to_status)?;
        Ok(Response::new(ApplyMutationResponse { new_commit }))
    }

    async fn apply_transaction(
        &self,
        request: Request<ApplyTransactionRequest>,
    ) -> Result<Response<ApplyTransactionResponse>, Status> {
        let req = request.into_inner();

        if req.operations.is_empty() {
            return Err(Status::invalid_argument(
                "ApplyTransactionRequest: operations must not be empty",
            ));
        }

        let mut mutations: Vec<Mutation> = Vec::new();

        for tx_op in req.operations {
            let mutation = match tx_op.operation {
                Some(TxOp::ProtobufOp(ref proto_mut)) => {
                    let schema_path_str = proto_mut.schema_path.clone();
                    let decl_name = extract_proto_decl_name(proto_mut);
                    let bytes = proto_to_envelope(proto_mut)?;
                    Mutation {
                        schema_path: SchemaPath::new(
                            req.project.clone(),
                            req.repo.clone(),
                            schema_path_str,
                        ),
                        format_id: "protobuf".to_string(),
                        declaration_name: decl_name,
                        operation: bytes,
                    }
                }
                Some(TxOp::FbsOp(ref fbs_mut)) => {
                    let schema_path_str = fbs_mut.schema_path.clone();
                    let decl_name = extract_fbs_decl_name(fbs_mut);
                    let bytes = fbs_to_envelope(fbs_mut)?;
                    Mutation {
                        schema_path: SchemaPath::new(
                            req.project.clone(),
                            req.repo.clone(),
                            schema_path_str,
                        ),
                        format_id: "flatbuffers".to_string(),
                        declaration_name: decl_name,
                        operation: bytes,
                    }
                }
                None => {
                    return Err(Status::invalid_argument(
                        "TransactionOp: operation oneof must be set",
                    ));
                }
            };
            mutations.push(mutation);
        }

        let batch_req = BatchMutateRequest {
            project: req.project,
            repo: req.repo,
            branch: req.branch,
            base_revision: req.base_revision,
            idempotency_key: req.idempotency_key,
            force: req.force,
            mutations,
            token: None,
            author: "schemahub-server".to_string(),
        };

        let new_commit = self.core.apply_mutations(batch_req).map_err(core_to_status)?;
        Ok(Response::new(ApplyTransactionResponse { new_commit }))
    }
}

/// Map a SchemaFormat integer to a format_id string.
fn schema_format_to_id(format: i32) -> Result<String, Status> {
    // SchemaFormat enum values from common.proto:
    // 0 = UNSPECIFIED, 1 = PROTOBUF, 2 = FLATBUFFERS, 3 = OPENAPI
    match format {
        1 => Ok("protobuf".to_string()),
        2 => Ok("flatbuffers".to_string()),
        3 => Ok("openapi".to_string()),
        _ => Err(Status::invalid_argument(format!(
            "unknown or unspecified SchemaFormat: {format}"
        ))),
    }
}

/// Extract the primary declaration name from the mutation operation oneof.
/// For message mutations, this is the message/enum/service/table name.
/// Falls back to "__root__" when the mutation targets the whole document.
fn extract_declaration_name(
    operation: &Option<schemahub_api::schemahub_v1::apply_mutation_request::Operation>,
) -> String {
    match operation {
        Some(MutationOperation::ProtobufOp(proto_mut)) => {
            extract_proto_decl_name(proto_mut)
        }
        Some(MutationOperation::FbsOp(fbs_mut)) => {
            extract_fbs_decl_name(fbs_mut)
        }
        Some(MutationOperation::OpenapiOp(_)) => "__document__".to_string(),
        None => "__root__".to_string(),
    }
}

fn extract_proto_decl_name(m: &ProtobufMutation) -> String {
    use schemahub_api::schemahub_v1::protobuf_mutation::Operation as ProtoOp;
    match &m.operation {
        Some(ProtoOp::AddField(o)) => o.message_name.clone(),
        Some(ProtoOp::RemoveField(o)) => o.message_name.clone(),
        Some(ProtoOp::RenameField(o)) => o.message_name.clone(),
        Some(ProtoOp::ChangeFieldType(o)) => o.message_name.clone(),
        Some(ProtoOp::ChangeFieldLabel(o)) => o.message_name.clone(),
        Some(ProtoOp::ReorderFields(o)) => o.message_name.clone(),
        Some(ProtoOp::AddMessage(o)) => o.message_name.clone(),
        Some(ProtoOp::RemoveMessage(o)) => o.message_name.clone(),
        Some(ProtoOp::RenameMessage(o)) => o.old_name.clone(),
        Some(ProtoOp::AddEnum(o)) => o.enum_name.clone(),
        Some(ProtoOp::RemoveEnum(o)) => o.enum_name.clone(),
        Some(ProtoOp::AddEnumValue(o)) => o.enum_name.clone(),
        Some(ProtoOp::RemoveEnumValue(o)) => o.enum_name.clone(),
        Some(ProtoOp::RenameEnumValue(o)) => o.enum_name.clone(),
        Some(ProtoOp::AddService(o)) => o.service_name.clone(),
        Some(ProtoOp::RemoveService(o)) => o.service_name.clone(),
        Some(ProtoOp::AddRpc(o)) => o.service_name.clone(),
        Some(ProtoOp::RemoveRpc(o)) => o.service_name.clone(),
        Some(ProtoOp::RenameRpc(o)) => o.service_name.clone(),
        Some(ProtoOp::UpdateImport(_)) => "__metadata__".to_string(),
        None => "__root__".to_string(),
    }
}

fn extract_fbs_decl_name(m: &FlatBuffersMutation) -> String {
    use schemahub_api::schemahub_v1::flat_buffers_mutation::Operation as FbsOp;
    match &m.operation {
        Some(FbsOp::AddField(o)) => o.table_name.clone(),
        Some(FbsOp::DeprecateField(o)) => o.table_name.clone(),
        Some(FbsOp::RenameField(o)) => o.table_name.clone(),
        Some(FbsOp::AddTable(o)) => o.table_name.clone(),
        Some(FbsOp::RemoveTable(o)) => o.table_name.clone(),
        Some(FbsOp::RenameTable(o)) => o.old_name.clone(),
        Some(FbsOp::AddEnum(o)) => o.enum_name.clone(),
        Some(FbsOp::AddEnumValue(o)) => o.enum_name.clone(),
        Some(FbsOp::AddUnion(o)) => o.union_name.clone(),
        Some(FbsOp::UpdateImport(_)) => "__metadata__".to_string(),
        None => "__root__".to_string(),
    }
}
