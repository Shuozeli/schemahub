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
use crate::Core;

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
        // Resolve the caller — `CreateProject` is special: authz can't run
        // (the project does not yet exist) so we authenticate and require a
        // non-anonymous identity directly.
        let caller = self.authn.identify(token)?;
        if caller.is_anonymous() {
            return Err(CoreError::Authz(AuthzError::PermissionDenied(
                "anonymous identities cannot create projects".to_string(),
            )));
        }
        if self.project_store.get(name).is_some() {
            return Err(CoreError::Other(format!(
                "project '{name}' already exists"
            )));
        }
        let creator = caller.id().unwrap_or("").to_string();
        let meta = ProjectMeta {
            name: name.to_string(),
            visibility,
            creator: creator.clone(),
        };
        self.project_store
            .set(meta.clone())
            .map_err(|e| CoreError::Other(format!("write project store: {e}")))?;
        // Bootstrap: the creator becomes the Owner.
        self.role_store
            .set(name, &creator, Role::Owner)
            .map_err(|e| CoreError::Other(format!("write role store: {e}")))?;
        Ok(meta)
    }

    /// Lookup a project by name. Returns `Ok(None)` if missing; returns
    /// `PermissionDenied` if the caller can't `Read` it.
    pub fn get_project(
        &self,
        name: &str,
        token: Option<&str>,
    ) -> CoreResult<Option<ProjectMeta>> {
        let Some(meta) = self.project_store.get(name) else {
            return Ok(None);
        };
        let identity = self.authn.identify(token)?;
        self.authz
            .check(&identity, Action::Read, &ResourcePath::project(name))?;
        Ok(Some(meta))
    }

    /// List every project the caller can `Read` (public + private-where-member).
    /// Anonymous callers see public projects only.
    pub fn list_projects(&self, token: Option<&str>) -> CoreResult<Vec<ProjectMeta>> {
        let identity = self.authn.identify(token)?;
        let all = self.project_store.list();
        let visible: Vec<ProjectMeta> = all
            .into_iter()
            .filter(|m| {
                self.authz
                    .check(&identity, Action::Read, &ResourcePath::project(&m.name))
                    .is_ok()
            })
            .collect();
        Ok(visible)
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
        self.role_store
            .set(project, identity_id, role)
            .map_err(|e| CoreError::Other(format!("write role store: {e}")))?;
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
        self.role_store
            .remove(project, identity_id)
            .map_err(|e| CoreError::Other(format!("write role store: {e}")))?;
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
        self.guard_last_owner(project, identity_id, /*removing=*/ false, Some(new_role))?;
        self.role_store
            .set(project, identity_id, new_role)
            .map_err(|e| CoreError::Other(format!("write role store: {e}")))?;
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
        Ok(self.role_store.list_project(project))
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    fn ensure_project_exists(&self, project: &str) -> CoreResult<()> {
        if self.project_store.get(project).is_none() {
            return Err(CoreError::Other(format!(
                "project '{project}' does not exist"
            )));
        }
        Ok(())
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
        let members = self.role_store.list_project(project);
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

/// Resolve the caller's identity using the core's authn provider. Exposed for
/// thin server handlers that need the resolved `Identity` (e.g. to default a
/// commit author or surface "you" in the `ProjectInfo.owner` field).
impl Core {
    pub fn resolve_identity(&self, token: Option<&str>) -> CoreResult<Identity> {
        Ok(self.authn.identify(token)?)
    }
}
