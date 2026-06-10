//! Core errors → `tonic::Status` (crate-structure.md §3.6).
//!
//! Maps each [`CoreError`] variant to the gRPC status code that best matches
//! the failure semantics (design.md §5/§6/§7). Compatibility violations and
//! conflict-related preconditions map to `FAILED_PRECONDITION`; auth failures to
//! `UNAUTHENTICATED` / `PERMISSION_DENIED`; bad input to `INVALID_ARGUMENT`.

use schemahub_core::CoreError;
use schemahub_jj::JjError;
use schemahub_types::{AuthnError, AuthzError, MutationError};
use tonic::Status;

/// Convert a [`CoreError`] into a `tonic::Status`.
pub fn to_status(err: CoreError) -> Status {
    match err {
        CoreError::UnknownFormat(f) => {
            Status::invalid_argument(format!("no compiler registered for format '{f}'"))
        }
        CoreError::UndetectableFormat(s) => {
            Status::invalid_argument(format!("could not detect a format for schema '{s}'"))
        }
        CoreError::Jj(e) => jj_to_status(e),
        CoreError::Authn(e) => authn_to_status(e),
        CoreError::Authz(AuthzError::PermissionDenied(m)) => Status::permission_denied(m),
        CoreError::Mutation(e) => mutation_to_status(e),
        CoreError::Parse(e) => Status::invalid_argument(format!("parse error: {e}")),
        CoreError::Print(e) => Status::internal(format!("print error: {e}")),
        CoreError::Diff(e) => Status::internal(format!("diff error: {e}")),
        CoreError::Read(e) => Status::not_found(format!("read error: {e}")),
        CoreError::Conflict(e) => Status::failed_precondition(format!("conflict error: {e}")),
        CoreError::Descriptor(e) => Status::internal(format!("descriptor error: {e}")),
        CoreError::Codegen(e) => Status::unimplemented(format!("codegen error: {e}")),
        CoreError::Incompatible(violations) => Status::failed_precondition(format!(
            "compatibility violation: {} issue(s) on a protected bookmark",
            violations.len()
        )),
        CoreError::LimitExceeded(m) => {
            Status::invalid_argument(format!("transaction limit exceeded: {m}"))
        }
        CoreError::EmptyTransaction => Status::invalid_argument("transaction has no operations"),
        CoreError::MixedTransaction(m) => {
            Status::invalid_argument(format!("invalid transaction batch: {m}"))
        }
        CoreError::Other(m) => Status::internal(m),
    }
}

fn jj_to_status(err: JjError) -> Status {
    match err {
        JjError::ObjectNotFound
        | JjError::DeclNotFound(_)
        | JjError::SchemaNotFound(_)
        | JjError::BookmarkNotFound(_)
        | JjError::TagNotFound(_) => Status::not_found(err.to_string()),
        JjError::BookmarkExists(_) => Status::already_exists(err.to_string()),
        JjError::NothingToUndo | JjError::NotConflicted { .. } | JjError::BadRef(_) => {
            Status::failed_precondition(err.to_string())
        }
        JjError::ObjectDb(_) | JjError::Corrupt(_) | JjError::Other(_) => {
            Status::internal(err.to_string())
        }
    }
}

fn authn_to_status(err: AuthnError) -> Status {
    match err {
        AuthnError::MissingCredentials | AuthnError::InvalidToken => {
            Status::unauthenticated(err.to_string())
        }
        AuthnError::Other(m) => Status::unauthenticated(m),
    }
}

fn mutation_to_status(err: MutationError) -> Status {
    match err {
        MutationError::DeclarationNotFound(_) | MutationError::FieldNotFound { .. } => {
            Status::not_found(err.to_string())
        }
        MutationError::UnsupportedInV1 => Status::unimplemented(err.to_string()),
        _ => Status::invalid_argument(err.to_string()),
    }
}
