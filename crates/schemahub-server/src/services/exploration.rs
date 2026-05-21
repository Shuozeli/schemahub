use std::sync::Arc;

use schemahub_core::Core;
use schemahub_types::DeclKind;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    DeclSummary as ProtoDeclSummary, FollowTypeRequest, FollowTypeResponse,
    GetDeclarationRequest, GetDeclarationResponse, ListDeclarationsRequest,
    ListDeclarationsResponse, ListDependenciesRequest, ListDependenciesResponse,
    ListSchemasRequest, ListSchemasResponse, SchemaInfo, SearchRequest, SearchResponse,
    exploration_service_server::ExplorationService,
    version_ref::Ref as VersionRefKind,
};

use crate::error::core_to_status;

pub struct ExplorationServiceImpl {
    core: Arc<Core>,
}

impl ExplorationServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

/// Resolve a proto VersionRef to a commit hex string.
fn resolve_version_ref_to_commit_hex(
    core: &Core,
    project: &str,
    repo: &str,
    vref: Option<schemahub_api::schemahub_v1::VersionRef>,
) -> Result<String, Status> {
    match vref {
        Some(v) => match v.r#ref {
            Some(VersionRefKind::Branch(branch)) => {
                let hash = core.get_branch_head(project, repo, &branch)
                    .map_err(core_to_status)?;
                Ok(hash.to_hex())
            }
            Some(VersionRefKind::Commit(hex)) => Ok(hex),
            Some(VersionRefKind::Tag(tag)) => {
                let key = schemahub_storage::keys::tag_ref_key(project, repo, &tag);
                core.storage.get_ref(&key)
                    .map_err(|e| Status::internal(e.to_string()))?
                    .ok_or_else(|| Status::not_found(format!("tag '{tag}' not found")))
                    .map(|h| h.to_hex())
            }
            None => {
                // Default to main branch.
                let hash = core.get_branch_head(project, repo, "main")
                    .map_err(core_to_status)?;
                Ok(hash.to_hex())
            }
        },
        None => {
            let hash = core.get_branch_head(project, repo, "main")
                .map_err(core_to_status)?;
            Ok(hash.to_hex())
        }
    }
}

/// Map schemahub_types::DeclKind → proto DeclKind i32.
fn decl_kind_to_proto(kind: DeclKind) -> i32 {
    match kind {
        DeclKind::Message => 1,
        DeclKind::Enum => 2,
        DeclKind::Service => 3,
        DeclKind::Table => 4,
        DeclKind::Struct => 5,
        DeclKind::FbsEnum => 6,
        DeclKind::Union => 7,
        DeclKind::PathItem => 8,
        DeclKind::ComponentSchema => 9,
        DeclKind::ComponentParameter => 10,
        DeclKind::ComponentResponse => 11,
        DeclKind::ComponentRequestBody => 12,
        DeclKind::DocumentMetadata => 13,
    }
}

/// Map schema name extension to SchemaFormat i32.
fn format_from_name(name: &str) -> i32 {
    if name.ends_with(".proto") {
        1 // PROTOBUF
    } else if name.ends_with(".fbs") {
        2 // FLATBUFFERS
    } else if name.ends_with(".yaml") || name.ends_with(".yml") || name.ends_with(".json") {
        3 // OPENAPI
    } else {
        0 // UNSPECIFIED
    }
}

#[tonic::async_trait]
impl ExplorationService for ExplorationServiceImpl {
    async fn list_schemas(
        &self,
        request: Request<ListSchemasRequest>,
    ) -> Result<Response<ListSchemasResponse>, Status> {
        let req = request.into_inner();
        let commit_hex = resolve_version_ref_to_commit_hex(
            &self.core,
            &req.project,
            &req.repo,
            req.at,
        )?;

        let schemas = self.core
            .list_schemas(&req.project, &req.repo, &commit_hex)
            .map_err(core_to_status)?;

        let schema_infos: Vec<SchemaInfo> = schemas
            .into_iter()
            .map(|(name, hash)| SchemaInfo {
                format: format_from_name(&name),
                head_blob: hash.to_hex(),
                name,
            })
            .collect();

        Ok(Response::new(ListSchemasResponse { schemas: schema_infos }))
    }

    async fn list_declarations(
        &self,
        request: Request<ListDeclarationsRequest>,
    ) -> Result<Response<ListDeclarationsResponse>, Status> {
        let req = request.into_inner();
        let commit_hex = resolve_version_ref_to_commit_hex(
            &self.core,
            &req.project,
            &req.repo,
            req.at,
        )?;

        let decls = self.core
            .list_declarations(&req.project, &req.repo, &req.schema_path, &commit_hex)
            .map_err(core_to_status)?;

        let kind_filter = req.kind_filter;
        let declarations: Vec<ProtoDeclSummary> = decls
            .into_iter()
            .filter(|d| {
                kind_filter == 0 || decl_kind_to_proto(d.kind) == kind_filter
            })
            .map(|d| ProtoDeclSummary {
                name: d.name,
                kind: decl_kind_to_proto(d.kind),
                doc_comment: d.doc_comment,
            })
            .collect();

        Ok(Response::new(ListDeclarationsResponse { declarations }))
    }

    async fn get_declaration(
        &self,
        request: Request<GetDeclarationRequest>,
    ) -> Result<Response<GetDeclarationResponse>, Status> {
        let req = request.into_inner();
        let commit_hex = resolve_version_ref_to_commit_hex(
            &self.core,
            &req.project,
            &req.repo,
            req.at,
        )?;

        let detail_opt = self.core
            .get_declaration(
                &req.project,
                &req.repo,
                &req.schema_path,
                &req.declaration_name,
                &commit_hex,
            )
            .map_err(core_to_status)?;

        match detail_opt {
            None => Err(Status::not_found(format!(
                "declaration '{}' not found in schema '{}'",
                req.declaration_name, req.schema_path
            ))),
            Some(detail) => {
                let detail_bytes = detail.as_bytes().clone();
                Ok(Response::new(GetDeclarationResponse {
                    summary: Some(ProtoDeclSummary {
                        name: req.declaration_name,
                        kind: 0, // unknown at this level without re-parsing
                        doc_comment: String::new(),
                    }),
                    detail: detail_bytes.to_vec().into(),
                    at_commit: commit_hex,
                }))
            }
        }
    }

    async fn follow_type(
        &self,
        _request: Request<FollowTypeRequest>,
    ) -> Result<Response<FollowTypeResponse>, Status> {
        Err(Status::unimplemented("FollowType not yet implemented"))
    }

    async fn list_dependencies(
        &self,
        _request: Request<ListDependenciesRequest>,
    ) -> Result<Response<ListDependenciesResponse>, Status> {
        Err(Status::unimplemented("ListDependencies not yet implemented"))
    }

    async fn search(
        &self,
        _request: Request<SearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        Err(Status::unimplemented("Search not yet implemented"))
    }
}
