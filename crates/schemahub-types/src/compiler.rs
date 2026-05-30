//! The `Compiler` trait — the boundary between the format-agnostic VCS layer
//! and the per-format compilers (design.md §2).
//!
//! The VCS layer operates entirely through this trait; it never calls
//! format-specific code directly. Every supported format (Protobuf,
//! FlatBuffers, OpenAPI) ships a `Compiler` implementation.

use bytes::Bytes;

use crate::blob::{DeclBlob, MetaBlob};
use crate::change::DeclChange;
use crate::compat::{CompatibilityRules, CompatibilityViolation};
use crate::conflict::ConflictSides;
use crate::decl::{DeclDetail, DeclSummary, TypeRef};
use crate::errors::{
    CodegenError, ConflictError, DescriptorError, DiffError, MutationError, ParseError, PrintError,
    ReadError,
};
use crate::import::Import;
use crate::language::Language;
use crate::mutation::{Mutation, MutationEffect};
use crate::parsed::{ParsedSchema, SchemaClosure, SchemaObjects};

/// The trait boundary between the VCS layer and the per-format compilers.
///
/// Object-safe: the VCS layer holds compilers as `Arc<dyn Compiler>` in a
/// registry keyed by [`format_id`](Compiler::format_id).
pub trait Compiler: Send + Sync + 'static {
    /// Unique format identifier: "protobuf" | "flatbuffers" | "openapi".
    fn format_id(&self) -> &'static str;

    // ── Ingest: source text → per-declaration objects ───────────────────────
    /// Parse source (reusing the sibling compiler), then split the resulting
    /// AST into one [`DeclBlob`] per top-level declaration plus one
    /// [`MetaBlob`] for the file.
    fn parse(&self, source: &str) -> Result<ParsedSchema, ParseError>;

    // ── Egress: per-declaration objects → canonical source (deterministic) ───
    /// Reassemble decls + meta into the compiler AST and print canonical source.
    fn print(&self, schema: &SchemaObjects) -> Result<String, PrintError>;

    // ── Diff ─────────────────────────────────────────────────────────────────
    fn diff_decl(&self, old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError>;

    // ── Granular mutation (validated against the AST) ────────────────────────
    /// Apply one typed op to one declaration (and possibly emit edits to
    /// others, e.g. a rename that touches referencing declarations).
    fn apply_mutation(
        &self,
        schema: &SchemaObjects,
        op: &Mutation,
    ) -> Result<MutationEffect, MutationError>;

    /// Transaction path: apply an ordered batch; only the final state is validated.
    fn apply_mutations(
        &self,
        schema: &SchemaObjects,
        ops: &[Mutation],
    ) -> Result<MutationEffect, MutationError>;

    // ── Compatibility (per changed declaration) ──────────────────────────────
    fn check_compatibility(
        &self,
        old: &DeclBlob,
        new: &DeclBlob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>>;

    // ── First-class conflicts (design.md §6) ─────────────────────────────────
    /// Render a conflicted declaration (a merge of N sides) for human/agent display.
    fn render_conflict(&self, sides: &ConflictSides) -> Result<String, ConflictError>;

    /// Validate a proposed resolution blob against the conflict.
    fn validate_resolution(&self, resolved: &DeclBlob) -> Result<(), ConflictError>;

    // ── Read / exploration ────────────────────────────────────────────────────
    fn summarize_decl(&self, blob: &DeclBlob) -> Result<DeclSummary, ReadError>;
    fn decl_detail(&self, blob: &DeclBlob) -> Result<DeclDetail, ReadError>;
    fn imports(&self, meta: &MetaBlob) -> Result<Vec<Import>, ReadError>;

    /// The type names a declaration references (for FollowType / rename propagation).
    fn type_refs(&self, blob: &DeclBlob) -> Result<Vec<TypeRef>, ReadError>;

    // ── Codegen (reuse sibling codegen) ───────────────────────────────────────
    /// Reassemble the transitive closure into the native descriptor artifact.
    fn generate_descriptors(&self, closure: &SchemaClosure) -> Result<Bytes, DescriptorError>;

    fn generate_code(
        &self,
        closure: &SchemaClosure,
        lang: Language,
    ) -> Result<String, CodegenError>;
}
