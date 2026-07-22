//! `schemahub-compiler-flatbuffers` — the FlatBuffers compiler, wrapping
//! `flatbuffers-rs` (`flatc-rs-parser`, `flatc-rs-schema`, `flatc-rs-codegen`)
//! and owning the `.fbs` printer (design.md §3.2, crate-structure.md §3.5).
//!
//! Status: P0–P4 implemented. The only deliberate limitation is full codegen
//! *layout* resolution (struct `byte_size`/`min_align`), which is performed by
//! the `flatc-rs-compiler` analyzer (not a dependency here); see `codegen.rs`.

mod blob;
mod codegen;
mod compat;
mod conflict;
mod diff;
mod mutations;
mod parse;
mod printer;
mod read;

#[cfg(test)]
mod tests;

pub use blob::{DeclPayload, FbsMeta};
pub use mutations::FbsOp;

use bytes::Bytes;

use schemahub_types::{
    CodegenError, CompatibilityRules, CompatibilityViolation, Compiler, ConflictError,
    ConflictSides, DeclBlob, DeclChange, DeclDetail, DeclSummary, DescriptorError, DiffError,
    Import, Language, Mutation, MutationEffect, MutationError, ParseError, ParsedSchema,
    PrintError, ReadError, SchemaClosure, SchemaObjects, TypeRef,
};

use crate::blob::{decode_decl, decode_meta};
use crate::printer::{print_decl, print_meta_header, print_root_type};

/// The FlatBuffers compiler front-end.
#[derive(Debug, Default, Clone)]
pub struct FlatBuffersCompiler;

impl FlatBuffersCompiler {
    pub fn new() -> Self {
        Self
    }
}

impl Compiler for FlatBuffersCompiler {
    fn format_id(&self) -> &'static str {
        "flatbuffers"
    }

    fn parse(&self, source: &str) -> Result<ParsedSchema, ParseError> {
        parse::parse(source)
    }

    fn print(&self, schema: &SchemaObjects) -> Result<String, PrintError> {
        // Reassemble metadata header, then declarations in original SOURCE order
        // (from `meta.decl_order`), then the trailing root_type. `schema.decls`
        // is a `BTreeMap` (alphabetical); without the recorded order, `.fbs`
        // declarations round-trip sorted by name instead of as written.
        let meta =
            decode_meta(&schema.meta).map_err(|e| PrintError::MalformedBlob(e.to_string()))?;
        let mut out = print_meta_header(&meta);

        // Names in recorded source order first; any remaining (e.g. added by a
        // later mutation, or old blobs with no recorded order) follow in
        // BTreeMap order. This reduces to plain BTreeMap order when empty.
        let mut printed = std::collections::HashSet::new();
        let mut print_one = |name: &str, out: &mut String| -> Result<(), PrintError> {
            if let Some(blob) = schema.decls.get(name) {
                if printed.insert(name.to_string()) {
                    let payload =
                        decode_decl(blob).map_err(|e| PrintError::MalformedBlob(e.to_string()))?;
                    out.push_str(&print_decl(&payload));
                    out.push('\n');
                }
            }
            Ok(())
        };
        for name in &meta.decl_order {
            print_one(name, &mut out)?;
        }
        for name in schema.decls.keys() {
            print_one(name, &mut out)?;
        }

        out.push_str(&print_root_type(&meta));
        Ok(out)
    }

    fn diff_decl(&self, old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError> {
        diff::diff_decl(old, new)
    }

    fn apply_mutation(
        &self,
        schema: &SchemaObjects,
        op: &Mutation,
    ) -> Result<MutationEffect, MutationError> {
        mutations::apply_mutation(schema, op)
    }

    fn apply_mutations(
        &self,
        schema: &SchemaObjects,
        ops: &[Mutation],
    ) -> Result<MutationEffect, MutationError> {
        mutations::apply_mutations(schema, ops)
    }

    fn check_compatibility(
        &self,
        old: &DeclBlob,
        new: &DeclBlob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>> {
        compat::check_compatibility(old, new, rules)
    }

    fn render_conflict(&self, sides: &ConflictSides) -> Result<String, ConflictError> {
        conflict::render_conflict(sides)
    }

    fn validate_resolution(&self, resolved: &DeclBlob) -> Result<(), ConflictError> {
        conflict::validate_resolution(resolved)
    }

    fn summarize_decl(&self, blob: &DeclBlob) -> Result<DeclSummary, ReadError> {
        read::summarize_decl(blob)
    }

    fn decl_detail(&self, blob: &DeclBlob) -> Result<DeclDetail, ReadError> {
        read::decl_detail(blob)
    }

    fn imports(&self, schema: &SchemaObjects) -> Result<Vec<Import>, ReadError> {
        read::imports(&schema.meta)
    }

    fn type_refs(&self, blob: &DeclBlob) -> Result<Vec<TypeRef>, ReadError> {
        read::type_refs(blob)
    }

    fn field_type_ref(
        &self,
        blob: &DeclBlob,
        field_name: &str,
    ) -> Result<Option<TypeRef>, ReadError> {
        read::field_type_ref(blob, field_name)
    }

    fn generate_descriptors(&self, closure: &SchemaClosure) -> Result<Bytes, DescriptorError> {
        codegen::generate_descriptors(closure)
    }

    fn generate_code(
        &self,
        closure: &SchemaClosure,
        lang: Language,
        options: &schemahub_types::CodegenOptions,
    ) -> Result<String, CodegenError> {
        codegen::generate_code(closure, lang, options)
    }
}
