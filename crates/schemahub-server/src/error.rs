//! Core errors → `tonic::Status` (crate-structure.md §3.6).
//!
//! Maps each [`CoreError`] variant to the gRPC status code that best matches
//! the failure semantics (design.md §5/§6/§7). Compatibility violations and
//! conflict-related preconditions map to `FAILED_PRECONDITION`; auth failures to
//! `UNAUTHENTICATED` / `PERMISSION_DENIED`; bad input to `INVALID_ARGUMENT`.

use schemahub_core::change_record::{ChangeLedgerError, ChangeStoreError};
use schemahub_core::{
    AccessStoreError, CoreError, IdempotencyError, RepositoryError, RepositoryStoreError,
};
use schemahub_jj::JjError;
use schemahub_types::{AuthnError, AuthzError, MutationError, ReadError};
use tonic::Status;

/// Convert a [`CoreError`] into a `tonic::Status`.
pub fn to_status(err: CoreError) -> Status {
    match err {
        CoreError::InvalidArgument(message) => Status::invalid_argument(message),
        CoreError::AlreadyExists(message) => Status::already_exists(message),
        CoreError::FailedPrecondition(message) => Status::failed_precondition(message),
        CoreError::ResourceExhausted(message) => Status::resource_exhausted(message),
        CoreError::UnknownFormat(f) => {
            Status::invalid_argument(format!("no compiler registered for format '{f}'"))
        }
        CoreError::UndetectableFormat(s) => {
            Status::invalid_argument(format!("could not detect a format for schema '{s}'"))
        }
        CoreError::Jj(e) => jj_to_status(e),
        CoreError::Authn(e) => authn_to_status(e),
        CoreError::Authz(AuthzError::PermissionDenied(m)) => Status::permission_denied(m),
        CoreError::Authz(AuthzError::Backend(m)) => Status::internal(m),
        CoreError::Mutation(e) => mutation_to_status(e),
        CoreError::Parse(e) => Status::invalid_argument(format!("parse error: {e}")),
        CoreError::Print(e) => Status::internal(format!("print error: {e}")),
        CoreError::Diff(e) => Status::internal(format!("diff error: {e}")),
        CoreError::Read(e) => read_to_status(e),
        CoreError::Conflict(e) => Status::failed_precondition(format!("conflict error: {e}")),
        CoreError::Descriptor(e) => Status::internal(format!("descriptor error: {e}")),
        CoreError::Codegen(e) => Status::unimplemented(format!("codegen error: {e}")),
        CoreError::ChangeLedger(e) => change_ledger_to_status(e),
        CoreError::Repository(e) => repository_to_status(e),
        CoreError::AccessStore(AccessStoreError::AlreadyExists(name)) => {
            Status::already_exists(name)
        }
        CoreError::AccessStore(AccessStoreError::NotFound(name)) => Status::not_found(name),
        CoreError::AccessStore(AccessStoreError::EtagMismatch {
            name,
            expected,
            current,
        }) => Status::aborted(format!(
            "project etag mismatch for {name}: expected {expected}, current {current}"
        )),
        CoreError::AccessStore(AccessStoreError::Backend(message)) => Status::internal(message),
        CoreError::Idempotency(IdempotencyError::InvalidArgument(message)) => {
            Status::invalid_argument(message)
        }
        CoreError::Idempotency(
            IdempotencyError::InProgress(message) | IdempotencyError::KeyReuse(message),
        ) => Status::failed_precondition(message),
        CoreError::Idempotency(IdempotencyError::Capacity) => {
            Status::failed_precondition("idempotency capacity is occupied by in-progress requests")
        }
        CoreError::Idempotency(IdempotencyError::Backend(message)) => Status::internal(message),
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
        CoreError::TransactionDeadlineExceeded => {
            Status::deadline_exceeded("transaction execution deadline exceeded")
        }
        CoreError::Other(m) => Status::internal(m),
    }
}

fn read_to_status(err: ReadError) -> Status {
    match err {
        ReadError::NotFound(_) | ReadError::FieldNotFound(_) => {
            Status::not_found(format!("read error: {err}"))
        }
        ReadError::NotATypeReference(_) => Status::invalid_argument(format!("read error: {err}")),
        ReadError::AmbiguousTypeReference(_) => {
            Status::failed_precondition(format!("read error: {err}"))
        }
        ReadError::MalformedBlob(_) => Status::internal(format!("read error: {err}")),
    }
}

fn repository_to_status(err: RepositoryError) -> Status {
    match err {
        RepositoryError::InvalidArgument(message) => Status::invalid_argument(message),
        RepositoryError::FailedPrecondition(message) => Status::failed_precondition(message),
        RepositoryError::Store(RepositoryStoreError::AlreadyExists(name)) => {
            Status::already_exists(format!("repository already exists: {name}"))
        }
        RepositoryError::Store(RepositoryStoreError::NotFound(name)) => {
            Status::not_found(format!("repository not found: {name}"))
        }
        RepositoryError::Store(RepositoryStoreError::EtagMismatch {
            name,
            expected,
            current,
        }) => Status::aborted(format!(
            "repository etag mismatch for {name}: expected {expected}, current {current}"
        )),
        RepositoryError::Store(RepositoryStoreError::Backend(message)) => Status::internal(message),
    }
}

fn change_ledger_to_status(err: ChangeLedgerError) -> Status {
    match err {
        ChangeLedgerError::InvalidArgument(message) => Status::invalid_argument(message),
        ChangeLedgerError::FailedPrecondition(message) => Status::failed_precondition(message),
        ChangeLedgerError::Store(ChangeStoreError::AlreadyExists(name)) => {
            Status::already_exists(format!("change record already exists: {name}"))
        }
        ChangeLedgerError::Store(ChangeStoreError::NotFound(name)) => {
            Status::not_found(format!("change record not found: {name}"))
        }
        ChangeLedgerError::Store(ChangeStoreError::EtagMismatch {
            name,
            expected,
            current,
        }) => Status::aborted(format!(
            "change record etag mismatch for {name}: expected {expected}, current {current}"
        )),
        ChangeLedgerError::Store(ChangeStoreError::Backend(message)) => Status::internal(message),
        ChangeLedgerError::Runtime(error) => Status::internal(error.to_string()),
    }
}

fn jj_to_status(err: JjError) -> Status {
    match err {
        JjError::ObjectNotFound
        | JjError::DeclNotFound(_)
        | JjError::SchemaNotFound(_)
        | JjError::BookmarkNotFound(_)
        | JjError::TagNotFound(_) => Status::not_found(err.to_string()),
        JjError::BookmarkExists(_) | JjError::TagExists(_) => {
            Status::already_exists(err.to_string())
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_exhaustion_preserves_the_public_grpc_status() {
        // Arrange
        let error = CoreError::ResourceExhausted("dependency scan bound exceeded".to_string());

        // Act
        let status = to_status(error);

        // Assert
        assert_eq!(status.code(), tonic::Code::ResourceExhausted);
        assert_eq!(status.message(), "dependency scan bound exceeded");
    }
}
