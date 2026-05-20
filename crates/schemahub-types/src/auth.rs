use crate::errors::{AuthnError, AuthzError};

/// An authenticated (or anonymous) caller identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    Anonymous,
    User(String),
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

/// A resource being acted upon.
#[derive(Clone, Debug)]
pub struct ResourcePath {
    pub project: String,
    pub repo: Option<String>,
}

impl ResourcePath {
    pub fn project(project: impl Into<String>) -> Self {
        Self { project: project.into(), repo: None }
    }

    pub fn repo(project: impl Into<String>, repo: impl Into<String>) -> Self {
        Self { project: project.into(), repo: Some(repo.into()) }
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
    fn check(&self, _caller: &Identity, _action: Action, _resource: &ResourcePath) -> Result<(), AuthzError> {
        Ok(())
    }
}
