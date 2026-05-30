//! Typed-op mutation envelope and its effect (design.md §2, §5).

use bytes::Bytes;

use crate::blob::{DeclBlob, MetaBlob};
use crate::schema_path::SchemaPath;

/// An opaque mutation envelope passed from the gRPC handler to the core, and
/// from the core to the `Compiler`. The core never inspects `operation` — it
/// only uses `format_id` to select the right compiler.
#[derive(Clone, Debug)]
pub struct Mutation {
    /// Which schema file to mutate.
    pub schema_path: SchemaPath,
    /// Which compiler handles this mutation: "protobuf" | "flatbuffers" | "openapi".
    pub format_id: String,
    /// Format-specific operation bytes. Deserialized only by the compiler.
    pub operation: Bytes,
}

/// What a mutation produced: changed/added/removed decls + possibly new meta.
#[derive(Clone, Debug, Default)]
pub struct MutationEffect {
    /// New file-level metadata, if the mutation changed it (e.g. added an import).
    pub meta: Option<MetaBlob>,
    /// Declarations to create or replace, by name.
    pub upserts: Vec<(String, DeclBlob)>,
    /// Declaration names to remove.
    pub removes: Vec<String>,
}
