//! Core-level errors that orchestration flows surface to the server.

use schemahub_types::CompatibilityViolation;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("no compiler registered for format '{0}'")]
    UnknownFormat(String),
    #[error("could not detect a format for schema '{0}'")]
    UndetectableFormat(String),
    #[error("jj error: {0}")]
    Jj(#[from] schemahub_jj::JjError),
    #[error("authentication error: {0}")]
    Authn(#[from] schemahub_types::AuthnError),
    #[error("authorization error: {0}")]
    Authz(#[from] schemahub_types::AuthzError),
    #[error("mutation error: {0}")]
    Mutation(#[from] schemahub_types::MutationError),
    #[error("parse error: {0}")]
    Parse(#[from] schemahub_types::ParseError),
    #[error("print error: {0}")]
    Print(#[from] schemahub_types::PrintError),
    #[error("diff error: {0}")]
    Diff(#[from] schemahub_types::DiffError),
    #[error("read error: {0}")]
    Read(#[from] schemahub_types::ReadError),
    #[error("conflict error: {0}")]
    Conflict(#[from] schemahub_types::ConflictError),
    #[error("descriptor error: {0}")]
    Descriptor(#[from] schemahub_types::DescriptorError),
    #[error("codegen error: {0}")]
    Codegen(#[from] schemahub_types::CodegenError),
    /// One or more compatibility violations blocked a protected-bookmark write
    /// (design.md §7). Carries the per-declaration violations.
    #[error("compatibility violation: {} issue(s) on protected bookmark", .0.len())]
    Incompatible(Vec<CompatibilityViolation>),
    /// A transaction exceeded a configured limit (design.md §5.2).
    #[error("transaction limit exceeded: {0}")]
    LimitExceeded(String),
    /// An empty transaction (no ops) was submitted.
    #[error("transaction has no operations")]
    EmptyTransaction,
    /// A transaction mixed incompatible targets — ops spanning multiple
    /// `(project, repo)` or `format_id` values, which is not allowed (a
    /// transaction is repo- and format-scoped; design.md §5.2).
    #[error("invalid transaction batch: {0}")]
    MixedTransaction(String),
    #[error("{0}")]
    Other(String),
}

pub type CoreResult<T> = Result<T, CoreError>;
