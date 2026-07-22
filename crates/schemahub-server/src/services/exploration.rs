//! `ExplorationService` — the read API (design.md §9). Maps each RPC onto the
//! corresponding `Core` exploration method.
//!
//! Each request's `at` VersionRef preserves an explicit branch, tag, or commit.
//! An omitted ref uses the repository's configured default bookmark, and Core
//! resolves it once to an immutable, repository-owned snapshot before reading.

use std::sync::Arc;

use schemahub_core::Core;
use schemahub_types::{Action, SchemaPath};
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::exploration_service_server::ExplorationService;

use crate::error::to_status;
use crate::services::{refspec_or_repository_default, token_from};
use crate::wire;

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
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.at,
            Action::Read,
            token.as_deref(),
        )?;
        let (names, at_commit) = self
            .core
            .list_schemas_resolved(&r.project, &r.repo, &at, token.as_deref())
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
                    ..Default::default()
                }
            })
            .collect();
        Ok(Response::new(pb::ListSchemasResponse {
            schemas,
            at_commit,
        }))
    }

    async fn list_declarations(
        &self,
        request: Request<pb::ListDeclarationsRequest>,
    ) -> Result<Response<pb::ListDeclarationsResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.at,
            Action::Read,
            token.as_deref(),
        )?;
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let (summaries, at_commit) = self
            .core
            .list_declarations_resolved(&schema, &at, token.as_deref())
            .map_err(to_status)?;
        let declarations = summaries.iter().map(wire::decl_summary_to_pb).collect();
        Ok(Response::new(pb::ListDeclarationsResponse {
            declarations,
            at_commit,
        }))
    }

    async fn get_declaration(
        &self,
        request: Request<pb::GetDeclarationRequest>,
    ) -> Result<Response<pb::GetDeclarationResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.at,
            Action::Read,
            token.as_deref(),
        )?;
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let (summary, detail, at_commit) = self
            .core
            .get_declaration_resolved(&schema, &at, &r.declaration_name, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::GetDeclarationResponse {
            summary: Some(wire::decl_summary_to_pb(&summary)),
            detail: detail.0.to_vec(),
            at_commit,
        }))
    }

    async fn get_schema_source(
        &self,
        request: Request<pb::GetSchemaSourceRequest>,
    ) -> Result<Response<pb::GetSchemaSourceResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.at,
            Action::Read,
            token.as_deref(),
        )?;
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let (at_commit, source) = self
            .core
            .get_schema_source_resolved(&schema, &at, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::GetSchemaSourceResponse {
            source: source.into_bytes(),
            at_commit,
        }))
    }

    async fn follow_type(
        &self,
        request: Request<pb::FollowTypeRequest>,
    ) -> Result<Response<pb::FollowTypeResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.at,
            Action::Read,
            token.as_deref(),
        )?;
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let followed = self
            .core
            .follow_field_type(
                &schema,
                &at,
                &r.declaration_name,
                &r.field_name,
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(pb::FollowTypeResponse {
            resolved_project: followed.target_schema.project,
            resolved_repo: followed.target_schema.repo,
            resolved_schema_path: followed.target_schema.schema_name,
            resolved_commit: followed.target_commit,
            summary: Some(wire::decl_summary_to_pb(&followed.summary)),
            detail: followed.detail.0.to_vec(),
            source_commit: followed.source_commit,
            pinned: followed.pinned,
            import_path: followed.import_path,
        }))
    }

    async fn list_dependencies(
        &self,
        request: Request<pb::ListDependenciesRequest>,
    ) -> Result<Response<pb::ListDependenciesResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.at,
            Action::Read,
            token.as_deref(),
        )?;
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let (edges, at_commit) = self
            .core
            .list_dependencies_detailed(&schema, &at, r.transitive, token.as_deref())
            .map_err(to_status)?;
        let dependencies = edges
            .into_iter()
            .map(|edge| pb::DependencyEntry {
                importing_schema: edge.importing_schema.schema_name,
                importing_decl: String::new(),
                imported_project: edge.imported_schema.project,
                imported_repo: edge.imported_schema.repo,
                imported_schema: edge.imported_schema.schema_name,
                imported_decl: edge.import.decl_name,
                resolved_commit: edge.import.resolved_commit.clone(),
                pinned: !edge.import.resolved_commit.is_empty(),
                import_path: edge.import.path,
                importing_project: edge.importing_schema.project,
                importing_repo: edge.importing_schema.repo,
                importing_commit: edge.importing_commit,
                target_commit: edge.target_commit,
                resolved: edge.resolved,
            })
            .collect();
        Ok(Response::new(pb::ListDependenciesResponse {
            dependencies,
            at_commit,
        }))
    }

    async fn list_dependents(
        &self,
        request: Request<pb::ListDependentsRequest>,
    ) -> Result<Response<pb::ListDependentsResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let target = SchemaPath::new(r.project, r.repo, r.schema_path);
        let core = self.core.clone();
        let scan =
            tokio::task::spawn_blocking(move || core.list_dependents(&target, token.as_deref()))
                .await
                .map_err(|error| {
                    tracing::error!(
                        event = "schemahub.dependencies.scan_worker_failed",
                        error = %error,
                    );
                    Status::internal("reverse-dependency scan worker failed")
                })?
                .map_err(to_status)?;
        let dependents = scan
            .dependents
            .into_iter()
            .map(|dependent| {
                let pinned = !dependent.import.resolved_commit.is_empty();
                pb::DependentEntry {
                    importing_project: dependent.importing_schema.project,
                    importing_repo: dependent.importing_schema.repo,
                    importing_schema: dependent.importing_schema.schema_name,
                    importing_decl: String::new(),
                    importing_bookmark: dependent.importing_bookmark,
                    importing_commit: dependent.importing_commit,
                    import_path: dependent.import.path,
                    imported_decl: dependent.import.decl_name,
                    resolved_commit: dependent.import.resolved_commit,
                    pinned,
                }
            })
            .collect();
        let snapshots = scan
            .snapshots
            .into_iter()
            .map(|snapshot| pb::DependencyScanSnapshot {
                project: snapshot.project,
                repo: snapshot.repo,
                bookmark: snapshot.bookmark,
                commit_id: snapshot.commit_id,
            })
            .collect();
        let schemas_scanned = u32::try_from(scan.schemas_scanned).map_err(|_| {
            Status::internal("reverse-dependency schema count exceeds the wire range")
        })?;
        Ok(Response::new(pb::ListDependentsResponse {
            dependents,
            snapshots,
            schemas_scanned,
        }))
    }

    async fn search(
        &self,
        request: Request<pb::SearchRequest>,
    ) -> Result<Response<pb::SearchResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        // Search is repo-scoped in the core and requires an explicit repository.
        if r.project.is_empty() || r.repo.is_empty() {
            return Err(Status::invalid_argument(
                "search requires project and repo (cross-repo search is v2)",
            ));
        }
        // Honor the optional `at` ref (branch / tag / commit). When omitted,
        // search at the repo's default bookmark — the previous behavior.
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.at,
            Action::Read,
            token.as_deref(),
        )?;
        let (hits, at_commit) = self
            .core
            .search_detailed_resolved(&r.project, &r.repo, &at, &r.query, token.as_deref())
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
        Ok(Response::new(pb::SearchResponse { results, at_commit }))
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
