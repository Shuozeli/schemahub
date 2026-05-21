use std::sync::Arc;

use schemahub_core::{Core, RepoConfig};
use schemahub_types::CompatibilityDirection;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    AddMemberRequest, AddMemberResponse, CreateProjectRequest, CreateProjectResponse,
    CreateRepoRequest, CreateRepoResponse, DeleteProjectRequest, DeleteProjectResponse,
    DeleteRepoRequest, DeleteRepoResponse, GetProjectRequest, GetProjectResponse, GetRepoRequest,
    GetRepoResponse, ListMembersRequest, ListMembersResponse, ListProjectsRequest,
    ListProjectsResponse, ListReposRequest, ListReposResponse, MemberEntry, ProjectInfo,
    RemoveMemberRequest, RemoveMemberResponse, UpdateMemberRoleRequest, UpdateMemberRoleResponse,
    UpdateRepoRequest, UpdateRepoResponse,
    project_service_server::ProjectService,
};

use crate::error::core_to_status;

pub struct ProjectServiceImpl {
    core: Arc<Core>,
}

impl ProjectServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

/// Map a CompatibilityDirection i32 from proto to the internal type.
fn compat_dir_from_i32(v: i32) -> CompatibilityDirection {
    match v {
        1 => CompatibilityDirection::Backward,
        2 => CompatibilityDirection::Forward,
        3 => CompatibilityDirection::Full,
        _ => CompatibilityDirection::Disabled,
    }
}

/// Map internal CompatibilityDirection to proto i32.
fn compat_dir_to_i32(d: CompatibilityDirection) -> i32 {
    match d {
        CompatibilityDirection::Backward => 1,
        CompatibilityDirection::Forward  => 2,
        CompatibilityDirection::Full     => 3,
        CompatibilityDirection::Disabled => 0,
    }
}

fn repo_config_to_proto(
    project: &str,
    name: &str,
    cfg: &RepoConfig,
) -> schemahub_api::schemahub_v1::RepoConfig {
    schemahub_api::schemahub_v1::RepoConfig {
        project: project.to_string(),
        name: name.to_string(),
        default_branch: cfg.default_branch.clone(),
        compatibility_direction: compat_dir_to_i32(cfg.compatibility_direction),
        protected_branches: cfg.protected_branches.clone(),
    }
}

/// Parse role i32 to a string.
fn role_to_str(role: i32) -> &'static str {
    match role {
        1 => "owner",
        2 => "maintainer",
        3 => "writer",
        4 => "reader",
        _ => "reader",
    }
}

fn role_str_to_i32(s: &str) -> i32 {
    match s {
        "owner"      => 1,
        "maintainer" => 2,
        "writer"     => 3,
        _            => 4,
    }
}

#[tonic::async_trait]
impl ProjectService for ProjectServiceImpl {
    async fn create_project(
        &self,
        request: Request<CreateProjectRequest>,
    ) -> Result<Response<CreateProjectResponse>, Status> {
        let req = request.into_inner();
        self.core
            .create_project(&req.name, req.is_public)
            .map_err(core_to_status)?;
        Ok(Response::new(CreateProjectResponse {
            project: Some(ProjectInfo {
                name: req.name,
                is_public: req.is_public,
                owner: String::new(),
            }),
        }))
    }

    async fn get_project(
        &self,
        request: Request<GetProjectRequest>,
    ) -> Result<Response<GetProjectResponse>, Status> {
        let req = request.into_inner();
        let meta = self.core
            .get_project_meta(&req.name)
            .map_err(core_to_status)?
            .ok_or_else(|| Status::not_found(format!("project '{}' not found", req.name)))?;
        let is_public = meta.get("is_public").and_then(|v| v.as_bool()).unwrap_or(false);
        let owner = meta.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Ok(Response::new(GetProjectResponse {
            project: Some(ProjectInfo { name: req.name, is_public, owner }),
        }))
    }

    async fn list_projects(
        &self,
        request: Request<ListProjectsRequest>,
    ) -> Result<Response<ListProjectsResponse>, Status> {
        let req = request.into_inner();
        let projects = self.core
            .list_projects(&req.name_prefix)
            .map_err(core_to_status)?;
        let project_infos: Vec<ProjectInfo> = projects
            .into_iter()
            .map(|meta| ProjectInfo {
                name: meta.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                is_public: meta.get("is_public").and_then(|v| v.as_bool()).unwrap_or(false),
                owner: meta.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            })
            .collect();
        Ok(Response::new(ListProjectsResponse { projects: project_infos }))
    }

    async fn delete_project(
        &self,
        request: Request<DeleteProjectRequest>,
    ) -> Result<Response<DeleteProjectResponse>, Status> {
        let req = request.into_inner();
        self.core
            .delete_project(&req.name, req.force)
            .map_err(core_to_status)?;
        Ok(Response::new(DeleteProjectResponse {}))
    }

