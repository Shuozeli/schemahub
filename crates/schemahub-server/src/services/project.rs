//! `ProjectService` — projects, repos, members (design.md §6, §11).
//!
//! Project + member management is wired to the [`Core`] project/role
//! orchestration (see `crates/schemahub-core/src/projects.rs`):
//! `CreateProject`, `GetProject`, `ListProjects`, `AddMember`,
//! `RemoveMember`, `UpdateMemberRole`, `ListMembers` all flow through Core
//! which runs the configured `AuthnProvider` + `AuthzPolicy` before touching
//! the role / project stores.
//!
//! Repo lifecycle is still implicit in the jj VCS model (a `(project, repo)`
//! springs into existence on the first write); `CreateRepo` / `GetRepo` /
//! `UpdateRepo` echo back the requested config until a persisted repo
//! registry lands.

use std::sync::Arc;

use schemahub_core::Core;
use schemahub_types::{Action, Role, Visibility};
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::project_service_server::ProjectService;

use crate::error::to_status;
use crate::services::token_from;

pub struct ProjectHandler {
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
        let token = token_from(&request)?;
        let r = request.into_inner();
        if r.name.is_empty() {
            return Err(Status::invalid_argument("name must not be empty"));
        }
        let visibility = if r.is_public {
            Visibility::Public
        } else {
            Visibility::Private
        };
        let meta = self
            .core
            .create_project(&r.name, visibility, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::CreateProjectResponse {
            project: Some(meta_to_proto(&meta)),
        }))
    }

    async fn get_project(
        &self,
        request: Request<pb::GetProjectRequest>,
    ) -> Result<Response<pb::GetProjectResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let meta = self
            .core
            .get_project(&r.name, token.as_deref())
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found(format!("project '{}' not found", r.name)))?;
        Ok(Response::new(pb::GetProjectResponse {
            project: Some(meta_to_proto(&meta)),
        }))
    }

    async fn list_projects(
        &self,
        request: Request<pb::ListProjectsRequest>,
    ) -> Result<Response<pb::ListProjectsResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let prefix = r.name_prefix;
        let visible = self
            .core
            .list_projects(token.as_deref())
            .map_err(to_status)?;
        let projects = visible
            .into_iter()
            .filter(|m| prefix.is_empty() || m.name.starts_with(&prefix))
            .map(|m| meta_to_proto(&m))
            .collect();
        Ok(Response::new(pb::ListProjectsResponse { projects }))
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
        let token = token_from(&request)?;
        let r = request.into_inner();
        // \"Create repo\" in the jj model is implicit (the first write to a
        // `(project, repo)` materialises it), so this RPC just echoes the
        // requested config. Even so, it must (a) authorize the caller and
        // (b) validate the project exists — without those, anonymous
        // callers could probe for project names and the call would lie.
        self.core
            .authorize_repo_action(token.as_deref(), Action::ManageRepo, &r.project, &r.name)
            .map_err(to_status)?;
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
        let token = token_from(&request)?;
        let r = request.into_inner();
        // Reads on a (project, repo) must still pass the read gate so
        // private projects don't leak repo existence to anonymous callers.
        self.core
            .authorize_repo_action(token.as_deref(), Action::Read, &r.project, &r.repo)
            .map_err(to_status)?;
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
        let token = token_from(&request)?;
        let r = request.into_inner();
        self.core
            .authorize_repo_action(token.as_deref(), Action::ManageRepo, &r.project, &r.repo)
            .map_err(to_status)?;
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
        request: Request<pb::ListReposRequest>,
    ) -> Result<Response<pb::ListReposResponse>, Status> {
        // Enumerating repos requires a persisted repo registry, which the
        // VCS layer doesn't keep — implicit-on-first-write means we can't
        // distinguish \"empty project\" from \"project with no writes yet\".
        // Returning \`vec![]\` would lie to clients; surface the gap.
        let _ = token_from(&request)?;
        Err(Status::unimplemented(
            "ListRepos is not supported by the implicit jj repo model in v1; \
             use ListBranches on a known (project, repo) instead",
        ))
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
        request: Request<pb::AddMemberRequest>,
    ) -> Result<Response<pb::AddMemberResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let role = role_from_proto(r.role)?;
        if r.identity.is_empty() {
            return Err(Status::invalid_argument("identity must not be empty"));
        }
        self.core
            .add_member(&r.project, &r.identity, role, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::AddMemberResponse {
            member: Some(pb::MemberEntry {
                identity: r.identity,
                role: r.role,
            }),
        }))
    }

    async fn remove_member(
        &self,
        request: Request<pb::RemoveMemberRequest>,
    ) -> Result<Response<pb::RemoveMemberResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        if r.identity.is_empty() {
            return Err(Status::invalid_argument("identity must not be empty"));
        }
        self.core
            .remove_member(&r.project, &r.identity, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::RemoveMemberResponse {}))
    }

    async fn update_member_role(
        &self,
        request: Request<pb::UpdateMemberRoleRequest>,
    ) -> Result<Response<pb::UpdateMemberRoleResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let new_role = role_from_proto(r.new_role)?;
        if r.identity.is_empty() {
            return Err(Status::invalid_argument("identity must not be empty"));
        }
        self.core
            .update_member_role(&r.project, &r.identity, new_role, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::UpdateMemberRoleResponse {
            member: Some(pb::MemberEntry {
                identity: r.identity,
                role: r.new_role,
            }),
        }))
    }

    async fn list_members(
        &self,
        request: Request<pb::ListMembersRequest>,
    ) -> Result<Response<pb::ListMembersResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let members = self
            .core
            .list_members(&r.project, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::ListMembersResponse {
            members: members
                .into_iter()
                .map(|(id, role)| pb::MemberEntry {
                    identity: id,
                    role: role_to_proto(role) as i32,
                })
                .collect(),
        }))
    }
}

// ── wire conversions ────────────────────────────────────────────────────────

fn meta_to_proto(meta: &schemahub_core::ProjectMeta) -> pb::ProjectInfo {
    pb::ProjectInfo {
        name: meta.name.clone(),
        is_public: matches!(meta.visibility, Visibility::Public),
        owner: meta.creator.clone(),
    }
}

fn role_from_proto(r: i32) -> Result<Role, Status> {
    match pb::Role::try_from(r) {
        Ok(pb::Role::Reader) => Ok(Role::Reader),
        Ok(pb::Role::Writer) => Ok(Role::Writer),
        Ok(pb::Role::Maintainer) => Ok(Role::Maintainer),
        Ok(pb::Role::Owner) => Ok(Role::Owner),
        Ok(pb::Role::Unspecified) | Err(_) => Err(Status::invalid_argument(
            "role must be Reader, Writer, Maintainer, or Owner",
        )),
    }
}

fn role_to_proto(r: Role) -> pb::Role {
    match r {
        Role::Reader => pb::Role::Reader,
        Role::Writer => pb::Role::Writer,
        Role::Maintainer => pb::Role::Maintainer,
        Role::Owner => pb::Role::Owner,
    }
}
