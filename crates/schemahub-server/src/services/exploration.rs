//! `ExplorationService` — the read API (design.md §9). Maps each RPC onto the
//! corresponding `Core` exploration method.
//!
//! Note: the request's `at` VersionRef is resolved to a full `RefSpec` via
//! `wire::version_ref_to_refspec` (the same helper `RefService.Diff` uses), so
//! branch, tag, and commit refs are all pinned to the correct historical
//! snapshot. An unset `at` defaults to the "main" bookmark.

use std::sync::Arc;

use schemahub_core::Core;
use schemahub_types::SchemaPath;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::exploration_service_server::ExplorationService;

use crate::error::to_status;
use crate::services::token_from;
use crate::wire;

const DEFAULT_BOOKMARK: &str = "main";

pub struct ExplorationHandler {
    core: Arc<Core>,
}

impl ExplorationHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl ExplorationService for ExplorationHandler {
    async fn list_schemas(
        &self,
        request: Request<pb::ListSchemasRequest>,
    ) -> Result<Response<pb::ListSchemasResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let names = self
            .core
            .list_schemas(&r.project, &r.repo, &at, token.as_deref())
            .map_err(to_status)?;
        let schemas = names
            .into_iter()
            .map(|name| {
                let format = schemahub_core::detect_format_from_name(&name)
                    .map(format_to_pb)
                    .unwrap_or(pb::SchemaFormat::Unspecified);
                pb::SchemaInfo {
                    name,
                    format: format as i32,
                    head_blob: String::new(),
                }
            })
            .collect();
        Ok(Response::new(pb::ListSchemasResponse { schemas }))
    }

    async fn list_declarations(
        &self,
        request: Request<pb::ListDeclarationsRequest>,
    ) -> Result<Response<pb::ListDeclarationsResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let summaries = self
            .core
            .list_declarations(&schema, &at, token.as_deref())
            .map_err(to_status)?;
        let declarations = summaries.iter().map(wire::decl_summary_to_pb).collect();
        Ok(Response::new(pb::ListDeclarationsResponse { declarations }))
    }

    async fn get_declaration(
        &self,
        request: Request<pb::GetDeclarationRequest>,
    ) -> Result<Response<pb::GetDeclarationResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        // Summary (list + filter by name) and detail.
        let summaries = self
            .core
            .list_declarations(&schema, &at, token.as_deref())
            .map_err(to_status)?;
        let summary = summaries
            .iter()
            .find(|s| s.name == r.declaration_name)
            .map(wire::decl_summary_to_pb);
        let detail = self
            .core
            .get_declaration(&schema, &at, &r.declaration_name, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::GetDeclarationResponse {
            summary,
            detail: detail.0.to_vec(),
            at_commit: String::new(),
        }))
    }

    async fn get_schema_source(
        &self,
        request: Request<pb::GetSchemaSourceRequest>,
    ) -> Result<Response<pb::GetSchemaSourceResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let source = self
            .core
            .get_schema_source(&schema, &at, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::GetSchemaSourceResponse {
            source: source.into_bytes(),
            at_commit: String::new(),
        }))
    }

    async fn follow_type(
        &self,
        request: Request<pb::FollowTypeRequest>,
    ) -> Result<Response<pb::FollowTypeResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let (refs, imports) = self
            .core
            .follow_type(&schema, &at, &r.declaration_name, token.as_deref())
            .map_err(to_status)?;
        // Resolve the named field's type against imports where possible. The
        // core returns the declaration's type refs + the file imports; we report
        // the first import that matches a referenced type, else echo the request.
        let referenced: Vec<String> = refs.into_iter().map(|t| t.name).collect();
        let matched = imports
            .iter()
            .find(|imp| referenced.iter().any(|n| n.ends_with(&imp.decl_name)));
        let (resolved_schema_path, resolved_commit) = match matched {
            Some(imp) => (imp.path.clone(), imp.resolved_commit.clone()),
            None => (r.schema_path.clone(), String::new()),
        };
        Ok(Response::new(pb::FollowTypeResponse {
            resolved_project: r.project,
            resolved_repo: r.repo,
            resolved_schema_path,
            resolved_commit,
            summary: None,
            detail: Vec::new(),
        }))
    }

    async fn list_dependencies(
        &self,
        request: Request<pb::ListDependenciesRequest>,
    ) -> Result<Response<pb::ListDependenciesResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let imports = self
            .core
            .list_dependencies(&schema, &at, token.as_deref())
            .map_err(to_status)?;
        let dependencies = imports
            .into_iter()
            .map(|imp| pb::DependencyEntry {
                importing_schema: r.schema_path.clone(),
                importing_decl: String::new(),
                imported_project: String::new(),
                imported_repo: String::new(),
                imported_schema: imp.path,
                imported_decl: imp.decl_name,
                resolved_commit: imp.resolved_commit,
            })
            .collect();
        Ok(Response::new(pb::ListDependenciesResponse { dependencies }))
    }

    async fn search(
        &self,
        request: Request<pb::SearchRequest>,
    ) -> Result<Response<pb::SearchResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        // Search is repo-scoped in the core; require project+repo and use "main".
        if r.project.is_empty() || r.repo.is_empty() {
            return Err(Status::invalid_argument(
                "search requires project and repo (cross-repo search is v2)",
            ));
        }
        // Honor the optional `at` ref (branch / tag / commit). When omitted,
        // search at the repo's default bookmark — the previous behavior.
        let at = wire::version_ref_to_refspec(&r.at, DEFAULT_BOOKMARK);
        let hits = self
            .core
            .search_detailed(&r.project, &r.repo, &at, &r.query, token.as_deref())
            .map_err(to_status)?;
        let results = hits
            .into_iter()
            .map(|h| pb::SearchResult {
                project: r.project.clone(),
                repo: r.repo.clone(),
                schema_path: h.schema_name,
                declaration: Some(wire::decl_summary_to_pb(&h.summary)),
            })
            .collect();
        Ok(Response::new(pb::SearchResponse { results }))
    }
}

fn format_to_pb(id: &str) -> pb::SchemaFormat {
    match id {
        "protobuf" => pb::SchemaFormat::Protobuf,
        "flatbuffers" => pb::SchemaFormat::Flatbuffers,
        "openapi" => pb::SchemaFormat::Openapi,
        _ => pb::SchemaFormat::Unspecified,
    }
}
