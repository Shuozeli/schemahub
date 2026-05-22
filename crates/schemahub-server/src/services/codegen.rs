use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use schemahub_core::Core;
use schemahub_types::{Blob, Hash, SchemaPath};
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

/// Load the `__schema__` blob from a schema tree identified by its hash.
fn load_schema_blob(core: &Core, schema_tree_hash: &Hash) -> Result<Blob, Status> {
    let tree_data = core.storage
        .read_object(schema_tree_hash)
        .map_err(|e| Status::internal(e.to_string()))?
        .ok_or_else(|| Status::internal("schema tree object not found"))?;
    let schema_tree = schemahub_core::objects::decode_tree(&tree_data)
        .map_err(|e| Status::internal(e.to_string()))?;
    let entry = schema_tree
        .entries
        .iter()
        .find(|e| e.name == "__schema__")
        .ok_or_else(|| Status::internal("__schema__ blob not found in schema tree"))?;
    let blob_hash = Hash::from_hex(&entry.hash)
        .map_err(|_| Status::internal(format!("invalid blob hash: {}", entry.hash)))?;
    let blob_data = core.storage
        .read_object(&blob_hash)
        .map_err(|e| Status::internal(e.to_string()))?
        .ok_or_else(|| Status::internal(format!("blob {} not found", entry.hash)))?;
    Ok(Blob::new(blob_data))
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

        let plugin = self.core.plugins.get(&format_id)
            .ok_or_else(|| Status::internal(format!("plugin for '{}' not registered", format_id)))?;

        // Build a name → tree_hash index for all schemas in this commit.
        let schemas = self.core
            .list_schemas(&req.project, &req.repo, &commit_hex)
            .map_err(core_to_status)?;
        let schema_index: HashMap<&str, &Hash> = schemas
            .iter()
            .map(|(name, hash)| (name.as_str(), hash))
            .collect();

        // BFS transitive closure over imports.
        let mut blobs: HashMap<SchemaPath, Blob> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut frontier: VecDeque<String> = VecDeque::new();

        // Validate the root schema exists before starting BFS.
        if !schema_index.contains_key(req.schema_path.as_str()) {
            return Err(Status::not_found(format!("schema '{}' not found", req.schema_path)));
        }
        frontier.push_back(req.schema_path.clone());

        while let Some(schema_name) = frontier.pop_front() {
            if visited.contains(&schema_name) {
                continue;
            }
            visited.insert(schema_name.clone());

            // Look up in the repo's schema index; skip external imports.
            let tree_hash = match schema_index.get(schema_name.as_str()) {
                Some(h) => *h,
                None => continue,
            };

            let blob = load_schema_blob(&self.core, tree_hash)?;
            let sp = SchemaPath::new(
                req.project.clone(),
                req.repo.clone(),
                schema_name.clone(),
            );

            // Queue transitive imports (only those not yet visited).
            if let Ok(imports) = plugin.imports(&blob) {
                for imp in imports {
                    if !visited.contains(&imp.path) {
                        frontier.push_back(imp.path);
                    }
                }
            }

            blobs.insert(sp, blob);
        }

        let descriptor_bytes = plugin
            .generate_descriptors(&blobs)
            .map_err(|e| Status::internal(e.to_string()))?;

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
