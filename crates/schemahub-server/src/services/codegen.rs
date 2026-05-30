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
        let token = token_from(&request);
        let r = request.into_inner();
        let bookmark = wire::version_ref_bookmark(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let bytes = self
            .core
            .generate_descriptors(&schema, &bookmark, token.as_deref())
            .map_err(to_status)?;
        let format = schemahub_core::detect_format_from_name(&r.schema_path)
            .map(|id| match id {
                "protobuf" => pb::SchemaFormat::Protobuf,
                "flatbuffers" => pb::SchemaFormat::Flatbuffers,
                "openapi" => pb::SchemaFormat::Openapi,
                _ => pb::SchemaFormat::Unspecified,
            })
            .unwrap_or(pb::SchemaFormat::Unspecified);
        Ok(Response::new(pb::GetDescriptorsResponse {
            descriptor_bytes: bytes.to_vec(),
            format: format as i32,
            at_commit: String::new(),
        }))
    }

    async fn preview_codegen(
        &self,
        request: Request<pb::PreviewCodegenRequest>,
    ) -> Result<Response<pb::PreviewCodegenResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let bookmark = wire::version_ref_bookmark(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let lang = wire::language_from_pb(pb::Language::try_from(r.language).unwrap_or(pb::Language::Unspecified))?;
        let code = self
            .core
            .preview_codegen(&schema, &bookmark, lang, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::PreviewCodegenResponse {
            content: code.into_bytes(),
            is_archive: false,
            at_commit: String::new(),
        }))
    }
}
