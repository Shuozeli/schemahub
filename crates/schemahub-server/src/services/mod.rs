//! gRPC service handlers (crate-structure.md §3.6). Each handler holds an
//! `Arc<Core>` and maps its RPCs onto `Core` methods, converting types via
//! [`crate::wire`] and errors via [`crate::error::to_status`].

pub mod admin;
pub mod bookmark;
pub mod codegen;
pub mod exploration;
pub mod history;
pub mod project;
pub mod schema;

use tonic::{Request, Status};

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
