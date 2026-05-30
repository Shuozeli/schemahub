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

use tonic::Request;

/// Extract a bearer/auth token from request metadata (`authorization` header).
/// Returns `None` when absent. Passed to the core's `AuthnProvider`.
pub(crate) fn token_from<T>(req: &Request<T>) -> Option<String> {
    req.metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").trim().to_string())
        .filter(|s| !s.is_empty())
}
