use std::collections::HashMap;

use bytes::Bytes;

use crate::{
    Blob, CompatibilityRules, CompatibilityViolation, DeclDetail, DeclSummary, DescriptorError,
    DiffError, Import, Language, Mutation, MutationError, ParseError, PrintError, ReadError,
    SchemaChange, SchemaPath,
    errors::CodegenError,
};

/// The trait boundary between the core layer and the format-specific layer.
/// Every supported schema format (Protobuf, FlatBuffers, OpenAPI) implements this.
///
/// The core layer operates entirely through this trait — it never calls
/// format-specific code directly.
pub trait FormatPlugin: Send + Sync + 'static {
    /// Unique identifier for this format, e.g. "protobuf", "flatbuffers", "openapi".
    /// Used by the core to select the right plugin for a given schema file.
    fn format_id(&self) -> &'static str;

    /// Parse source text into a serialized AST blob.
    /// For OpenAPI: returns an envelope blob containing all declarations.
    /// For Protobuf/FlatBuffers: returns one blob per schema file.
    fn parse(&self, source: &str) -> Result<Blob, ParseError>;

    /// Reconstruct canonical source text from an AST blob.
    /// Must be deterministic: parse(print(blob)) produces an identical blob.
    fn print(&self, blob: &Blob) -> Result<String, PrintError>;

    /// Compute the semantic diff between two AST blobs.
    /// Returns one SchemaChange per top-level declaration that changed.
    fn diff(&self, old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError>;

    /// Validate and apply a single mutation against the current AST blob.
    /// Called by the single-mutation flow. Returns the new blob if valid.
    fn apply_mutation(
        &self,
        blob: &Blob,
        mutation: &Mutation,
    ) -> Result<Blob, MutationError>;

    /// Apply a sequence of mutations across one or more schemas.
    /// Called by the transaction flow. Intermediate states are NOT validated —
    /// only the final state of each blob is returned for compatibility checking.
    ///
    /// Input: all schemas that may be touched, keyed by schema path.
    /// Output: only the schemas that were actually changed (unchanged schemas omitted).
    fn apply_mutations(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        mutations: &[Mutation],
    ) -> Result<HashMap<SchemaPath, Blob>, MutationError>;

    /// Check whether the change from `old` to `new` satisfies the given
    /// compatibility rules. Called once per changed schema after apply_mutations.
    fn check_compatibility(
        &self,
        old: &Blob,
        new: &Blob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>>;

    // ── Read / exploration ────────────────────────────────────────────────────

    /// List all top-level declarations in the blob with summary info.
    /// Used by the ListDeclarations RPC.
    fn list_declarations(&self, blob: &Blob) -> Result<Vec<DeclSummary>, ReadError>;

    /// Return full detail for one named declaration.
    /// The returned bytes are opaque to the core and forwarded to the client.
    fn get_declaration(&self, blob: &Blob, name: &str) -> Result<DeclDetail, ReadError>;

    /// Extract all import references from the blob.
    /// Used by the core for BFS transitive closure during codegen.
    fn imports(&self, blob: &Blob) -> Result<Vec<Import>, ReadError>;

    // ── Codegen ───────────────────────────────────────────────────────────────

    /// Produce a self-contained descriptor artifact from a transitive closure of blobs.
    ///   Protobuf    → serialized FileDescriptorSet
    ///   FlatBuffers → tar archive of reconstructed .fbs files
    ///   OpenAPI     → resolved YAML with all $ref inlined
    ///
    /// The core pre-computes the transitive closure via BFS and passes it here.
    fn generate_descriptors(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
    ) -> Result<Bytes, DescriptorError>;

    /// Render generated code for a given language.
    /// Returns CodegenError::UnsupportedLanguage for unsupported combinations.
    fn generate_code(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        language: Language,
    ) -> Result<String, CodegenError>;
}
