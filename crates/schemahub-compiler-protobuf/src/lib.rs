//! `schemahub-compiler-protobuf` — the Protobuf compiler, wrapping
//! `protobuf-rs` (`protoc-rs-parser`, `protoc-rs-schema`, `protoc-rs-codegen`)
//! and owning the `.proto` printer (design.md §3.1, crate-structure.md §3.5).
//!
//! It parses `.proto` source into a real `FileDescriptorProto`, splits it into
//! one `DeclBlob` per top-level message/enum/service plus a file `MetaBlob`,
//! and reassembles + prints canonical source for round-tripping.

mod blob;
mod codec;
mod codegen;
mod compat;
mod conflict;
mod diff;
mod mutations;
mod operations;
mod parse;
mod printer;
mod read;
mod reassemble;
mod typecompat;

pub use blob::{decode_decl, decode_meta, DeclPayload, MetaPayload, BLOB_VERSION};
pub use operations::{
    tag, OpAddEnumValue, OpAddField, OpAddRpc, OpAddService, OpChangeCardinality,
    OpChangeFieldNumber, OpChangeFieldType, OpChangeRpcType, OpCreateEnum, OpCreateMessage,
    OpDeleteEnum, OpDeleteMessage, OpRemoveEnumValue, OpRemoveField, OpRemoveRpc, OpRemoveService,
    OpRenameEnumValue, OpRenameField, OpRenameMessage, OpRenameRpc, OpRenameService,
    OpReorderFields, OpUpdateImport, ProtoOp, ProtoOperation,
};

use bytes::Bytes;

use schemahub_types::{
    CodegenError, CompatibilityRules, CompatibilityViolation, Compiler, ConflictError,
    ConflictSides, DeclBlob, DeclChange, DeclDetail, DeclSummary, DescriptorError, DiffError,
    Import, Language, Mutation, MutationEffect, MutationError, ParseError, ParsedSchema,
    PrintError, ReadError, SchemaClosure, SchemaObjects, TypeRef,
};

/// The Protobuf compiler front-end.
#[derive(Debug, Default, Clone)]
pub struct ProtobufCompiler;

impl ProtobufCompiler {
    pub fn new() -> Self {
        Self
    }
}

impl Compiler for ProtobufCompiler {
    fn format_id(&self) -> &'static str {
        "protobuf"
    }

    fn parse(&self, source: &str) -> Result<ParsedSchema, ParseError> {
        parse::parse_source(source)
    }

    fn print(&self, schema: &SchemaObjects) -> Result<String, PrintError> {
        let file = reassemble::reassemble(schema)?;
        Ok(printer::print_file(&file))
    }

    fn diff_decl(&self, old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError> {
        diff::diff(old, new)
    }

    fn apply_mutation(
        &self,
        schema: &SchemaObjects,
        op: &Mutation,
    ) -> Result<MutationEffect, MutationError> {
        mutations::apply_one(schema, op)
    }

    fn apply_mutations(
        &self,
        schema: &SchemaObjects,
        ops: &[Mutation],
    ) -> Result<MutationEffect, MutationError> {
        mutations::apply_many(schema, ops)
    }

    fn check_compatibility(
        &self,
        old: &DeclBlob,
        new: &DeclBlob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>> {
        compat::check(old, new, rules)
    }

    fn render_conflict(&self, sides: &ConflictSides) -> Result<String, ConflictError> {
        conflict::render(sides)
    }

    fn validate_resolution(&self, resolved: &DeclBlob) -> Result<(), ConflictError> {
        conflict::validate_resolution(resolved)
    }

    fn summarize_decl(&self, blob: &DeclBlob) -> Result<DeclSummary, ReadError> {
        read::summarize(blob)
    }

    fn decl_detail(&self, blob: &DeclBlob) -> Result<DeclDetail, ReadError> {
        read::detail(blob)
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
        _options: &schemahub_types::CodegenOptions,
    ) -> Result<String, CodegenError> {
        codegen::generate_code(closure, lang)
    }
}
