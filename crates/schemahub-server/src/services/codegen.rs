use std::sync::Arc;

use schemahub_core::Core;
use schemahub_types::{Blob, SchemaPath};
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    GetDescriptorsRequest, GetDescriptorsResponse, PreviewCodegenRequest, PreviewCodegenResponse,
    codegen_service_server::CodegenService,
    version_ref::Ref as VersionRefKind,
};

use crate::error::core_to_status;

pub struct CodegenServiceImpl {
    core: Arc<Core>,
}

impl CodegenServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

/// Resolve a VersionRef to a commit hex string.
fn resolve_vref(
    core: &Core,
    project: &str,
    repo: &str,
    vref: Option<schemahub_api::schemahub_v1::VersionRef>,
) -> Result<String, Status> {
    match vref {
        Some(v) => match v.r#ref {
            Some(VersionRefKind::Branch(branch)) => core
                .get_branch_head(project, repo, &branch)
                .map(|h| h.to_hex())
                .map_err(core_to_status),
            Some(VersionRefKind::Commit(hex)) => Ok(hex),
            Some(VersionRefKind::Tag(tag)) => {
                let key = schemahub_storage::keys::tag_ref_key(project, repo, &tag);
                core.storage
                    .get_ref(&key)
                    .map_err(|e| Status::internal(e.to_string()))?
                    .ok_or_else(|| Status::not_found(format!("tag '{tag}' not found")))
                    .map(|h| h.to_hex())
            }
            None => core
                .get_branch_head(project, repo, "main")
                .map(|h| h.to_hex())
                .map_err(core_to_status),
        },
        None => core
            .get_branch_head(project, repo, "main")
            .map(|h| h.to_hex())
            .map_err(core_to_status),
    }
}

#[tonic::async_trait]
impl CodegenService for CodegenServiceImpl {
    async fn get_descriptors(
        &self,
        request: Request<GetDescriptorsRequest>,
    ) -> Result<Response<GetDescriptorsResponse>, Status> {
        let req = request.into_inner();

        let commit_hex = resolve_vref(&self.core, &req.project, &req.repo, req.at)?;

        // Detect format from schema_path extension.
        let format_id = schemahub_core::detect_format_from_name(&req.schema_path)
            .ok_or_else(|| Status::invalid_argument(format!(
                "cannot detect format for schema path '{}'",
                req.schema_path
            )))?;

        let proto_format = match format_id.as_str() {
            "protobuf"    => 1i32,
            "flatbuffers" => 2i32,
            "openapi"     => 3i32,
            _             => 0i32,
        };

        // Load all blobs for the schema.
        let plugin = self.core.plugins.get(&format_id)
            .ok_or_else(|| Status::internal(format!("plugin for '{}' not registered", format_id)))?;

        let schemas = self.core
            .list_schemas(&req.project, &req.repo, &commit_hex)
            .map_err(core_to_status)?;

        let (_, schema_tree_hash) = schemas
            .iter()
            .find(|(name, _)| name == &req.schema_path)
            .ok_or_else(|| Status::not_found(format!("schema '{}' not found", req.schema_path)))?;

        // Load the schema tree object to get declaration blobs.
        let schema_tree_data = self.core.storage
            .read_object(schema_tree_hash)
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::internal("schema tree object not found"))?;
        let schema_tree = schemahub_core::objects::decode_tree(&schema_tree_data)
            .map_err(|e| Status::internal(e.to_string()))?;

        // Build blobs map and print reconstructed source.
        let mut printed_parts = Vec::new();
        let schema_path = SchemaPath::new(req.project.clone(), req.repo.clone(), req.schema_path.clone());

        for entry in &schema_tree.entries {
            if entry.kind != schemahub_core::objects::KIND_BLOB {
                continue;
            }
            let blob_hash = schemahub_types::Hash::from_hex(&entry.hash)
                .map_err(|_| Status::internal(format!("invalid blob hash: {}", entry.hash)))?;
            let blob_data = self.core.storage
                .read_object(&blob_hash)
                .map_err(|e| Status::internal(e.to_string()))?
                .ok_or_else(|| Status::internal(format!("blob {} not found", entry.hash)))?;
            let blob = Blob::new(blob_data);

            match plugin.print(&blob) {
                Ok(source) => printed_parts.push(source),
                Err(_) => {} // skip unrepresentable blobs
            }
        }

        // Try generate_descriptors using print-reconstructed blobs.
        let descriptor_bytes = if printed_parts.is_empty() {
            vec![]
        } else {
            // For a simple implementation: return the reconstructed source as UTF-8 bytes.
            // A full implementation would call plugin.generate_descriptors() with the blobs map.
            let full_source = printed_parts.join("\n\n");
            full_source.into_bytes()
        };

        let _ = schema_path; // suppress unused warning

        Ok(Response::new(GetDescriptorsResponse {
            descriptor_bytes: descriptor_bytes.into(),
            format: proto_format,
            at_commit: commit_hex,
        }))
    }

    async fn preview_codegen(
        &self,
        _request: Request<PreviewCodegenRequest>,
    ) -> Result<Response<PreviewCodegenResponse>, Status> {
        Err(Status::unimplemented("PreviewCodegen not yet implemented"))
    }
}
