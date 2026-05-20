use std::sync::Arc;

use schemahub_core::Core;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    FollowTypeRequest, FollowTypeResponse, GetDeclarationRequest, GetDeclarationResponse,
    ListDeclarationsRequest, ListDeclarationsResponse, ListDependenciesRequest,
    ListDependenciesResponse, ListSchemasRequest, ListSchemasResponse, SearchRequest,
    SearchResponse,
    exploration_service_server::ExplorationService,
};

pub struct ExplorationServiceImpl {
    #[allow(dead_code)]
    core: Arc<Core>,
}

impl ExplorationServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl ExplorationService for ExplorationServiceImpl {
    async fn list_schemas(
        &self,
        _request: Request<ListSchemasRequest>,
    ) -> Result<Response<ListSchemasResponse>, Status> {
        Err(Status::unimplemented("ListSchemas not yet implemented"))
    }

    async fn list_declarations(
        &self,
        _request: Request<ListDeclarationsRequest>,
    ) -> Result<Response<ListDeclarationsResponse>, Status> {
        Err(Status::unimplemented("ListDeclarations not yet implemented"))
    }

    async fn get_declaration(
        &self,
        _request: Request<GetDeclarationRequest>,
    ) -> Result<Response<GetDeclarationResponse>, Status> {
        Err(Status::unimplemented("GetDeclaration not yet implemented"))
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
