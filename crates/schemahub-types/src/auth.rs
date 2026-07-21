use crate::errors::{AuthnError, AuthzError};

/// The kind of principal represented by an authenticated identity.
///
/// Humans, agents, and services share the same authorization model; this value
/// exists for audit and workflow policy, not as a privilege level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    Anonymous,
    Human,
    Agent,
    Service,
}

/// An authenticated (or anonymous) caller identity.
///
/// `User` carries an opaque `id` (used as the role-store key and commit-author
/// fallback) and an optional human `display` name. The `id` is what auth
/// providers resolve a bearer token into; the `display` is for audit/UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    Anonymous,
    User {
        id: String,
        display: Option<String>,
    },
    Agent {
        id: String,
        display: Option<String>,
        delegated_by: Option<String>,
    },
    Service {
        id: String,
        display: Option<String>,
    },
}

impl Identity {
    /// Construct a User identity with no display name.
    pub fn user(id: impl Into<String>) -> Self {
        Identity::User {
            id: id.into(),
            display: None,
        }
    }

    /// Construct a User identity with a display name.
    pub fn user_with_display(id: impl Into<String>, display: impl Into<String>) -> Self {
        Identity::User {
            id: id.into(),
            display: Some(display.into()),
        }
    }

    /// Construct an agent identity. `delegated_by` identifies the human or
    /// service that authorized the agent run when that information is known.
    pub fn agent(
        id: impl Into<String>,
        display: Option<String>,
        delegated_by: Option<String>,
    ) -> Self {
        Identity::Agent {
            id: id.into(),
            display,
            delegated_by,
        }
    }

    /// Construct a non-human service identity.
    pub fn service(id: impl Into<String>, display: Option<String>) -> Self {
        Identity::Service {
            id: id.into(),
            display,
        }
    }

    /// Return the opaque identity id when authenticated, else `None`.
    pub fn id(&self) -> Option<&str> {
        match self {
            Identity::Anonymous => None,
            Identity::User { id, .. }
            | Identity::Agent { id, .. }
            | Identity::Service { id, .. } => Some(id.as_str()),
        }
    }

    /// Return the display name supplied by the authentication provider.
    pub fn display(&self) -> Option<&str> {
        match self {
            Identity::Anonymous => None,
            Identity::User { display, .. }
            | Identity::Agent { display, .. }
            | Identity::Service { display, .. } => display.as_deref(),
        }
    }

    /// Return the principal kind used in change-record audit metadata.
    pub fn kind(&self) -> IdentityKind {
        match self {
            Identity::Anonymous => IdentityKind::Anonymous,
            Identity::User { .. } => IdentityKind::Human,
            Identity::Agent { .. } => IdentityKind::Agent,
            Identity::Service { .. } => IdentityKind::Service,
        }
    }

    /// Return the delegating identity for an agent, if one was authenticated.
    pub fn delegated_by(&self) -> Option<&str> {
        match self {
            Identity::Agent { delegated_by, .. } => delegated_by.as_deref(),
            Identity::Anonymous | Identity::User { .. } | Identity::Service { .. } => None,
        }
    }

    /// True if this is the anonymous identity (no bearer token / unknown token).
    pub fn is_anonymous(&self) -> bool {
        matches!(self, Identity::Anonymous)
    }
}

/// The action being requested. Used by AuthzPolicy::check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    /// Required for --force mutations. Mapped to Maintainer+ role.
    Force,
    ManageProject,
    ManageRepo,
}

/// Project-scoped role. Higher values include strictly more permissions
/// (see [`Action`] → minimum-Role mapping in `RoleBasedAuthz`).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Role {
    Reader,
    Writer,
    Maintainer,
    Owner,
}

impl Role {
    /// Parse a case-insensitive role name (matches the CLI / config surface).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "reader" => Some(Role::Reader),
            "writer" => Some(Role::Writer),
            "maintainer" => Some(Role::Maintainer),
            "owner" => Some(Role::Owner),
            _ => None,
        }
    }
}

/// Project visibility (design.md §6): Public projects allow anonymous reads,
/// Private projects require an authenticated identity with a role on the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Visibility {
    Public,
    Private,
}

/// A resource being acted upon.
#[derive(Clone, Debug)]
pub struct ResourcePath {
    pub project: String,
    pub repo: Option<String>,
}

impl ResourcePath {
    pub fn project(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            repo: None,
        }
    }

    pub fn repo(project: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            repo: Some(repo.into()),
        }
    }
}

/// Extracts an Identity from request metadata (e.g. an Authorization header).
pub trait AuthnProvider: Send + Sync + 'static {
    fn identify(&self, token: Option<&str>) -> Result<Identity, AuthnError>;
}

/// Checks whether a caller Identity may perform an Action on a ResourcePath.
pub trait AuthzPolicy: Send + Sync + 'static {
    fn check(
        &self,
        caller: &Identity,
        action: Action,
        resource: &ResourcePath,
    ) -> Result<(), AuthzError>;
}

// ── No-op implementations for getting-started deployments ────────────────────

/// No-op AuthnProvider: treats every request as Identity::Anonymous.
pub struct NoopAuthn;

impl AuthnProvider for NoopAuthn {
    fn identify(&self, _token: Option<&str>) -> Result<Identity, AuthnError> {
        Ok(Identity::Anonymous)
    }
}

/// No-op AuthzPolicy: allows every action for every identity.
pub struct NoopAuthz;

impl AuthzPolicy for NoopAuthz {
    fn check(
        &self,
        _caller: &Identity,
        _action: Action,
        _resource: &ResourcePath,
    ) -> Result<(), AuthzError> {
        Ok(())
    }
}
