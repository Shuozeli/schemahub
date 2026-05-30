//! AuthN/AuthZ invocation used by the mutation/read flows (design.md §11).
//!
//! The core holds an `Arc<dyn AuthnProvider>` + `Arc<dyn AuthzPolicy>`; the
//! default Noop implementations (from `schemahub-types`) allow everything. Auth
//! runs after the idempotency check, before the transaction (steps 2–3).

use schemahub_types::{Action, AuthnProvider, AuthzPolicy, Identity, ResourcePath};

use crate::error::CoreResult;
use crate::Core;

/// Identify the caller, then authorize `action` on `project/repo`. Returns the
/// resolved [`Identity`] (used as the commit author fallback / audit record).
pub(crate) fn authorize(
    authn: &dyn AuthnProvider,
    authz: &dyn AuthzPolicy,
    token: Option<&str>,
    action: Action,
    project: &str,
    repo: &str,
) -> CoreResult<Identity> {
    let identity = authn.identify(token)?;
    let resource = ResourcePath::repo(project, repo);
    authz.check(&identity, action, &resource)?;
    Ok(identity)
}

impl Core {
    /// Run auth on `action` against `(project, repo)`. Returns the resolved
    /// [`Identity`]. Used by the schema-lifecycle (create/update/delete) and
    /// other server handlers that go through the Core but bypass the
    /// mutation/transaction flow.
    pub fn authorize_repo_action(
        &self,
        token: Option<&str>,
        action: Action,
        project: &str,
        repo: &str,
    ) -> CoreResult<Identity> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            action,
            project,
            repo,
        )
    }
}
