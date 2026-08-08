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
use crate::auth_store::{ProjectMeta, ProjectStorePage, RoleStorePage};
use crate::error::{CoreError, CoreResult};
use crate::repository::now_unix_millis;
use crate::Core;

const DEFAULT_INTERNAL_PROJECT_PAGE_SIZE: usize = 256;
const DEFAULT_INTERNAL_MEMBER_PAGE_SIZE: usize = 256;

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
        let _guard = self.acquire_control_plane_guard(name)?;
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
        let audit = self.control_plane_audit_context(&creator)?;
        Ok(self
            .project_store
            .create_with_owner_audited(meta, &creator, &audit)?)
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
        let mut visible = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.project_store.list_page(
                include_archived,
                "",
                cursor.as_deref(),
                DEFAULT_INTERNAL_PROJECT_PAGE_SIZE,
            )?;
            for meta in page.projects {
                match self.authorize_project_read(&meta, &identity) {
                    Ok(()) => visible.push(meta),
                    Err(CoreError::Authz(AuthzError::PermissionDenied(_))) => {}
                    Err(error) => return Err(error),
                }
            }
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(visible)
    }

    /// Read one authorization-filtered project page while bounding the
    /// underlying catalog scan. A page may contain fewer than `limit` entries
    /// (including zero) and still carry a continuation when the bounded scan
    /// crossed only projects hidden from this caller.
    pub fn list_projects_page(
        &self,
        include_archived: bool,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> CoreResult<ProjectStorePage> {
        validate_project_filter(name_prefix, start_after)?;
        if limit == 0 {
            return Ok(ProjectStorePage {
                projects: Vec::new(),
                next_cursor: None,
            });
        }
        let identity = self.authn.identify(token)?;
        let scan_limit = limit.saturating_mul(4).clamp(64, 1_000);
        let scanned =
            self.project_store
                .list_page(include_archived, name_prefix, start_after, scan_limit)?;
        let mut visible = Vec::new();
        for meta in scanned.projects {
            match self.authorize_project_read(&meta, &identity) {
                Ok(()) => {
                    visible.push(meta);
                    if visible.len() > limit {
                        break;
                    }
                }
                Err(CoreError::Authz(AuthzError::PermissionDenied(_))) => {}
                Err(error) => return Err(error),
            }
        }
        if visible.len() > limit {
            visible.truncate(limit);
            return Ok(ProjectStorePage {
                next_cursor: visible.last().map(|meta| meta.name.clone()),
                projects: visible,
            });
        }
        Ok(ProjectStorePage {
            projects: visible,
            next_cursor: scanned.next_cursor,
        })
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
        let _guard = self.acquire_control_plane_guard(name)?;
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
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        Ok(self
            .project_store
            .replace_audited(expected_etag, meta, &audit)?)
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
        let _guard = self.acquire_control_plane_guard(name)?;
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
        if !force
            && !self
                .repository_store
                .list_page(name, true, "", None, 1)?
                .repositories
                .is_empty()
        {
            return Err(CoreError::FailedPrecondition(
                "project has repository records; set force=true to archive while retaining them"
                    .to_string(),
            ));
        }
        let now = now_unix_millis()?;
        meta.archived = true;
        meta.archive_time_unix_ms = Some(now);
        meta.update_time_unix_ms = now;
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        Ok(self
            .project_store
            .replace_audited(expected_etag, meta, &audit)?)
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
        validate_project_name(project)?;
        validate_member_identity(identity_id)?;
        let _guard = self.acquire_control_plane_guard(project)?;
        self.ensure_project_exists(project)?;
        let identity = authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::ManageProject,
            project,
            "",
        )?;
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        self.role_store
            .set_audited(project, identity_id, role, &audit)?;
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
        validate_project_name(project)?;
        validate_member_identity(identity_id)?;
        let _guard = self.acquire_control_plane_guard(project)?;
        self.ensure_project_exists(project)?;
        let identity = authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::ManageProject,
            project,
            "",
        )?;
        self.guard_last_owner(project, identity_id, /*removing=*/ true, None)?;
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        self.role_store
            .remove_audited(project, identity_id, &audit)?;
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
        validate_project_name(project)?;
        validate_member_identity(identity_id)?;
        let _guard = self.acquire_control_plane_guard(project)?;
        self.ensure_project_exists(project)?;
        let identity = authorize(
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
        let audit = self.control_plane_audit_context(identity.id().unwrap_or("anonymous"))?;
        self.role_store
            .set_audited(project, identity_id, new_role, &audit)?;
        Ok(())
    }

    /// List members of a project (gated by `Action::Read`).
    pub fn list_members(
        &self,
        project: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<(String, Role)>> {
        validate_project_name(project)?;
        self.ensure_project_exists(project)?;
        let identity = self.authn.identify(token)?;
        self.authz
            .check(&identity, Action::Read, &ResourcePath::project(project))?;
        let mut members = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.role_store.list_project_page(
                project,
                cursor.as_deref(),
                DEFAULT_INTERNAL_MEMBER_PAGE_SIZE,
            )?;
            members.extend(page.members);
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(members)
    }

    /// Read one bounded, identity-ordered member page after authorizing the
    /// caller for the parent project. Inactive membership tombstones may
    /// produce an empty page with a continuation.
    pub fn list_members_page(
        &self,
        project: &str,
        start_after: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> CoreResult<RoleStorePage> {
        validate_project_name(project)?;
        if let Some(cursor) = start_after {
            validate_member_identity(cursor)?;
        }
        self.ensure_project_exists(project)?;
        let identity = self.authn.identify(token)?;
        self.authz
            .check(&identity, Action::Read, &ResourcePath::project(project))?;
        Ok(self
            .role_store
            .list_project_page(project, start_after, limit)?)
    }

    /// Return only the authenticated caller's role after checking that the
    /// project is readable. This avoids enumerating the membership catalog for
    /// UI summaries.
    pub fn caller_project_role(
        &self,
        project: &str,
        token: Option<&str>,
    ) -> CoreResult<Option<Role>> {
        validate_project_name(project)?;
        self.ensure_project_exists(project)?;
        let identity = self.authn.identify(token)?;
        self.authz
            .check(&identity, Action::Read, &ResourcePath::project(project))?;
        Ok(self.role_store.get(project, &identity)?)
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

    pub(crate) fn acquire_control_plane_guard(
        &self,
        project: &str,
    ) -> CoreResult<Box<dyn schemahub_jj::ObjectDbLockGuard + '_>> {
        let key = format!("schemahub-control-plane/projects/{project}");
        self.control_plane_db
            .acquire_publication_guard(&key)
            .map_err(|error| {
                CoreError::Other(format!(
                    "acquiring project control-plane coordination lock: {error}"
                ))
            })
    }

    fn authorize_project_read(&self, meta: &ProjectMeta, identity: &Identity) -> CoreResult<()> {
        if meta.archived {
            return self.authorize_project_owner(&meta.name, identity);
        }
        Ok(self
            .authz
            .check(identity, Action::Read, &ResourcePath::project(&meta.name))?)
    }

    pub(crate) fn authorize_project_owner(
        &self,
        project: &str,
        identity: &Identity,
    ) -> CoreResult<()> {
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

pub(crate) fn validate_project_name(name: &str) -> CoreResult<()> {
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

fn validate_member_identity(identity_id: &str) -> CoreResult<()> {
    if identity_id.is_empty()
        || identity_id.len() > 512
        || identity_id.chars().any(char::is_control)
    {
        return Err(CoreError::InvalidArgument(
            "member identity must be a 1-512 byte opaque value without control characters"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_project_filter(name_prefix: &str, start_after: Option<&str>) -> CoreResult<()> {
    if name_prefix.contains('/')
        || name_prefix.chars().any(char::is_control)
        || name_prefix.len() > 128
    {
        return Err(CoreError::InvalidArgument(
            "project name prefix must be at most 128 characters without '/' or control characters"
                .to_string(),
        ));
    }
    if let Some(cursor) = start_after {
        validate_project_name(cursor)?;
        if !cursor.starts_with(name_prefix) {
            return Err(CoreError::InvalidArgument(
                "project page cursor is outside the requested name prefix".to_string(),
            ));
        }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use schemahub_jj::{Jj, MemoryObjectDb, ObjectDb};
    use schemahub_types::{Identity, NoopAuthn, NoopAuthz};

    use super::*;
    use crate::{
        AccessStoreResult, BearerTokenAuthn, CompilerRegistry, ObjectDbProjectStore,
        ObjectDbRoleStore, ProjectStore, RepoConfigStore, RoleBasedAuthz, RoleStore,
    };

    struct SlowRoleStore {
        inner: ObjectDbRoleStore,
        active_lists: AtomicUsize,
        max_active_lists: AtomicUsize,
    }

    impl SlowRoleStore {
        fn new(db: Arc<dyn ObjectDb>) -> Self {
            Self {
                inner: ObjectDbRoleStore::new(db),
                active_lists: AtomicUsize::new(0),
                max_active_lists: AtomicUsize::new(0),
            }
        }
    }

    impl RoleStore for SlowRoleStore {
        fn get(&self, project: &str, identity: &Identity) -> AccessStoreResult<Option<Role>> {
            self.inner.get(project, identity)
        }

        fn set(&self, project: &str, identity_id: &str, role: Role) -> AccessStoreResult<()> {
            self.inner.set(project, identity_id, role)
        }

        fn remove(&self, project: &str, identity_id: &str) -> AccessStoreResult<()> {
            self.inner.remove(project, identity_id)
        }

        fn list_project(&self, project: &str) -> AccessStoreResult<Vec<(String, Role)>> {
            let active = self.active_lists.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_lists.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            let result = self.inner.list_project(project);
            self.active_lists.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    #[test]
    fn concurrent_owner_removals_cannot_leave_a_project_ownerless() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = Arc::new(ObjectDbProjectStore::new(db.clone()));
        projects
            .create_with_owner(
                ProjectMeta::new("acme", Visibility::Private, "alice", 1_000),
                "alice",
            )
            .unwrap();
        let roles = Arc::new(SlowRoleStore::new(db.clone()));
        roles.set("acme", "bob", Role::Owner).unwrap();
        let core = Arc::new(Core::with_stores(
            Arc::new(Jj::new(db)),
            CompilerRegistry::new(),
            Arc::new(NoopAuthn),
            Arc::new(NoopAuthz),
            RepoConfigStore::new(),
            roles.clone(),
            projects,
        ));
        let start = Arc::new(Barrier::new(2));

        // Act
        let results = std::thread::scope(|scope| {
            let first_core = core.clone();
            let first_start = start.clone();
            let first = scope.spawn(move || {
                first_start.wait();
                first_core.remove_member("acme", "alice", None)
            });
            let second_core = core.clone();
            let second_start = start;
            let second = scope.spawn(move || {
                second_start.wait();
                second_core.remove_member("acme", "bob", None)
            });
            [first.join().unwrap(), second.join().unwrap()]
        });

        // Assert
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            roles.list_project("acme").unwrap(),
            vec![(
                if results[0].is_ok() { "bob" } else { "alice" }.to_string(),
                Role::Owner,
            )]
        );
        assert_eq!(roles.max_active_lists.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn bounded_project_page_can_continue_after_a_page_of_hidden_projects() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = Arc::new(ObjectDbProjectStore::new(db.clone()));
        let roles = Arc::new(ObjectDbRoleStore::new(db.clone()));
        for index in 0..65 {
            let name = format!("hidden-{index:03}");
            projects
                .create_with_owner(
                    ProjectMeta::new(&name, Visibility::Private, "alice", 1_000),
                    "alice",
                )
                .unwrap();
        }
        projects
            .create_with_owner(
                ProjectMeta::new("zz-public", Visibility::Public, "alice", 1_000),
                "alice",
            )
            .unwrap();
        let core = Core::with_stores(
            Arc::new(Jj::new(db)),
            CompilerRegistry::new(),
            Arc::new(BearerTokenAuthn::new(HashMap::new())),
            Arc::new(RoleBasedAuthz::new(roles.clone(), projects.clone())),
            RepoConfigStore::new(),
            roles,
            projects,
        );

        // Act
        let hidden_page = core.list_projects_page(false, "", None, 1, None).unwrap();
        let visible_page = core
            .list_projects_page(false, "", hidden_page.next_cursor.as_deref(), 1, None)
            .unwrap();

        // Assert
        assert!(hidden_page.projects.is_empty());
        assert_eq!(hidden_page.next_cursor.as_deref(), Some("hidden-063"));
        assert_eq!(visible_page.projects[0].name, "zz-public");
        assert_eq!(visible_page.next_cursor, None);
    }
}
