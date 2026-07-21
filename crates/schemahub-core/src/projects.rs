//! Project + member orchestration (design.md §6, §11). Thin authorized
//! wrappers over [`ProjectStore`] + [`RoleStore`]:
//!
//! - `CreateProject`: any authenticated identity may create; caller becomes
//!   the Owner.
//! - `GetProject` / `ListProjects`: gated by `Action::Read` (public projects
//!   are visible to anonymous; private projects only to members).
//! - Member management (`AddMember` / `RemoveMember` / `UpdateMemberRole`):
//!   Owner-only (`Action::ManageProject`). The "last Owner" invariant is
//!   enforced fail-fast — a project must always have ≥ 1 Owner.
//!
//! Server handlers in `crates/schemahub-server/src/services/project.rs` are
//! thin wire-conversion layers over the methods here.

use schemahub_types::{Action, AuthzError, Identity, ResourcePath, Role, Visibility};

use crate::auth::authorize;
use crate::auth_store::ProjectMeta;
use crate::error::{CoreError, CoreResult};
use crate::repository::now_unix_millis;
use crate::Core;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectUpdate {
    pub visibility: Option<Visibility>,
}

impl ProjectUpdate {
    pub fn is_empty(&self) -> bool {
        self.visibility.is_none()
    }
}

impl Core {
    // ── Projects ────────────────────────────────────────────────────────────

    /// Create a new project. The caller (resolved by the configured
    /// `AuthnProvider`) becomes the Owner. Anonymous callers are rejected —
    /// project creation always requires an authenticated identity.
    pub fn create_project(
        &self,
        name: &str,
        visibility: Visibility,
        token: Option<&str>,
    ) -> CoreResult<ProjectMeta> {
        validate_project_name(name)?;
        // Resolve the caller — `CreateProject` is special: authz can't run
        // (the project does not yet exist) so we authenticate and require a
        // non-anonymous identity directly.
        let caller = self.authn.identify(token)?;
        if caller.is_anonymous() {
            return Err(CoreError::Authz(AuthzError::PermissionDenied(
                "anonymous identities cannot create projects".to_string(),
            )));
        }
        let creator = caller.id().unwrap_or("").to_string();
        let meta = ProjectMeta::new(name, visibility, creator.clone(), now_unix_millis()?);
        Ok(self.project_store.create_with_owner(meta, &creator)?)
    }

    /// Lookup a project by name. Returns `Ok(None)` if missing; returns
    /// `PermissionDenied` if the caller can't `Read` it.
    pub fn get_project(&self, name: &str, token: Option<&str>) -> CoreResult<Option<ProjectMeta>> {
        self.get_project_with_archived(name, false, token)
    }

    /// Lookup a project, optionally including an archived record. Archived
    /// records are visible only to project Owners.
    pub fn get_project_with_archived(
        &self,
        name: &str,
        include_archived: bool,
        token: Option<&str>,
    ) -> CoreResult<Option<ProjectMeta>> {
        validate_project_name(name)?;
        let Some(meta) = self.project_store.get(name)? else {
            return Ok(None);
        };
        if meta.archived && !include_archived {
            return Ok(None);
        }
        let identity = self.authn.identify(token)?;
        self.authorize_project_read(&meta, &identity)?;
        Ok(Some(meta))
    }

    /// List every project the caller can `Read` (public + private-where-member).
    /// Anonymous callers see public projects only.
    pub fn list_projects(&self, token: Option<&str>) -> CoreResult<Vec<ProjectMeta>> {
        self.list_projects_with_archived(false, token)
    }