    async fn create_repo(
        &self,
        request: Request<CreateRepoRequest>,
    ) -> Result<Response<CreateRepoResponse>, Status> {
        let req = request.into_inner();
        let default_branch = if req.default_branch.is_empty() {
            "main".to_string()
        } else {
            req.default_branch.clone()
        };
        let compat = compat_dir_from_i32(req.compatibility_direction);
        let protected = if req.protected_branches.is_empty() {
            vec![default_branch.clone()]
        } else {
            req.protected_branches.clone()
        };
        let config = RepoConfig {
            default_branch: default_branch.clone(),
            compatibility_direction: compat,
            protected_branches: protected,
        };
        self.core
            .initialize_repo(&req.project, &req.name, &config)
            .map_err(core_to_status)?;
        Ok(Response::new(CreateRepoResponse {
            repo: Some(repo_config_to_proto(&req.project, &req.name, &config)),
        }))
    }

    async fn get_repo(
        &self,
        request: Request<GetRepoRequest>,
    ) -> Result<Response<GetRepoResponse>, Status> {
        let req = request.into_inner();
        let cfg = self.core
            .get_repo_config(&req.project, &req.repo)
            .map_err(core_to_status)?;
        Ok(Response::new(GetRepoResponse {
            repo: Some(repo_config_to_proto(&req.project, &req.repo, &cfg)),
        }))
    }

    async fn update_repo(
        &self,
        request: Request<UpdateRepoRequest>,
    ) -> Result<Response<UpdateRepoResponse>, Status> {
        let req = request.into_inner();
        let mut cfg = self.core
            .get_repo_config(&req.project, &req.repo)
            .map_err(core_to_status)?;

        if req.compatibility_direction != 0 {
            cfg.compatibility_direction = compat_dir_from_i32(req.compatibility_direction);
        }
        if !req.protected_branches.is_empty() {
            cfg.protected_branches = req.protected_branches.clone();
        }
        if !req.default_branch.is_empty() {
            cfg.default_branch = req.default_branch.clone();
        }

        self.core
            .set_repo_config(&req.project, &req.repo, &cfg)
            .map_err(core_to_status)?;
        Ok(Response::new(UpdateRepoResponse {
            repo: Some(repo_config_to_proto(&req.project, &req.repo, &cfg)),
        }))
    }

    async fn list_repos(
        &self,
        request: Request<ListReposRequest>,
    ) -> Result<Response<ListReposResponse>, Status> {
        let req = request.into_inner();
        let repos = self.core
            .list_repos(&req.project, &req.name_prefix)
            .map_err(core_to_status)?;
        let repo_configs: Vec<schemahub_api::schemahub_v1::RepoConfig> = repos
            .into_iter()
            .map(|(name, cfg)| repo_config_to_proto(&req.project, &name, &cfg))
            .collect();
        Ok(Response::new(ListReposResponse { repos: repo_configs }))
    }

    async fn delete_repo(
        &self,
        request: Request<DeleteRepoRequest>,
    ) -> Result<Response<DeleteRepoResponse>, Status> {
        let req = request.into_inner();
        self.core
            .delete_repo(&req.project, &req.repo, req.force)
            .map_err(core_to_status)?;
        Ok(Response::new(DeleteRepoResponse {}))
    }

    async fn add_member(
        &self,
        request: Request<AddMemberRequest>,
    ) -> Result<Response<AddMemberResponse>, Status> {
        let req = request.into_inner();
        let role_str = role_to_str(req.role);
        self.core
            .add_member(&req.project, &req.identity, role_str)
            .map_err(core_to_status)?;
        Ok(Response::new(AddMemberResponse {
            member: Some(MemberEntry {
                identity: req.identity,
                role: req.role,
            }),
        }))
    }

    async fn remove_member(
        &self,
        request: Request<RemoveMemberRequest>,
    ) -> Result<Response<RemoveMemberResponse>, Status> {
        let req = request.into_inner();
        self.core
            .remove_member(&req.project, &req.identity)
            .map_err(core_to_status)?;
        Ok(Response::new(RemoveMemberResponse {}))
    }

    async fn update_member_role(
        &self,
        request: Request<UpdateMemberRoleRequest>,
    ) -> Result<Response<UpdateMemberRoleResponse>, Status> {
        let req = request.into_inner();
        let role_str = role_to_str(req.new_role);
        self.core
            .add_member(&req.project, &req.identity, role_str)
            .map_err(core_to_status)?;
        Ok(Response::new(UpdateMemberRoleResponse {
            member: Some(MemberEntry {
                identity: req.identity,
                role: req.new_role,
            }),
        }))
    }

    async fn list_members(
        &self,
        request: Request<ListMembersRequest>,
    ) -> Result<Response<ListMembersResponse>, Status> {
        let req = request.into_inner();
        let members_raw = self.core
            .list_members(&req.project)
            .map_err(core_to_status)?;
        let members: Vec<MemberEntry> = members_raw
            .into_iter()
            .map(|(identity, role_str)| MemberEntry {
                identity,
                role: role_str_to_i32(&role_str),
            })
            .collect();
        Ok(Response::new(ListMembersResponse { members }))
    }
}
