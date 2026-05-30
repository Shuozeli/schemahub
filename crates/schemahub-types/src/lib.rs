//! `schemahub-types` — the shared types and trait boundary between the
//! format-agnostic VCS layer and the per-format compilers (design.md §2,
//! crate-structure.md §3.1).
//!
//! Depends on `bytes` + `std` only.

pub mod auth;
pub mod blob;
pub mod change;
pub mod compat;
pub mod compiler;
pub mod conflict;
pub mod decl;
pub mod errors;
pub mod import;
pub mod language;
pub mod mutation;
pub mod parsed;
pub mod schema_path;

pub use auth::{Action, AuthnProvider, AuthzPolicy, Identity, NoopAuthn, NoopAuthz, ResourcePath};
pub use blob::{DeclBlob, MetaBlob};
pub use change::DeclChange;
pub use compat::{CompatibilityDirection, CompatibilityRules, CompatibilityViolation};
pub use compiler::Compiler;
pub use conflict::ConflictSides;
pub use decl::{DeclDetail, DeclKind, DeclSummary, TypeRef};
pub use errors::{
    AuthnError, AuthzError, CodegenError, ConflictError, DescriptorError, DiffError, MutationError,
    ParseError, PrintError, ReadError,
};
pub use import::Import;
pub use language::Language;
pub use mutation::{Mutation, MutationEffect};
pub use parsed::{ParsedSchema, SchemaClosure, SchemaObjects};
pub use schema_path::SchemaPath;
