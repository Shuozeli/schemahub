//! gRPC service handlers (crate-structure.md §3.6). Each handler holds an
//! `Arc<Core>` and maps its RPCs onto `Core` methods, converting types via
//! [`crate::wire`] and errors via [`crate::error::to_status`].

pub mod admin;
pub mod bookmark;
pub mod change;
pub mod codegen;
pub mod exploration;
pub mod history;
pub mod project;
pub mod schema;
pub mod serving;

use schemahub_core::Core;
use schemahub_jj::RefSpec;
use schemahub_types::Action;
use tonic::{Request, Status};

use schemahub_api::schemahub_v1 as pb;

use crate::error::to_status;
use crate::wire;

/// The audit author recorded when the caller is anonymous (no token).
pub(crate) const DEFAULT_AUTHOR: &str = "schemahub";

/// Resolve the commit / op-log author from the bearer token.
///
/// Returns the authenticated identity's id (e.g. \"alice\") when present;
/// falls back to [`DEFAULT_AUTHOR`] for anonymous callers (public-project
/// reads, and writes that the authz layer has already rejected before we
/// reach the commit path).
///
/// This is the single source of truth for \"who's committing\" — handlers
/// must not accept a client-supplied author string and pass it to the JJ layer,
/// since that would let any authenticated caller forge an arbitrary audit
/// trail.
pub(crate) fn resolve_author(core: &Core, token: Option<&str>) -> Result<String, Status> {
    let identity = core.resolve_identity(token).map_err(to_status)?;
    Ok(identity.id().unwrap_or(DEFAULT_AUTHOR).to_string())
}

/// Preserve an explicit branch/tag/commit, or select the repository's
/// configured default bookmark with the same authorization as the operation.
pub(crate) fn refspec_or_repository_default(
    core: &Core,
    project: &str,
    repo: &str,
    at: &Option<pb::VersionRef>,
    action: Action,
    token: Option<&str>,
) -> Result<RefSpec, Status> {
    if let Some(at) = wire::version_ref_to_optional_refspec(at) {
        return Ok(at);
    }
    let bookmark = core
        .repository_default_bookmark(project, repo, action, token)
        .map_err(to_status)?;
    Ok(RefSpec::bookmark(bookmark))
}

/// Extract a bearer/auth token from request metadata (`authorization` header).
///
/// Returns `Ok(None)` when the header is absent (an anonymous request — valid
/// for public-project reads). Returns `Err(Status::unauthenticated)` when the
/// header is present but cannot be decoded as ASCII metadata: a malformed
/// header is a client bug or attempted bypass, not a missing one, and
/// silently falling through to anonymous would risk granting unintended
/// public-read access.
pub(crate) fn token_from<T>(req: &Request<T>) -> Result<Option<String>, Status> {
    let Some(raw) = req.metadata().get("authorization") else {
        return Ok(None);
    };
    let s = raw
        .to_str()
        .map_err(|_| Status::unauthenticated("authorization header is not valid ASCII"))?;
    let trimmed = s.trim_start_matches("Bearer ").trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}
