//! Real `AuthnProvider` + `AuthzPolicy` implementations (design.md §6).
//!
//! - [`BearerTokenAuthn`] resolves an `Authorization: Bearer <token>` to a
//!   user [`Identity`] via a static token table. Unknown / missing tokens map
//!   to [`Identity::Anonymous`] (a valid identity for public-project reads);
//!   the authz layer is the one that decides whether the action is permitted.
//! - [`RoleBasedAuthz`] checks an action against the caller's project role
//!   (via [`RoleStore`]) and the project's visibility (via [`ProjectStore`]).

use std::collections::HashMap;
use std::sync::Arc;

use schemahub_types::{
    Action, AuthnError, AuthnProvider, AuthzError, AuthzPolicy, Identity, ResourcePath, Role,
    Visibility,
};

use crate::auth_store::{ProjectStore, RoleStore};

/// Bearer-token authn over a static token → Identity table.
///
/// The server passes `Authorization: Bearer <token>` through the
/// existing `token_from` extractor; the trimmed token is what
/// [`BearerTokenAuthn::identify`] receives. Unknown / missing tokens resolve
/// to [`Identity::Anonymous`] — never an error — so public-project reads
/// remain open. Errors are reserved for malformed credentials in future
/// (e.g. externally-validated JWTs), which this v1 impl does not support.
pub struct BearerTokenAuthn {
    tokens: HashMap<String, Identity>,
}

impl BearerTokenAuthn {
    /// Build from an explicit token → identity table. Used by the server's
    /// composition root after parsing `[auth].tokens` from `schemahub.toml`.
    pub fn new(tokens: HashMap<String, Identity>) -> Self {
        Self { tokens }
    }
}

impl AuthnProvider for BearerTokenAuthn {
    fn identify(&self, token: Option<&str>) -> Result<Identity, AuthnError> {
        let Some(t) = token else {
            return Ok(Identity::Anonymous);
        };
        if t.is_empty() {
            return Ok(Identity::Anonymous);
        }
        Ok(self
            .tokens
            .get(t)
            .cloned()
            .unwrap_or(Identity::Anonymous))
    }
}

/// Role-based authz over project visibility + the per-project role table
/// (design.md §6).
///
/// Decision rules:
/// 1. **Public-project rule:** `Action::Read` on a Public project is always
///    allowed, even for [`Identity::Anonymous`].
/// 2. **Writes always require an authenticated identity.** Anonymous +
///    non-read action ⇒ `PermissionDenied` regardless of visibility.
/// 3. **Role rule:** authenticated identities are allowed iff their role on
///    the project is ≥ the minimum role for the requested action:
///    `Read → Reader`, `Write → Writer`, `Force → Maintainer`,
///    `ManageRepo → Maintainer`, `ManageProject → Owner`.
/// 4. Projects with no [`ProjectMeta`](crate::auth_store::ProjectMeta) entry
///    are treated as **Private** (fail-closed): an unknown project is not a
///    public one.
pub struct RoleBasedAuthz {
    roles: Arc<dyn RoleStore>,
    projects: Arc<dyn ProjectStore>,
}

impl RoleBasedAuthz {
    pub fn new(roles: Arc<dyn RoleStore>, projects: Arc<dyn ProjectStore>) -> Self {
        Self { roles, projects }
    }

    /// Minimum role required to perform `action`.
    fn min_role(action: Action) -> Role {
        match action {
            Action::Read => Role::Reader,
            Action::Write => Role::Writer,
            Action::Force => Role::Maintainer,
            Action::ManageRepo => Role::Maintainer,
            Action::ManageProject => Role::Owner,
        }
    }
}

