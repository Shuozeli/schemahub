//! `CodegenService` — descriptors + preview codegen (design.md §10).

use std::sync::Arc;

use schemahub_core::Core;
use schemahub_types::SchemaPath;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::codegen_service_server::CodegenService;

use crate::error::to_status;
use crate::services::token_from;
use crate::wire;

const DEFAULT_BOOKMARK: &str = "main";

pub struct CodegenHandler {
    core: Arc<Core>,
}

impl CodegenHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl CodegenService for CodegenHandler {
    async fn get_descriptors(
        &self,
        request: Request<pb::GetDescriptorsRequest>,
    ) -> Result<Response<pb::GetDescriptorsResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let bytes = self
            .core
            .generate_descriptors_at(&schema, &at, token.as_deref())
            .map_err(to_status)?;
        let format = schemahub_core::detect_format_from_name(&r.schema_path)
            .map(|id| match id {
                "protobuf" => pb::SchemaFormat::Protobuf,
                "flatbuffers" => pb::SchemaFormat::Flatbuffers,
                "openapi" => pb::SchemaFormat::Openapi,
                _ => pb::SchemaFormat::Unspecified,
            })
            .unwrap_or(pb::SchemaFormat::Unspecified);
        let at_commit = resolve_at_commit(&self.core, &r.project, &r.repo, &r.at, token.as_deref());
        Ok(Response::new(pb::GetDescriptorsResponse {
            descriptor_bytes: bytes.to_vec(),
            format: format as i32,
            at_commit,
        }))
    }

    async fn preview_codegen(
        &self,
        request: Request<pb::PreviewCodegenRequest>,
    ) -> Result<Response<pb::PreviewCodegenResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let lang = wire::language_from_pb(
            pb::Language::try_from(r.language).unwrap_or(pb::Language::Unspecified),
        )?;
        let code = self
            .core
            .preview_codegen_at(&schema, &at, lang, token.as_deref())
            .map_err(to_status)?;
        let at_commit = resolve_at_commit(&self.core, &r.project, &r.repo, &r.at, token.as_deref());
        Ok(Response::new(pb::PreviewCodegenResponse {
            content: code.into_bytes(),
            is_archive: false,
            at_commit,
        }))
    }
}

/// Resolve `at` (or the default bookmark) to a concrete commit id so codegen
/// clients can cache by commit. Best-effort: if the resolve fails, returns
/// empty string rather than failing the whole codegen call (descriptors are
/// the primary payload).
fn resolve_at_commit(
    core: &Core,
    project: &str,
    repo: &str,
    at: &Option<pb::VersionRef>,
    token: Option<&str>,
) -> String {
    let refspec = wire::version_ref_to_refspec(at, DEFAULT_BOOKMARK);
    core.log(project, repo, Some(&refspec), Some(1), token)
        .ok()
        .and_then(|entries| entries.into_iter().next())
        .map(|e| e.commit_id)
        .unwrap_or_default()
}
