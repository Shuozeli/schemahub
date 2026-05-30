use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("syntax error at line {line}: {message}")]
    SyntaxError { line: usize, message: String },
    #[error("unsupported schema version: {0}")]
    UnsupportedVersion(String),
    #[error("parse error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum PrintError {
    #[error("blob is malformed or has an unsupported blob_version: {0}")]
    MalformedBlob(String),
    #[error("print error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum DiffError {
    #[error("blob is malformed: {0}")]
    MalformedBlob(String),
}

#[derive(Debug, Error)]
pub enum MutationError {
    #[error("blob is malformed: {0}")]
    MalformedBlob(String),
    #[error("declaration '{0}' not found")]
    DeclarationNotFound(String),
    #[error("field '{field}' not found in '{declaration}'")]
    FieldNotFound { declaration: String, field: String },
    #[error("field number {0} is already in use or reserved")]
    FieldNumberConflict(u32),
    #[error("field number {0} is reserved and cannot be reused")]
    FieldNumberReserved(u32),
    #[error("{0}")]
    InvalidOperation(String),
    #[error("this mutation is not supported in v1")]
    UnsupportedInV1,
    #[error("mutation operation bytes could not be decoded: {0}")]
    InvalidOperationBytes(String),
}

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("blob is malformed: {0}")]
    MalformedBlob(String),
    #[error("declaration '{0}' not found in blob")]
    NotFound(String),
}

#[derive(Debug, Error)]
pub enum ConflictError {
    #[error("blob is malformed: {0}")]
    MalformedBlob(String),
    #[error("proposed resolution is not a valid declaration: {0}")]
    InvalidResolution(String),
    #[error("conflict has no sides to render")]
    EmptyConflict,
    #[error("conflict error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum DescriptorError {
    #[error("blob is malformed: {0}")]
    MalformedBlob(String),
    #[error("descriptor generation error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("language {0:?} is not supported by this plugin")]
    UnsupportedLanguage(crate::Language),
    #[error("blob is malformed: {0}")]
    MalformedBlob(String),
    #[error("codegen error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum AuthnError {
    #[error("missing authentication credentials")]
    MissingCredentials,
    #[error("invalid or expired token")]
    InvalidToken,
    #[error("authentication error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum AuthzError {
    #[error("permission denied: {0}")]
    PermissionDenied(String),
}