impl AuthzPolicy for RoleBasedAuthz {
    fn check(
        &self,
        caller: &Identity,
        action: Action,
        resource: &ResourcePath,
    ) -> Result<(), AuthzError> {
        // Rule 1: anonymous read on a public project is always allowed.
        let visibility = self
            .projects
            .get(&resource.project)
            .map(|m| m.visibility)
            .unwrap_or(Visibility::Private);

        if action == Action::Read && visibility == Visibility::Public {
            return Ok(());
        }

        // Rule 2: any non-read action requires an authenticated identity.
        if caller.is_anonymous() {
            return Err(AuthzError::PermissionDenied(format!(
                "anonymous access denied for {action:?} on project '{}'",
                resource.project
            )));
        }

        // Rule 3: caller must have a role ≥ the minimum.
        let needed = Self::min_role(action);
        match self.roles.get(&resource.project, caller) {
            Some(role) if role >= needed => Ok(()),
            Some(role) => Err(AuthzError::PermissionDenied(format!(
                "role {role:?} cannot {action:?} on project '{}' (need {needed:?}+)",
                resource.project
            ))),
            None => Err(AuthzError::PermissionDenied(format!(
                "caller has no role on project '{}'",
                resource.project
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_store::ProjectMeta;
    use std::sync::Mutex;

    // ── Test doubles ────────────────────────────────────────────────────────

    #[derive(Default)]
    struct MemRoleStore {
        roles: Mutex<HashMap<(String, String), Role>>, // (project, identity_id) → role
    }

    impl RoleStore for MemRoleStore {
        fn get(&self, project: &str, identity: &Identity) -> Option<Role> {
            let id = identity.id()?;
            self.roles
                .lock()
                .unwrap()
                .get(&(project.to_string(), id.to_string()))
                .copied()
        }
        fn set(&self, project: &str, identity_id: &str, role: Role) -> std::io::Result<()> {
            self.roles
                .lock()
                .unwrap()
                .insert((project.to_string(), identity_id.to_string()), role);
            Ok(())
        }
        fn remove(&self, project: &str, identity_id: &str) -> std::io::Result<()> {
            self.roles
                .lock()
                .unwrap()
                .remove(&(project.to_string(), identity_id.to_string()));
            Ok(())
        }
        fn list_project(&self, project: &str) -> Vec<(String, Role)> {
            self.roles
                .lock()
                .unwrap()
                .iter()
                .filter(|((p, _), _)| p == project)
                .map(|((_, id), r)| (id.clone(), *r))
                .collect()
        }
    }

    #[derive(Default)]
    struct MemProjectStore {
        projects: Mutex<HashMap<String, ProjectMeta>>,
    }

    impl ProjectStore for MemProjectStore {
        fn get(&self, project: &str) -> Option<ProjectMeta> {
            self.projects.lock().unwrap().get(project).cloned()
        }
        fn set(&self, meta: ProjectMeta) -> std::io::Result<()> {
            self.projects.lock().unwrap().insert(meta.name.clone(), meta);
            Ok(())
        }
        fn list(&self) -> Vec<ProjectMeta> {
            self.projects.lock().unwrap().values().cloned().collect()
        }
    }

    fn fixture(visibility: Visibility) -> (RoleBasedAuthz, Arc<MemRoleStore>, Arc<MemProjectStore>) {
        let roles: Arc<MemRoleStore> = Arc::new(MemRoleStore::default());
        let projects: Arc<MemProjectStore> = Arc::new(MemProjectStore::default());
        projects
            .set(ProjectMeta {
                name: "acme".into(),
                visibility,
                creator: "alice".into(),
            })
            .unwrap();
        let authz = RoleBasedAuthz::new(roles.clone(), projects.clone());
        (authz, roles, projects)
    }

    fn res() -> ResourcePath {
        ResourcePath::project("acme")
    }

    #[test]
    fn public_project_allows_anonymous_read() {
        // Arrange
        let (authz, _r, _p) = fixture(Visibility::Public);

        // Act
        let result = authz.check(&Identity::Anonymous, Action::Read, &res());

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn public_project_denies_anonymous_write() {
        // Arrange
        let (authz, _r, _p) = fixture(Visibility::Public);

        // Act
        let result = authz.check(&Identity::Anonymous, Action::Write, &res());

        // Assert
        assert!(matches!(result, Err(AuthzError::PermissionDenied(_))));
    }

    #[test]
    fn private_project_denies_anonymous_read() {
        // Arrange
        let (authz, _r, _p) = fixture(Visibility::Private);

        // Act
        let result = authz.check(&Identity::Anonymous, Action::Read, &res());

        // Assert
        assert!(matches!(result, Err(AuthzError::PermissionDenied(_))));
    }

    #[test]
    fn reader_role_allows_read_denies_write() {
        // Arrange
        let (authz, roles, _p) = fixture(Visibility::Private);
        roles.set("acme", "bob", Role::Reader).unwrap();
        let bob = Identity::user("bob");

        // Act / Assert
        assert!(authz.check(&bob, Action::Read, &res()).is_ok());
        assert!(authz.check(&bob, Action::Write, &res()).is_err());
    }

    #[test]
    fn writer_role_denies_force() {
        // Arrange
        let (authz, roles, _p) = fixture(Visibility::Private);
        roles.set("acme", "bob", Role::Writer).unwrap();
        let bob = Identity::user("bob");

        // Act / Assert
        assert!(authz.check(&bob, Action::Write, &res()).is_ok());
        assert!(authz.check(&bob, Action::Force, &res()).is_err());
    }

    #[test]
    fn maintainer_role_allows_force_denies_manage_project() {
        // Arrange
        let (authz, roles, _p) = fixture(Visibility::Private);
        roles.set("acme", "bob", Role::Maintainer).unwrap();
        let bob = Identity::user("bob");

        // Act / Assert
        assert!(authz.check(&bob, Action::Force, &res()).is_ok());
        assert!(authz.check(&bob, Action::ManageRepo, &res()).is_ok());
        assert!(authz.check(&bob, Action::ManageProject, &res()).is_err());
    }

    #[test]
    fn owner_role_allows_manage_project() {
        // Arrange
        let (authz, roles, _p) = fixture(Visibility::Private);
        roles.set("acme", "alice", Role::Owner).unwrap();
        let alice = Identity::user("alice");

        // Act
        let result = authz.check(&alice, Action::ManageProject, &res());

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn unknown_project_is_private_by_default() {
        // Arrange
        let roles: Arc<MemRoleStore> = Arc::new(MemRoleStore::default());
        let projects: Arc<MemProjectStore> = Arc::new(MemProjectStore::default());
        let authz = RoleBasedAuthz::new(roles, projects);

        // Act
        let result = authz.check(&Identity::Anonymous, Action::Read, &ResourcePath::project("ghost"));

        // Assert: anonymous read denied because the project is treated as private.
        assert!(matches!(result, Err(AuthzError::PermissionDenied(_))));
    }

    #[test]
    fn bearer_token_resolves_known_token() {
        // Arrange
        let mut tokens = HashMap::new();
        tokens.insert("t1".to_string(), Identity::user("alice"));
        let authn = BearerTokenAuthn::new(tokens);

        // Act
        let identity = authn.identify(Some("t1")).unwrap();

        // Assert
        assert_eq!(identity.id(), Some("alice"));
    }

    #[test]
    fn bearer_token_unknown_is_anonymous() {
        // Arrange
        let authn = BearerTokenAuthn::new(HashMap::new());

        // Act
        let identity = authn.identify(Some("nope")).unwrap();

        // Assert
        assert!(identity.is_anonymous());
    }

    #[test]
    fn bearer_token_missing_is_anonymous() {
        // Arrange
        let authn = BearerTokenAuthn::new(HashMap::new());

        // Act
        let identity = authn.identify(None).unwrap();

        // Assert
        assert!(identity.is_anonymous());
    }
}