    /// List readable projects. Archived projects are excluded by default and,
    /// when explicitly requested, are visible only to their Owners.
    pub fn list_projects_with_archived(
        &self,
        include_archived: bool,
        token: Option<&str>,
    ) -> CoreResult<Vec<ProjectMeta>> {
        let identity = self.authn.identify(token)?;
        let all = self.project_store.list()?;
        let mut visible = Vec::new();
        for meta in all {
            if meta.archived && !include_archived {
                continue;
            }
            match self.authorize_project_read(&meta, &identity) {
                Ok(()) => visible.push(meta),
                Err(CoreError::Authz(AuthzError::PermissionDenied(_))) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(visible)
    }

    /// Update mutable project metadata with optimistic concurrency. Project
    /// names, creators, timestamps, and archive state are output-only.
    pub fn update_project(
        &self,
        name: &str,
        expected_etag: &str,
        patch: ProjectUpdate,
        token: Option<&str>,
    ) -> CoreResult<ProjectMeta> {
        validate_project_name(name)?;
        validate_project_etag(expected_etag)?;
        if patch.is_empty() {
            return Err(CoreError::InvalidArgument(
                "update mask selects no project fields".to_string(),
            ));
        }
        let identity = self.authn.identify(token)?;
        self.authz.check(
            &identity,
            Action::ManageProject,
            &ResourcePath::project(name),
        )?;
        let mut meta = self
            .project_store
            .get(name)?
            .ok_or_else(|| crate::AccessStoreError::NotFound(name.to_string()))?;
        if meta.archived {
            return Err(CoreError::FailedPrecondition(
                "an archived project cannot be updated".to_string(),
            ));
        }
        if let Some(visibility) = patch.visibility {
            meta.visibility = visibility;
        }
        meta.update_time_unix_ms = now_unix_millis()?;
        Ok(self.project_store.replace(expected_etag, meta)?)
    }

    /// Soft-delete a project registry entry. Repositories and JJ history are
    /// retained. Projects containing repository records require `force=true`.
    pub fn archive_project(
        &self,
        name: &str,
        expected_etag: &str,
        force: bool,
        token: Option<&str>,
    ) -> CoreResult<ProjectMeta> {
        validate_project_name(name)?;
        validate_project_etag(expected_etag)?;
        let mut meta = self
            .project_store
            .get(name)?
            .ok_or_else(|| crate::AccessStoreError::NotFound(name.to_string()))?;
        let identity = self.authn.identify(token)?;
        self.authorize_project_owner(name, &identity)?;
        if meta.archived {
            if meta.etag != expected_etag {
                return Err(crate::AccessStoreError::EtagMismatch {
                    name: name.to_string(),
                    expected: expected_etag.to_string(),
                    current: meta.etag,
                }
                .into());
            }
            return Ok(meta);
        }
        if !force && !self.repository_store.list(name)?.is_empty() {
            return Err(CoreError::FailedPrecondition(
                "project has repository records; set force=true to archive while retaining them"
                    .to_string(),
            ));
        }
        let now = now_unix_millis()?;
        meta.archived = true;
        meta.archive_time_unix_ms = Some(now);
        meta.update_time_unix_ms = now;
        Ok(self.project_store.replace(expected_etag, meta)?)
    }

    // ── Members ─────────────────────────────────────────────────────────────

    /// Add (or replace, if already a member) a project member. Owner-only.
    pub fn add_member(
        &self,
        project: &str,
        identity_id: &str,
        role: Role,
        token: Option<&str>,
    ) -> CoreResult<()> {
        self.ensure_project_exists(project)?;
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::ManageProject,
            project,
            "",
        )?;
        self.role_store.set(project, identity_id, role)?;
        Ok(())
    }

    /// Remove a project member. Owner-only. Fails fast if removing this member
    /// would leave the project with zero Owners.
    pub fn remove_member(
        &self,
        project: &str,
        identity_id: &str,
        token: Option<&str>,
    ) -> CoreResult<()> {
        self.ensure_project_exists(project)?;
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::ManageProject,
            project,
            "",
        )?;
        self.guard_last_owner(project, identity_id, /*removing=*/ true, None)?;
        self.role_store.remove(project, identity_id)?;
        Ok(())
    }

