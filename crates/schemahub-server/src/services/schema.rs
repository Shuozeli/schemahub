use std::sync::Arc;

use bytes::Bytes;
use prost::Message;
use schemahub_core::{Core, MutateRequest};
use schemahub_types::{Mutation, SchemaPath};
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    ApplyMutationRequest, ApplyMutationResponse, ApplyTransactionRequest,
    ApplyTransactionResponse, CreateSchemaRequest, CreateSchemaResponse, DeleteSchemaRequest,
    DeleteSchemaResponse, FlatBuffersMutation, OpenApiMutation, ProtobufMutation,
    UpdateSchemaRequest, UpdateSchemaResponse,
    apply_mutation_request::Operation as MutationOperation,
    schema_service_server::SchemaService,
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

/// Encode a prost Message to bytes (for the opaque Mutation.operation field).
fn encode_proto<M: Message>(msg: &M) -> Bytes {
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap_or_default();
    Bytes::from(buf)
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

        // Determine format_id and encode the format-specific operation as bytes.
        let (format_id, schema_path_str, operation_bytes) = match req.operation {
            Some(MutationOperation::ProtobufOp(ref proto_mut)) => {
                let schema_path = proto_mut.schema_path.clone();
                let bytes = encode_proto(proto_mut);
                ("protobuf".to_string(), schema_path, bytes)
            }
            Some(MutationOperation::FbsOp(ref fbs_mut)) => {
                let schema_path = fbs_mut.schema_path.clone();
                let bytes = encode_proto(fbs_mut);
                ("flatbuffers".to_string(), schema_path, bytes)
            }
            Some(MutationOperation::OpenapiOp(ref oa_mut)) => {
                let schema_path = oa_mut.schema_path.clone();
                let bytes = encode_proto(oa_mut);
                ("openapi".to_string(), schema_path, bytes)
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
        _request: Request<ApplyTransactionRequest>,
    ) -> Result<Response<ApplyTransactionResponse>, Status> {
        Err(Status::unimplemented("ApplyTransaction not yet implemented"))
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
