//! `ProjectService` — projects, repos, members (design.md §6, §11).
//!
//! Project + member management is wired to the [`Core`] project/role
//! orchestration (see `crates/schemahub-core/src/projects.rs`):
//! `CreateProject`, `GetProject`, `ListProjects`, `AddMember`,
//! `RemoveMember`, `UpdateMemberRole`, `ListMembers` all flow through Core
//! which runs the configured `AuthnProvider` + `AuthzPolicy` before touching
//! the role / project stores.
//!
//! Repository lifecycle uses a durable resource store over the configured
//! redb/PostgreSQL backend. Updates use field masks plus ETags; deletion is an
//! auditable archive that retains JJ history.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;

use schemahub_core::{
    Core, CreateRepository, ProjectUpdate, RepoConfig, Repository, RepositoryUpdate, ReviewPolicy,
    ServingPolicy,
};
use schemahub_types::{CompatibilityDirection, Role, Visibility};
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
            .get_project_with_archived(&r.name, r.include_archived, token.as_deref())
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found(format!("project '{}' not found", r.name)))?;
        Ok(Response::new(pb::GetProjectResponse {
            project: Some(meta_to_proto(&meta)),
        }))
    }

    async fn update_project(
        &self,
        request: Request<pb::UpdateProjectRequest>,
    ) -> Result<Response<pb::UpdateProjectResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let resource = r
            .project
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("project resource must be provided"))?;
        let mask = r
            .update_mask
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("update_mask must be provided"))?;
        let patch = project_patch(resource, &mask.paths)?;
        let meta = self
            .core
            .update_project(&resource.name, &resource.etag, patch, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::UpdateProjectResponse {
            project: Some(meta_to_proto(&meta)),
        }))
    }

    async fn list_projects(
        &self,
        request: Request<pb::ListProjectsRequest>,
    ) -> Result<Response<pb::ListProjectsResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let limit = page_size(r.page_size)?;
        let prefix = r.name_prefix;
        let cursor = parse_project_page_token(&r.page_token, &prefix, r.include_archived)?;
        let mut visible = self
            .core
            .list_projects_with_archived(r.include_archived, token.as_deref())
            .map_err(to_status)?;
        visible.retain(|meta| {
            (prefix.is_empty() || meta.name.starts_with(&prefix))
                && cursor
                    .as_ref()
                    .is_none_or(|cursor| meta.name.cmp(cursor) == Ordering::Greater)
        });
        let has_more = visible.len() > limit;
        visible.truncate(limit);
        let next_page_token = if has_more {
            visible
                .last()
                .map(|meta| make_project_page_token(&prefix, r.include_archived, &meta.name))
                .unwrap_or_default()
        } else {
            String::new()
        };
        Ok(Response::new(pb::ListProjectsResponse {
            projects: visible.iter().map(meta_to_proto).collect(),
            next_page_token,
        }))
    }

    async fn delete_project(
        &self,
        request: Request<pb::DeleteProjectRequest>,
    ) -> Result<Response<pb::DeleteProjectResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        self.core
            .archive_project(&r.name, &r.etag, r.force, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::DeleteProjectResponse {}))
    }

    async fn create_repo(
        &self,
        request: Request<pb::CreateRepoRequest>,
    ) -> Result<Response<pb::CreateRepoResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let config = create_repo_config(&r)?;
        let repository = self
            .core
            .create_repository(
                CreateRepository {
                    project: r.project,
                    name: r.name,
                    config,
                },
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(pb::CreateRepoResponse {
            repo: Some(repository_to_proto(repository)),
        }))
    }

    async fn get_repo(
        &self,
        request: Request<pb::GetRepoRequest>,
    ) -> Result<Response<pb::GetRepoResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let repository = self
            .core
            .get_repository(&r.project, &r.repo, r.include_archived, token.as_deref())
            .map_err(to_status)?
            .ok_or_else(|| {
                Status::not_found(format!("repository {}/{} not found", r.project, r.repo))
            })?;
        Ok(Response::new(pb::GetRepoResponse {
            repo: Some(repository_to_proto(repository)),
        }))
    }

    async fn update_repo(
        &self,
        request: Request<pb::UpdateRepoRequest>,
    ) -> Result<Response<pb::UpdateRepoResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let resource = r.repo_config.as_ref().ok_or_else(|| {
            Status::invalid_argument("repo_config resource must be provided for update")
        })?;
        if resource.project != r.project || resource.name != r.repo {
            return Err(Status::invalid_argument(
                "repo_config project/name must match request project/repo",
            ));
        }
        let mask = r
            .update_mask
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("update_mask must be provided"))?;
        let patch = repository_patch(resource, &mask.paths)?;
        let repository = self
            .core
            .update_repository(&r.project, &r.repo, &resource.etag, patch, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::UpdateRepoResponse {
            repo: Some(repository_to_proto(repository)),
        }))
    }

    async fn list_repos(
        &self,
        request: Request<pb::ListReposRequest>,
    ) -> Result<Response<pb::ListReposResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let limit = page_size(r.page_size)?;
        let cursor = parse_repo_page_token(
            &r.page_token,
            &r.project,
            &r.name_prefix,
            r.include_archived,
        )?;
        let mut repositories = self
            .core
            .list_repositories(&r.project, r.include_archived, token.as_deref())
            .map_err(to_status)?;
        repositories.retain(|repository| {
            (r.name_prefix.is_empty() || repository.name.starts_with(&r.name_prefix))
                && cursor
                    .as_ref()
                    .is_none_or(|cursor| repository.name.cmp(cursor) == Ordering::Greater)
        });
        let has_more = repositories.len() > limit;
        repositories.truncate(limit);
        let next_page_token = if has_more {
            repositories
                .last()
                .map(|repository| {
                    make_repo_page_token(
                        &r.project,
                        &r.name_prefix,
                        r.include_archived,
                        &repository.name,
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        Ok(Response::new(pb::ListReposResponse {
            repos: repositories.into_iter().map(repository_to_proto).collect(),
            next_page_token,
        }))
    }

    async fn delete_repo(
        &self,
        request: Request<pb::DeleteRepoRequest>,
    ) -> Result<Response<pb::DeleteRepoResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        self.core
            .archive_repository(&r.project, &r.repo, &r.etag, r.force, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::DeleteRepoResponse {}))
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

const DEFAULT_REPO_PAGE_SIZE: usize = 50;
const MAX_REPO_PAGE_SIZE: usize = 200;

fn project_patch(project: &pb::ProjectInfo, paths: &[String]) -> Result<ProjectUpdate, Status> {
    if paths.is_empty() {
        return Err(Status::invalid_argument(
            "update_mask must select at least one field",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut patch = ProjectUpdate::default();
    for path in paths {
        if !seen.insert(path.as_str()) {
            return Err(Status::invalid_argument(format!(
                "update_mask contains duplicate path {path:?}"
            )));
        }
        match path.as_str() {
            "is_public" => {
                patch.visibility = Some(if project.is_public {
                    Visibility::Public
                } else {
                    Visibility::Private
                });
            }
            "name" | "owner" | "etag" | "create_time" | "update_time" | "archived"
            | "archive_time" => {
                return Err(Status::invalid_argument(format!(
                    "update_mask path {path:?} is output-only"
                )))
            }
            _ => {
                return Err(Status::invalid_argument(format!(
                    "unsupported update_mask path {path:?}"
                )))
            }
        }
    }
    Ok(patch)
}

fn create_repo_config(request: &pb::CreateRepoRequest) -> Result<RepoConfig, Status> {
    let compatibility_direction =
        match pb::CompatibilityDirection::try_from(request.compatibility_direction) {
            Ok(pb::CompatibilityDirection::Unspecified) => CompatibilityDirection::Full,
            Ok(direction) => compatibility_direction_from_proto(direction)?,
            Err(_) => {
                return Err(Status::invalid_argument(format!(
                    "unknown compatibility_direction value {}",
                    request.compatibility_direction
                )))
            }
        };
    Ok(RepoConfig {
        default_bookmark: if request.default_branch.is_empty() {
            "main".to_string()
        } else {
            request.default_branch.clone()
        },
        compatibility_direction,
        protected_bookmarks: if request.protected_branches.is_empty() {
            vec!["main".to_string()]
        } else {
            request.protected_branches.clone()
        },
        review_policy: request
            .review_policy
            .as_ref()
            .map(review_policy_from_proto)
            .unwrap_or_default(),
        serving_policy: request
            .serving_policy
            .as_ref()
            .map(serving_policy_from_proto)
            .unwrap_or_default(),
    })
}

fn repository_patch(
    repository: &pb::RepoConfig,
    paths: &[String],
) -> Result<RepositoryUpdate, Status> {
    if paths.is_empty() {
        return Err(Status::invalid_argument(
            "update_mask must select at least one field",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut patch = RepositoryUpdate::default();
    for path in paths {
        if !seen.insert(path.as_str()) {
            return Err(Status::invalid_argument(format!(
                "update_mask contains duplicate path {path:?}"
            )));
        }
        match path.as_str() {
            "default_branch" => {
                patch.default_bookmark = Some(repository.default_branch.clone());
            }
            "compatibility_direction" => {
                let direction =
                    pb::CompatibilityDirection::try_from(repository.compatibility_direction)
                        .map_err(|_| {
                            Status::invalid_argument(format!(
                                "unknown compatibility_direction value {}",
                                repository.compatibility_direction
                            ))
                        })?;
                patch.compatibility_direction =
                    Some(compatibility_direction_from_proto(direction)?);
            }
            "protected_branches" => {
                patch.protected_bookmarks = Some(repository.protected_branches.clone());
            }
            "review_policy" => {
                patch.review_policy = Some(review_policy_from_proto(
                    repository.review_policy.as_ref().ok_or_else(|| {
                        Status::invalid_argument(
                            "repo_config.review_policy is required by update_mask",
                        )
                    })?,
                ));
            }
            "serving_policy" => {
                patch.serving_policy = Some(serving_policy_from_proto(
                    repository.serving_policy.as_ref().ok_or_else(|| {
                        Status::invalid_argument(
                            "repo_config.serving_policy is required by update_mask",
                        )
                    })?,
                ));
            }
            "project" | "name" | "etag" | "create_time" | "update_time" | "archived"
            | "archive_time" => {
                return Err(Status::invalid_argument(format!(
                    "update_mask path {path:?} is output-only"
                )))
            }
            _ => {
                return Err(Status::invalid_argument(format!(
                    "unsupported update_mask path {path:?}"
                )))
            }
        }
    }
    Ok(patch)
}

fn compatibility_direction_from_proto(
    direction: pb::CompatibilityDirection,
) -> Result<CompatibilityDirection, Status> {
    match direction {
        pb::CompatibilityDirection::Backward => Ok(CompatibilityDirection::Backward),
        pb::CompatibilityDirection::Forward => Ok(CompatibilityDirection::Forward),
        pb::CompatibilityDirection::Full => Ok(CompatibilityDirection::Full),
        pb::CompatibilityDirection::Disabled => Ok(CompatibilityDirection::Disabled),
        pb::CompatibilityDirection::Unspecified => Err(Status::invalid_argument(
            "compatibility_direction must not be unspecified",
        )),
    }
}

fn compatibility_direction_to_proto(
    direction: CompatibilityDirection,
) -> pb::CompatibilityDirection {
    match direction {
        CompatibilityDirection::Backward => pb::CompatibilityDirection::Backward,
        CompatibilityDirection::Forward => pb::CompatibilityDirection::Forward,
        CompatibilityDirection::Full => pb::CompatibilityDirection::Full,
        CompatibilityDirection::Disabled => pb::CompatibilityDirection::Disabled,
    }
}

fn review_policy_from_proto(policy: &pb::ReviewPolicy) -> ReviewPolicy {
    ReviewPolicy {
        required_approvals: policy.required_approvals,
        require_change_record: policy.require_change_record,
    }
}

fn serving_policy_from_proto(policy: &pb::ServingPolicy) -> ServingPolicy {
    ServingPolicy {
        source: policy.source,
        descriptors: policy.descriptors,
        generated_code: policy.generated_code,
    }
}

fn repository_to_proto(repository: Repository) -> pb::RepoConfig {
    pb::RepoConfig {
        project: repository.project,
        name: repository.name,
        default_branch: repository.config.default_bookmark,
        compatibility_direction: compatibility_direction_to_proto(
            repository.config.compatibility_direction,
        ) as i32,
        protected_branches: repository.config.protected_bookmarks,
        review_policy: Some(pb::ReviewPolicy {
            required_approvals: repository.config.review_policy.required_approvals,
            require_change_record: repository.config.review_policy.require_change_record,
        }),
        serving_policy: Some(pb::ServingPolicy {
            source: repository.config.serving_policy.source,
            descriptors: repository.config.serving_policy.descriptors,
            generated_code: repository.config.serving_policy.generated_code,
        }),
        etag: repository.etag,
        create_time: Some(timestamp_from_millis(repository.create_time_unix_ms)),
        update_time: Some(timestamp_from_millis(repository.update_time_unix_ms)),
        archived: repository.archived,
        archive_time: repository.archive_time_unix_ms.map(timestamp_from_millis),
    }
}

fn page_size(requested: i32) -> Result<usize, Status> {
    if requested < 0 {
        return Err(Status::invalid_argument("page_size must not be negative"));
    }
    Ok(if requested == 0 {
        DEFAULT_REPO_PAGE_SIZE
    } else {
        (requested as usize).min(MAX_REPO_PAGE_SIZE)
    })
}

fn parse_project_page_token(
    token: &str,
    name_prefix: &str,
    include_archived: bool,
) -> Result<Option<String>, Status> {
    if token.is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = token.splitn(4, ':').collect();
    let decoded = |value: &str| {
        hex::decode(value)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    let archived = if include_archived { "1" } else { "0" };
    if parts.len() != 4
        || parts[0] != "v1"
        || parts[1] != archived
        || decoded(parts[2]).as_deref() != Some(name_prefix)
    {
        return Err(Status::invalid_argument(
            "page_token is invalid for this project filter",
        ));
    }
    decoded(parts[3])
        .filter(|name| !name.is_empty())
        .map(Some)
        .ok_or_else(|| Status::invalid_argument("page_token has an invalid project cursor"))
}

fn make_project_page_token(name_prefix: &str, include_archived: bool, last_name: &str) -> String {
    format!(
        "v1:{}:{}:{}",
        u8::from(include_archived),
        hex::encode(name_prefix),
        hex::encode(last_name)
    )
}

fn parse_repo_page_token(
    token: &str,
    project: &str,
    name_prefix: &str,
    include_archived: bool,
) -> Result<Option<String>, Status> {
    if token.is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = token.splitn(5, ':').collect();
    let decoded = |value: &str| {
        hex::decode(value)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    let archived = if include_archived { "1" } else { "0" };
    if parts.len() != 5
        || parts[0] != "v1"
        || parts[1] != archived
        || decoded(parts[2]).as_deref() != Some(project)
        || decoded(parts[3]).as_deref() != Some(name_prefix)
    {
        return Err(Status::invalid_argument(
            "page_token is invalid for this project or filter",
        ));
    }
    decoded(parts[4])
        .filter(|name| !name.is_empty())
        .map(Some)
        .ok_or_else(|| Status::invalid_argument("page_token has an invalid repository cursor"))
}

fn make_repo_page_token(
    project: &str,
    name_prefix: &str,
    include_archived: bool,
    last_name: &str,
) -> String {
    format!(
        "v1:{}:{}:{}:{}",
        u8::from(include_archived),
        hex::encode(project),
        hex::encode(name_prefix),
        hex::encode(last_name)
    )
}

fn timestamp_from_millis(millis: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: millis.div_euclid(1_000),
        nanos: (millis.rem_euclid(1_000) * 1_000_000) as i32,
    }
}

// ── wire conversions ────────────────────────────────────────────────────────

fn meta_to_proto(meta: &schemahub_core::ProjectMeta) -> pb::ProjectInfo {
    pb::ProjectInfo {
        name: meta.name.clone(),
        is_public: matches!(meta.visibility, Visibility::Public),
        owner: meta.creator.clone(),
        etag: meta.etag.clone(),
        create_time: Some(timestamp_from_millis(meta.create_time_unix_ms)),
        update_time: Some(timestamp_from_millis(meta.update_time_unix_ms)),
        archived: meta.archived,
        archive_time: meta.archive_time_unix_ms.map(timestamp_from_millis),
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
