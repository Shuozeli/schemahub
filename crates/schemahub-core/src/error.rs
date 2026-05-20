use schemahub_types::{CompatibilityViolation, MutationError};
use schemahub_storage::StorageError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("conflict: current HEAD is {current_head}, but provided base is {provided_base}")]
    Conflict {
        current_head: String,
        provided_base: String,
    },

    #[error("compatibility violation(s): {}", format_violations(.0))]
    CompatibilityViolation(Vec<CompatibilityViolation>),

    #[error("mutation error: {0}")]
    MutationError(#[from] MutationError),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    #[error("object corrupted: {0}")]
    ObjectCorrupted(String),
}

fn format_violations(violations: &[CompatibilityViolation]) -> String {
    violations
        .iter()
        .map(|v| v.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}
