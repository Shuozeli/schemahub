use std::sync::Arc;

use schemahub_core::Core;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    AddMemberRequest, AddMemberResponse, CreateProjectRequest, CreateProjectResponse,
    CreateRepoRequest, CreateRepoResponse, DeleteProjectRequest, DeleteProjectResponse,
    DeleteRepoRequest, DeleteRepoResponse, GetProjectRequest, GetProjectResponse, GetRepoRequest,
    GetRepoResponse, ListMembersRequest, ListMembersResponse, ListProjectsRequest,
    ListProjectsResponse, ListReposRequest, ListReposResponse, RemoveMemberRequest,
    RemoveMemberResponse, UpdateMemberRoleRequest, UpdateMemberRoleResponse, UpdateRepoRequest,
    UpdateRepoResponse,
    project_service_server::ProjectService,
};

pub struct ProjectServiceImpl {
    #[allow(dead_code)]
    core: Arc<Core>,
}

impl ProjectServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl ProjectService for ProjectServiceImpl {
    async fn create_project(
        &self,
        _request: Request<CreateProjectRequest>,
    ) -> Result<Response<CreateProjectResponse>, Status> {
        Err(Status::unimplemented("CreateProject not yet implemented"))
    }

    async fn get_project(
        &self,
        _request: Request<GetProjectRequest>,
    ) -> Result<Response<GetProjectResponse>, Status> {
        Err(Status::unimplemented("GetProject not yet implemented"))
    }

    async fn list_projects(
        &self,
        _request: Request<ListProjectsRequest>,
    ) -> Result<Response<ListProjectsResponse>, Status> {
        Err(Status::unimplemented("ListProjects not yet implemented"))
    }

    async fn delete_project(
        &self,
        _request: Request<DeleteProjectRequest>,
    ) -> Result<Response<DeleteProjectResponse>, Status> {
        Err(Status::unimplemented("DeleteProject not yet implemented"))
    }

    async fn create_repo(
        &self,
        _request: Request<CreateRepoRequest>,
    ) -> Result<Response<CreateRepoResponse>, Status> {
        Err(Status::unimplemented("CreateRepo not yet implemented"))
    }

    async fn get_repo(
        &self,
        _request: Request<GetRepoRequest>,
    ) -> Result<Response<GetRepoResponse>, Status> {
        Err(Status::unimplemented("GetRepo not yet implemented"))
    }

    async fn update_repo(
        &self,
        _request: Request<UpdateRepoRequest>,
    ) -> Result<Response<UpdateRepoResponse>, Status> {
        Err(Status::unimplemented("UpdateRepo not yet implemented"))
    }

    async fn list_repos(
        &self,
        _request: Request<ListReposRequest>,
    ) -> Result<Response<ListReposResponse>, Status> {
        Err(Status::unimplemented("ListRepos not yet implemented"))
    }

    async fn delete_repo(
        &self,
        _request: Request<DeleteRepoRequest>,
    ) -> Result<Response<DeleteRepoResponse>, Status> {
        Err(Status::unimplemented("DeleteRepo not yet implemented"))
    }

    async fn add_member(
        &self,
        _request: Request<AddMemberRequest>,
    ) -> Result<Response<AddMemberResponse>, Status> {
        Err(Status::unimplemented("AddMember not yet implemented"))
    }

    async fn remove_member(
        &self,
        _request: Request<RemoveMemberRequest>,
    ) -> Result<Response<RemoveMemberResponse>, Status> {
        Err(Status::unimplemented("RemoveMember not yet implemented"))
    }

    async fn update_member_role(
        &self,
        _request: Request<UpdateMemberRoleRequest>,
    ) -> Result<Response<UpdateMemberRoleResponse>, Status> {
        Err(Status::unimplemented("UpdateMemberRole not yet implemented"))
    }

    async fn list_members(
        &self,
        _request: Request<ListMembersRequest>,
    ) -> Result<Response<ListMembersResponse>, Status> {
        Err(Status::unimplemented("ListMembers not yet implemented"))
    }
}