    /// Change a project member's role. Owner-only. Fails fast if downgrading
    /// the last remaining Owner.
    pub fn update_member_role(
        &self,
        project: &str,
        identity_id: &str,
        new_role: Role,
        token: Option<&str>,
    ) -> CoreResult<()> {
        self.ensure_project_exists(project)?;
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::ManageProject,
            project,
            "",
        )?;
        self.guard_last_owner(
            project,
            identity_id,
            /*removing=*/ false,
            Some(new_role),
        )?;
        self.role_store.set(project, identity_id, new_role)?;
        Ok(())
    }

    /// List members of a project (gated by `Action::Read`).
    pub fn list_members(
        &self,
        project: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<(String, Role)>> {
        self.ensure_project_exists(project)?;
        let identity = self.authn.identify(token)?;
        self.authz
            .check(&identity, Action::Read, &ResourcePath::project(project))?;
        Ok(self.role_store.list_project(project)?)
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    pub(crate) fn ensure_project_exists(&self, project: &str) -> CoreResult<()> {
        let meta = self
            .project_store
            .get(project)?
            .ok_or_else(|| crate::AccessStoreError::NotFound(project.to_string()))?;
        if meta.archived {
            return Err(CoreError::FailedPrecondition(format!(
                "project '{project}' is archived"
            )));
        }
        Ok(())
    }

    fn authorize_project_read(&self, meta: &ProjectMeta, identity: &Identity) -> CoreResult<()> {
        if meta.archived {
            return self.authorize_project_owner(&meta.name, identity);
        }
        Ok(self
            .authz
            .check(identity, Action::Read, &ResourcePath::project(&meta.name))?)
    }

    fn authorize_project_owner(&self, project: &str, identity: &Identity) -> CoreResult<()> {
        let Some(identity_id) = identity.id() else {
            return Err(CoreError::Authz(AuthzError::PermissionDenied(format!(
                "anonymous access denied for archived project '{project}'"
            ))));
        };
        match self.role_store.get(project, identity)? {
            Some(Role::Owner) => Ok(()),
            Some(role) => Err(CoreError::Authz(AuthzError::PermissionDenied(format!(
                "role {role:?} cannot manage project '{project}' (need Owner)"
            )))),
            None => Err(CoreError::Authz(AuthzError::PermissionDenied(format!(
                "caller '{identity_id}' is not an Owner of project '{project}'"
            )))),
        }
    }

    /// Enforce the "every project must have ≥ 1 Owner" invariant.
    ///
    /// - `removing = true`: the candidate is being removed.
    /// - `removing = false`: the candidate's role is being set to `new_role`.
    ///   A downgrade from Owner to anything else counts as removing an Owner.
    fn guard_last_owner(
        &self,
        project: &str,
        candidate_id: &str,
        removing: bool,
        new_role: Option<Role>,
    ) -> CoreResult<()> {
        let members = self.role_store.list_project(project)?;
        let owners: Vec<&str> = members
            .iter()
            .filter(|(_, r)| *r == Role::Owner)
            .map(|(id, _)| id.as_str())
            .collect();

        let candidate_is_owner = owners.contains(&candidate_id);
        if !candidate_is_owner {
            return Ok(());
        }
        let losing_owner = removing || matches!(new_role, Some(r) if r != Role::Owner);
        if losing_owner && owners.len() <= 1 {
            return Err(CoreError::Authz(AuthzError::PermissionDenied(format!(
                "refusing to leave project '{project}' with zero Owners"
            ))));
        }
        Ok(())
    }
}

fn validate_project_name(name: &str) -> CoreResult<()> {
    if name.trim().is_empty()
        || name.contains('/')
        || name.chars().any(char::is_control)
        || name.len() > 128
    {
        return Err(CoreError::InvalidArgument(
            "project name must be a 1-128 character resource path segment without control characters"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_project_etag(etag: &str) -> CoreResult<()> {
    if etag.is_empty() {
        return Err(CoreError::InvalidArgument(
            "project etag must not be empty".to_string(),
        ));
    }
    Ok(())
}

/// Resolve the caller's identity using the core's authn provider. Exposed for
/// thin server handlers that need the resolved `Identity` (e.g. to default a
/// commit author or surface "you" in the `ProjectInfo.owner` field).
impl Core {
    pub fn resolve_identity(&self, token: Option<&str>) -> CoreResult<Identity> {
        Ok(self.authn.identify(token)?)
    }
}
