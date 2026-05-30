//! `ProjectService` — projects, repos, members (design.md §11).
//!
//! In the jj model a `(project, repo)` is implicit: it springs into existence on
//! the first write (the op-log/bookmark set is created lazily). There is no
//! separate project/repo registry in `Core`/`Vcs` yet, so these handlers are
//! thin: Create/Get/Update echo back the requested config; List returns what is
//! observable; member/ACL management maps to the deferred RBAC layer
//! (design.md §11 ships Noop auth) and is reported UNIMPLEMENTED.

use std::sync::Arc;

use schemahub_core::Core;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::project_service_server::ProjectService;

pub struct ProjectHandler {
    #[allow(dead_code)]
    core: Arc<Core>,
}

impl ProjectHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl ProjectService for ProjectHandler {
    async fn create_project(
        &self,
        request: Request<pb::CreateProjectRequest>,
    ) -> Result<Response<pb::CreateProjectResponse>, Status> {
        let r = request.into_inner();
        Ok(Response::new(pb::CreateProjectResponse {
            project: Some(pb::ProjectInfo {
                name: r.name,
                is_public: r.is_public,
                owner: String::new(),
            }),
        }))
    }

    async fn get_project(
        &self,
        request: Request<pb::GetProjectRequest>,
    ) -> Result<Response<pb::GetProjectResponse>, Status> {
        let r = request.into_inner();
        Ok(Response::new(pb::GetProjectResponse {
            project: Some(pb::ProjectInfo {
                name: r.name,
                is_public: true,
                owner: String::new(),
            }),
        }))
    }

    async fn list_projects(
        &self,
        _request: Request<pb::ListProjectsRequest>,
    ) -> Result<Response<pb::ListProjectsResponse>, Status> {
        // No persisted project registry in v1; projects are implicit.
        Ok(Response::new(pb::ListProjectsResponse { projects: vec![] }))
    }

    async fn delete_project(
        &self,
        _request: Request<pb::DeleteProjectRequest>,
    ) -> Result<Response<pb::DeleteProjectResponse>, Status> {
        Err(Status::unimplemented(
            "project deletion is not exposed by the VCS layer in v1",
        ))
    }

    async fn create_repo(
        &self,
        request: Request<pb::CreateRepoRequest>,
    ) -> Result<Response<pb::CreateRepoResponse>, Status> {
        let r = request.into_inner();
        Ok(Response::new(pb::CreateRepoResponse {
            repo: Some(pb::RepoConfig {
                project: r.project,
                name: r.name,
                default_branch: if r.default_branch.is_empty() {
                    "main".to_string()
                } else {
                    r.default_branch
                },
                compatibility_direction: r.compatibility_direction,
                protected_branches: if r.protected_branches.is_empty() {
                    vec!["main".to_string()]
                } else {
                    r.protected_branches
                },
            }),
        }))
    }

    async fn get_repo(
        &self,
        request: Request<pb::GetRepoRequest>,
    ) -> Result<Response<pb::GetRepoResponse>, Status> {
        let r = request.into_inner();
        Ok(Response::new(pb::GetRepoResponse {
            repo: Some(pb::RepoConfig {
                project: r.project,
                name: r.repo,
                default_branch: "main".to_string(),
                compatibility_direction: pb::CompatibilityDirection::Full as i32,
                protected_branches: vec!["main".to_string()],
            }),
        }))
    }

    async fn update_repo(
        &self,
        request: Request<pb::UpdateRepoRequest>,
    ) -> Result<Response<pb::UpdateRepoResponse>, Status> {
        let r = request.into_inner();
        Ok(Response::new(pb::UpdateRepoResponse {
            repo: Some(pb::RepoConfig {
                project: r.project,
                name: r.repo,
                default_branch: if r.default_branch.is_empty() {
                    "main".to_string()
                } else {
                    r.default_branch
                },
                compatibility_direction: r.compatibility_direction,
                protected_branches: r.protected_branches,
            }),
        }))
    }

    async fn list_repos(
        &self,
        _request: Request<pb::ListReposRequest>,
    ) -> Result<Response<pb::ListReposResponse>, Status> {
        Ok(Response::new(pb::ListReposResponse { repos: vec![] }))
    }

    async fn delete_repo(
        &self,
        _request: Request<pb::DeleteRepoRequest>,
    ) -> Result<Response<pb::DeleteRepoResponse>, Status> {
        Err(Status::unimplemented(
            "repo deletion is not exposed by the VCS layer in v1",
        ))
    }

    async fn add_member(
        &self,
        _request: Request<pb::AddMemberRequest>,
    ) -> Result<Response<pb::AddMemberResponse>, Status> {
        Err(Status::unimplemented(
            "member management requires the RBAC layer (deferred; Noop auth ships by default)",
        ))
    }

    async fn remove_member(
        &self,
        _request: Request<pb::RemoveMemberRequest>,
    ) -> Result<Response<pb::RemoveMemberResponse>, Status> {
        Err(Status::unimplemented("member management is deferred"))
    }

    async fn update_member_role(
        &self,
        _request: Request<pb::UpdateMemberRoleRequest>,
    ) -> Result<Response<pb::UpdateMemberRoleResponse>, Status> {
        Err(Status::unimplemented("member management is deferred"))
    }

    async fn list_members(
        &self,
        _request: Request<pb::ListMembersRequest>,
    ) -> Result<Response<pb::ListMembersResponse>, Status> {
        Ok(Response::new(pb::ListMembersResponse { members: vec![] }))
    }
}
