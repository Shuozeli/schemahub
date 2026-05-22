pub mod ast;
pub mod compat;
pub mod diff;
pub mod mutations;
pub mod operations;
pub mod parser;
pub mod printer;

use std::collections::HashMap;

use bytes::Bytes;
use schemahub_types::{
    Blob, CompatibilityRules, CompatibilityViolation, DeclDetail, DeclKind, DeclSummary,
    DescriptorError, DiffError, FormatPlugin, Import, Language, Mutation, MutationError,
    ParseError, PrintError, ReadError, SchemaChange, SchemaPath,
    errors::CodegenError,
};

use crate::ast::{
    DeclBlob, ParseEnvelope, ParsedDecl, KIND_ENUM, KIND_METADATA, KIND_STRUCT,
    KIND_TABLE, KIND_UNION, decode_decl_blob, decode_decl_blob_print, decode_decl_blob_read,
    decode_envelope, decode_envelope_mutation, decode_envelope_print, encode_decl_blob,
    encode_envelope, unwrap_enum, unwrap_enum_print, unwrap_enum_read, unwrap_metadata,
    unwrap_metadata_print, unwrap_metadata_read, unwrap_struct_print, unwrap_struct_read,
    unwrap_table, unwrap_table_print, unwrap_table_read, unwrap_union_print, unwrap_union_read,
    wrap_enum, wrap_metadata, wrap_struct, wrap_table, wrap_union,
};
use crate::compat::{check_enum_compat, check_table_compat};
use crate::diff::diff_decl_blobs;
use crate::mutations::{apply_to_enum, apply_to_struct, apply_to_table};
use crate::operations::{FbsOp, decode_operation};
use crate::parser::parse_fbs;
use crate::printer::{print_enum, print_metadata, print_struct, print_table, print_union};

// ── FlatBuffersPlugin ─────────────────────────────────────────────────────────

pub struct FlatBuffersPlugin;

impl FormatPlugin for FlatBuffersPlugin {
    fn format_id(&self) -> &'static str {
        "flatbuffers"
    }

    // ── parse ─────────────────────────────────────────────────────────────────

    fn parse(&self, source: &str) -> Result<Blob, ParseError> {
        let parsed = parse_fbs(source)?;

        let mut declarations = Vec::new();

        // Metadata blob
        let meta_decl_blob = wrap_metadata(&parsed.metadata);
        declarations.push(ParsedDecl {
            tree_key: "__metadata__".into(),
            blob_bytes: encode_decl_blob(&meta_decl_blob),
        });

        // Tables
        for t in &parsed.tables {
            let decl_blob = wrap_table(t);
            declarations.push(ParsedDecl {
                tree_key: t.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        // Structs
        for s in &parsed.structs {
            let decl_blob = wrap_struct(s);
            declarations.push(ParsedDecl {
                tree_key: s.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        // Enums
        for e in &parsed.enums {
            let decl_blob = wrap_enum(e);
            declarations.push(ParsedDecl {
                tree_key: e.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        // Unions
        for u in &parsed.unions {
            let decl_blob = wrap_union(u);
            declarations.push(ParsedDecl {
                tree_key: u.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        let envelope = ParseEnvelope { declarations };
        Ok(Blob::from(encode_envelope(&envelope)))
    }

    // ── print ─────────────────────────────────────────────────────────────────

    fn print(&self, blob: &Blob) -> Result<String, PrintError> {
        let data = blob.as_bytes();

        // Try to decode as ParseEnvelope
        if let Ok(envelope) = decode_envelope_print(data) {
            if !envelope.declarations.is_empty() {
                return print_envelope(&envelope);
            }
        }

        // Fall back to single DeclBlob
        let decl_blob = decode_decl_blob_print(data)?;
        print_single_decl_blob(&decl_blob)
    }

    // ── diff ──────────────────────────────────────────────────────────────────

    fn diff(&self, old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError> {
        let maybe_detail = diff_decl_blobs(old.as_bytes(), new.as_bytes())?;

        if let Some(detail_bytes) = maybe_detail {
            let name = decl_name_from_blob(new.as_bytes()).unwrap_or_else(|| "unknown".into());
            let detail_text = String::from_utf8_lossy(&detail_bytes).into_owned();
            Ok(vec![SchemaChange::DeclarationModified {
                name,
                detail: Bytes::from(detail_text.into_bytes()),
            }])
        } else {
            Ok(vec![])
        }
    }

    // ── apply_mutation ────────────────────────────────────────────────────────

    fn apply_mutation(&self, blob: &Blob, mutation: &Mutation) -> Result<Blob, MutationError> {
        let op = decode_operation(&mutation.operation)?;
        let decl_name = &mutation.declaration_name;

        // Decode existing ParseEnvelope, or start empty
        let mut envelope = if blob.as_bytes().is_empty() {
            ParseEnvelope { declarations: vec![] }
        } else {
            decode_envelope_mutation(blob.as_bytes())?
        };

        // Schema-level (declaration create/delete/rename) ops
        match &op {
            FbsOp::AddTable(o) => {
                let new_table = crate::ast::TableBlob {
                    name: o.table_name.clone(),
                    fields: vec![],
                    doc_comment: o.doc_comment.clone(),
                };
                let decl_blob = wrap_table(&new_table);
                envelope.declarations.push(ParsedDecl {
                    tree_key: o.table_name.clone(),
                    blob_bytes: encode_decl_blob(&decl_blob),
                });
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            FbsOp::AddEnum(o) => {
                let new_enum = crate::ast::EnumBlob {
                    name: o.enum_name.clone(),
                    base_type: o.base_type.clone(),
                    values: vec![],
                    doc_comment: o.doc_comment.clone(),
                };
                let decl_blob = wrap_enum(&new_enum);
                envelope.declarations.push(ParsedDecl {
                    tree_key: o.enum_name.clone(),
                    blob_bytes: encode_decl_blob(&decl_blob),
                });
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            FbsOp::AddUnion(o) => {
                let new_union = crate::ast::UnionBlob {
                    name: o.union_name.clone(),
                    members: o.members.clone(),
                    doc_comment: o.doc_comment.clone(),
                };
                let decl_blob = wrap_union(&new_union);
                envelope.declarations.push(ParsedDecl {
                    tree_key: o.union_name.clone(),
                    blob_bytes: encode_decl_blob(&decl_blob),
                });
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            FbsOp::RemoveTable(o) => {
                envelope.declarations.retain(|d| d.tree_key != o.table_name);
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            FbsOp::RenameTable(o) => {
                if let Some(decl) = envelope.declarations.iter_mut().find(|d| d.tree_key == o.old_name) {
                    decl.tree_key = o.new_name.clone();
                    if let Ok(db) = decode_decl_blob(&decl.blob_bytes) {
                        if db.kind == KIND_TABLE {
                            if let Ok(mut tbl) = unwrap_table(&db) {
                                tbl.name = o.new_name.clone();
                                let new_db = wrap_table(&tbl);
                                decl.blob_bytes = encode_decl_blob(&new_db);
                            }
                        }
                    }
                }
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            FbsOp::UpdateImport(o) => {
                if let Some(meta_decl) = envelope.declarations.iter_mut().find(|d| d.tree_key == "__metadata__") {
                    let meta_db = decode_decl_blob(&meta_decl.blob_bytes)?;
                    let mut meta = unwrap_metadata(&meta_db)?;
                    if o.remove {
                        meta.imports.retain(|i| i != &o.import_path);
                    } else if !meta.imports.contains(&o.import_path) {
                        meta.imports.push(o.import_path.clone());
                    }
                    let new_meta_db = wrap_metadata(&meta);
                    meta_decl.blob_bytes = encode_decl_blob(&new_meta_db);
                }
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            _ => {}
        }

        // Declaration-level (field/value) ops: find the target declaration
        let decl_idx = envelope.declarations.iter()
            .position(|d| d.tree_key == *decl_name)
            .ok_or_else(|| MutationError::InvalidOperation(
                format!("declaration '{}' not found in schema", decl_name)
            ))?;

        let decl_blob = decode_decl_blob(&envelope.declarations[decl_idx].blob_bytes)?;

        let new_decl_blob = match decl_blob.kind {
            KIND_TABLE => {
                let tbl = unwrap_table(&decl_blob)?;
                let new_tbl = apply_to_table(&tbl, &op)?;
                wrap_table(&new_tbl)
            }
            KIND_ENUM => {
                let e = unwrap_enum(&decl_blob)?;
                let new_e = apply_to_enum(&e, &op)?;
                wrap_enum(&new_e)
            }
            KIND_STRUCT => {
                let s = crate::ast::unwrap_struct(&decl_blob)?;
                let _new_s = apply_to_struct(&s, &op)?; // always errors
                unreachable!()
            }
            KIND_UNION => {
                return Err(MutationError::UnsupportedInV1);
            }
            KIND_METADATA => {
                return Err(MutationError::InvalidOperation(
                    "cannot mutate __metadata__ blob directly — use UpdateImport".into(),
                ));
            }
            other => {
                return Err(MutationError::MalformedBlob(format!(
                    "unknown DeclBlob kind {other}"
                )));
            }
        };

        envelope.declarations[decl_idx].blob_bytes = encode_decl_blob(&new_decl_blob);
        Ok(Blob::from(encode_envelope(&envelope)))
    }

    // ── apply_mutations ───────────────────────────────────────────────────────

    fn apply_mutations(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        mutations: &[Mutation],
    ) -> Result<HashMap<SchemaPath, Blob>, MutationError> {
        let mut result: HashMap<SchemaPath, Blob> = HashMap::new();
        let mut working: HashMap<SchemaPath, Blob> = blobs.clone();

        for mutation in mutations {
            let current = working
                .get(&mutation.schema_path)
                .cloned()
                .unwrap_or_else(|| Blob::from(vec![]));
            let new_blob = self.apply_mutation(&current, mutation)?;
            working.insert(mutation.schema_path.clone(), new_blob.clone());
            result.insert(mutation.schema_path.clone(), new_blob);
        }

        Ok(result)
    }

    // ── check_compatibility ───────────────────────────────────────────────────

    fn check_compatibility(
        &self,
        old: &Blob,
        new: &Blob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>> {
        let direction = rules.direction;

        let old_decl = decode_decl_blob(old.as_bytes()).map_err(|e| {
            vec![CompatibilityViolation {
                declaration_name: "unknown".into(),
                field_name: None,
                message: format!("malformed old blob: {e}"),
            }]
        })?;

        let new_decl = decode_decl_blob(new.as_bytes()).map_err(|e| {
            vec![CompatibilityViolation {
                declaration_name: "unknown".into(),
                field_name: None,
                message: format!("malformed new blob: {e}"),
            }]
        })?;

        let violations = match (old_decl.kind, new_decl.kind) {
            (KIND_TABLE, KIND_TABLE) => {
                let old_tbl = unwrap_table(&old_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                let new_tbl = unwrap_table(&new_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                check_table_compat(&old_tbl, &new_tbl, direction)
            }
            (KIND_ENUM, KIND_ENUM) => {
                let old_e = unwrap_enum(&old_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                let new_e = unwrap_enum(&new_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                check_enum_compat(&old_e, &new_e, direction)
            }
            (KIND_STRUCT, KIND_STRUCT) => {
                // Structs are immutable — any diff is a violation
                if old_decl.data != new_decl.data {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: "struct changed: structs are immutable in FlatBuffers".into(),
                    }]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        };

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }

    // ── list_declarations ─────────────────────────────────────────────────────

    fn list_declarations(&self, blob: &Blob) -> Result<Vec<DeclSummary>, ReadError> {
        let envelope = decode_envelope(blob.as_bytes())?;
        let mut summaries = Vec::new();

        for parsed_decl in &envelope.declarations {
            if parsed_decl.tree_key == "__metadata__" {
                continue;
            }
            let decl_blob = decode_decl_blob_read(&parsed_decl.blob_bytes)?;
            let summary = match decl_blob.kind {
                KIND_TABLE => {
                    let t = unwrap_table_read(&decl_blob)?;
                    DeclSummary {
                        name: t.name.clone(),
                        kind: DeclKind::Table,
                        doc_comment: first_line(&t.doc_comment),
                    }
                }
                KIND_STRUCT => {
                    let s = unwrap_struct_read(&decl_blob)?;
                    DeclSummary {
                        name: s.name.clone(),
                        kind: DeclKind::Struct,
                        doc_comment: first_line(&s.doc_comment),
                    }
                }
                KIND_ENUM => {
                    let e = unwrap_enum_read(&decl_blob)?;
                    DeclSummary {
                        name: e.name.clone(),
                        kind: DeclKind::FbsEnum,
                        doc_comment: first_line(&e.doc_comment),
                    }
                }
                KIND_UNION => {
                    let u = unwrap_union_read(&decl_blob)?;
                    DeclSummary {
                        name: u.name.clone(),
                        kind: DeclKind::Union,
                        doc_comment: first_line(&u.doc_comment),
                    }
                }
                KIND_METADATA => continue,
                other => {
                    return Err(ReadError::MalformedBlob(format!(
                        "unknown DeclBlob kind {other}"
                    )));
                }
            };
            summaries.push(summary);
        }

        Ok(summaries)
    }

    // ── get_declaration ───────────────────────────────────────────────────────

    fn get_declaration(&self, blob: &Blob, name: &str) -> Result<DeclDetail, ReadError> {
        let data = blob.as_bytes();

        if let Ok(envelope) = decode_envelope(data) {
            if !envelope.declarations.is_empty() {
                let found = envelope
                    .declarations
                    .iter()
                    .find(|d| d.tree_key == name)
                    .ok_or_else(|| ReadError::NotFound(name.to_owned()))?;
                let decl_blob = decode_decl_blob_read(&found.blob_bytes)?;
                let text = print_single_decl_blob_read(&decl_blob)?;
                return Ok(DeclDetail::new(text));
            }
        }

        let decl_blob = decode_decl_blob_read(data)?;
        let text = print_single_decl_blob_read(&decl_blob)?;
        Ok(DeclDetail::new(text))
    }

    // ── imports ───────────────────────────────────────────────────────────────

    fn imports(&self, blob: &Blob) -> Result<Vec<Import>, ReadError> {
        let envelope = decode_envelope(blob.as_bytes())?;

        let meta_decl = envelope
            .declarations
            .iter()
            .find(|d| d.tree_key == "__metadata__")
            .ok_or_else(|| ReadError::NotFound("__metadata__".into()))?;

        let meta_db = decode_decl_blob_read(&meta_decl.blob_bytes)?;
        let metadata = unwrap_metadata_read(&meta_db)?;

        Ok(metadata
            .imports
            .into_iter()
            .map(|path| Import {
                path,
                resolved_commit: String::new(),
                decl_name: String::new(),
            })
            .collect())
    }

    // ── generate_descriptors ──────────────────────────────────────────────────

    fn generate_descriptors(
        &self,
        _blobs: &HashMap<SchemaPath, Blob>,
    ) -> Result<Bytes, DescriptorError> {
        Err(DescriptorError::Other(
            "FlatBuffers descriptor generation not yet implemented".into(),
        ))
    }

    // ── generate_code ─────────────────────────────────────────────────────────

    fn generate_code(
        &self,
        _blobs: &HashMap<SchemaPath, Blob>,
        language: Language,
    ) -> Result<String, CodegenError> {
        Err(CodegenError::UnsupportedLanguage(language))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_owned()
}

fn decl_name_from_blob(data: &[u8]) -> Option<String> {
    let decl_blob = decode_decl_blob(data).ok()?;
    match decl_blob.kind {
        KIND_TABLE => {
            let t = unwrap_table(&decl_blob).ok()?;
            Some(t.name)
        }
        KIND_ENUM => {
            let e = unwrap_enum(&decl_blob).ok()?;
            Some(e.name)
        }
        KIND_STRUCT => {
            let s = crate::ast::unwrap_struct(&decl_blob).ok()?;
            Some(s.name)
        }
        KIND_UNION => {
            let u = crate::ast::unwrap_union(&decl_blob).ok()?;
            Some(u.name)
        }
        _ => None,
    }
}

fn print_envelope(envelope: &ParseEnvelope) -> Result<String, PrintError> {
    let mut out = String::new();

    if let Some(meta_decl) = envelope.declarations.iter().find(|d| d.tree_key == "__metadata__") {
        let meta_db = decode_decl_blob_print(&meta_decl.blob_bytes)?;
        let meta = unwrap_metadata_print(&meta_db)?;
        let header = print_metadata(&meta);
        if !header.is_empty() {
            out.push_str(&header);
            out.push('\n');
        }
    }

    for parsed_decl in &envelope.declarations {
        if parsed_decl.tree_key == "__metadata__" {
            continue;
        }
        let decl_blob = decode_decl_blob_print(&parsed_decl.blob_bytes)?;
        let fragment = print_single_decl_blob(&decl_blob)?;
        out.push_str(&fragment);
        out.push('\n');
    }

    Ok(out)
}

fn print_single_decl_blob(decl_blob: &DeclBlob) -> Result<String, PrintError> {
    match decl_blob.kind {
        KIND_TABLE => {
            let t = unwrap_table_print(decl_blob)?;
            Ok(print_table(&t))
        }
        KIND_STRUCT => {
            let s = unwrap_struct_print(decl_blob)?;
            Ok(print_struct(&s))
        }
        KIND_ENUM => {
            let e = unwrap_enum_print(decl_blob)?;
            Ok(print_enum(&e))
        }
        KIND_UNION => {
            let u = unwrap_union_print(decl_blob)?;
            Ok(print_union(&u))
        }
        KIND_METADATA => {
            let m = unwrap_metadata_print(decl_blob)?;
            Ok(print_metadata(&m))
        }
        other => Err(PrintError::MalformedBlob(format!(
            "unknown DeclBlob kind {other}"
        ))),
    }
}

fn print_single_decl_blob_read(decl_blob: &DeclBlob) -> Result<String, ReadError> {
    match decl_blob.kind {
        KIND_TABLE => {
            let t = unwrap_table_read(decl_blob)?;
            Ok(print_table(&t))
        }
        KIND_STRUCT => {
            let s = unwrap_struct_read(decl_blob)?;
            Ok(print_struct(&s))
        }
        KIND_ENUM => {
            let e = unwrap_enum_read(decl_blob)?;
            Ok(print_enum(&e))
        }
        KIND_UNION => {
            let u = unwrap_union_read(decl_blob)?;
            Ok(print_union(&u))
        }
        KIND_METADATA => {
            let m = unwrap_metadata_read(decl_blob)?;
            Ok(print_metadata(&m))
        }
        other => Err(ReadError::MalformedBlob(format!(
            "unknown DeclBlob kind {other}"
        ))),
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use crate::operations::{
        FbsOperationEnvelope, OpAddEnumValue, OpAddField, OpDeprecateField, OpRenameField,
        op_tag,
    };

    const SOURCE: &str = r#"
        namespace Payments.V1;
        include "common.fbs";

        // An order record
        table Order {
            id: string;
            amount: int32;
            status: int32 = 0;
        }

        table Item {
            name: string;
            price: int64;
        }

        // Payment status enum
        enum PaymentStatus : byte {
            UNKNOWN = 0,
            PENDING = 1,
            COMPLETED = 2,
        }

        struct Point {
            x: float;
            y: float;
        }

        union Shape { Circle, Square }

        root_type Order;
    "#;

    fn make_op(tag: u32, payload: Vec<u8>) -> Bytes {
        Bytes::from(FbsOperationEnvelope::encode_op(tag, payload))
    }

    fn make_mutation(schema_path: &str, decl_name: &str, tag: u32, payload: Vec<u8>) -> Mutation {
        Mutation {
            schema_path: SchemaPath::new("proj", "repo", schema_path),
            format_id: "flatbuffers".into(),
            declaration_name: decl_name.into(),
            operation: make_op(tag, payload),
        }
    }

    #[test]
    fn plugin_parse_returns_envelope() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let decls = plugin.list_declarations(&blob).unwrap();
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Order"), "missing table: {names:?}");
        assert!(names.contains(&"PaymentStatus"), "missing enum: {names:?}");
        assert!(names.contains(&"Point"), "missing struct: {names:?}");
        assert!(names.contains(&"Shape"), "missing union: {names:?}");
    }

    #[test]
    fn plugin_list_declarations_kinds() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let decls = plugin.list_declarations(&blob).unwrap();

        let order = decls.iter().find(|d| d.name == "Order").unwrap();
        assert_eq!(order.kind, DeclKind::Table);

        let status = decls.iter().find(|d| d.name == "PaymentStatus").unwrap();
        assert_eq!(status.kind, DeclKind::FbsEnum);
    }

    #[test]
    fn plugin_imports() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let imports = plugin.imports(&blob).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, "common.fbs");
    }

    #[test]
    fn plugin_get_declaration() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let detail = plugin.get_declaration(&blob, "Order").unwrap();
        let text = std::str::from_utf8(detail.as_bytes()).unwrap();
        assert!(text.contains("Order"), "got: {text}");
        assert!(text.contains("id"), "got: {text}");
    }

    #[test]
    fn plugin_get_declaration_not_found() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let result = plugin.get_declaration(&blob, "NonExistent");
        assert!(matches!(result, Err(ReadError::NotFound(_))));
    }

    #[test]
    fn plugin_print_roundtrip() {
        let plugin = FlatBuffersPlugin;
        let blob1 = plugin.parse(SOURCE).unwrap();
        let text = plugin.print(&blob1).unwrap();
        let blob2 = plugin.parse(&text).unwrap();

        let decls1 = plugin.list_declarations(&blob1).unwrap();
        let decls2 = plugin.list_declarations(&blob2).unwrap();

        let names1: Vec<&str> = decls1.iter().map(|d| d.name.as_str()).collect();
        let names2: Vec<&str> = decls2.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names1, names2, "declaration names differ after roundtrip");
    }

    #[test]
    fn plugin_diff_identical() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse(SOURCE).unwrap();

        let envelope = decode_envelope(blob.as_bytes()).unwrap();
        let decl = envelope.declarations.iter().find(|d| d.tree_key == "Order").unwrap();
        let decl_blob = Blob::from(decl.blob_bytes.clone());

        let changes = plugin.diff(&decl_blob, &decl_blob).unwrap();
        assert!(changes.is_empty(), "identical blobs should produce no changes");
    }

    #[test]
    fn plugin_diff_detects_change() {
        let plugin = FlatBuffersPlugin;

        let src1 = "table Foo { id: string; }";
        let src2 = "table Foo { id: string; amount: int32; }";

        let blob1 = plugin.parse(src1).unwrap();
        let blob2 = plugin.parse(src2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Foo");
        let new_decl = get_decl_blob(&blob2, "Foo");
        let changes = plugin.diff(&old_decl, &new_decl).unwrap();
        assert!(!changes.is_empty(), "should detect field addition");
    }

    fn decl_from_result<'a>(envelope: &'a crate::ast::ParseEnvelope, name: &str) -> &'a crate::ast::ParsedDecl {
        envelope.declarations.iter().find(|d| d.tree_key == name).unwrap()
    }

    #[test]
    fn apply_mutation_add_field() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse("table Order { id: string; }").unwrap();

        let payload = OpAddField {
            field_name: "amount".into(),
            field_type: "int32".into(),
            default_value: String::new(),
            doc_comment: String::new(),
        }.encode_to_vec();

        let mutation = make_mutation("schema.fbs", "Order", op_tag::ADD_FIELD, payload);
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let envelope = decode_envelope(new_blob.as_bytes()).unwrap();
        let decl = decl_from_result(&envelope, "Order");
        let tbl = unwrap_table(&decode_decl_blob(&decl.blob_bytes).unwrap()).unwrap();
        assert_eq!(tbl.fields.len(), 2);
        assert_eq!(tbl.fields[1].name, "amount");
        assert_eq!(tbl.fields[1].slot_index, 1);
    }

    #[test]
    fn apply_mutation_deprecate_field() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse("table Order { id: string; old_field: string; }").unwrap();

        let payload = OpDeprecateField { field_name: "old_field".into() }.encode_to_vec();
        let mutation = make_mutation("schema.fbs", "Order", op_tag::DEPRECATE_FIELD, payload);
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let envelope = decode_envelope(new_blob.as_bytes()).unwrap();
        let decl = decl_from_result(&envelope, "Order");
        let tbl = unwrap_table(&decode_decl_blob(&decl.blob_bytes).unwrap()).unwrap();
        let f = tbl.fields.iter().find(|f| f.name == "old_field").unwrap();
        assert!(f.deprecated, "field should be deprecated");
        assert_eq!(f.slot_index, 1, "slot must be unchanged");
    }

    #[test]
    fn apply_mutation_rename_field() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse("table Order { id: string; }").unwrap();

        let payload = OpRenameField {
            old_field_name: "id".into(),
            new_field_name: "order_id".into(),
        }.encode_to_vec();
        let mutation = make_mutation("schema.fbs", "Order", op_tag::RENAME_FIELD, payload);
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let envelope = decode_envelope(new_blob.as_bytes()).unwrap();
        let decl = decl_from_result(&envelope, "Order");
        let tbl = unwrap_table(&decode_decl_blob(&decl.blob_bytes).unwrap()).unwrap();
        assert_eq!(tbl.fields[0].name, "order_id");
        assert_eq!(tbl.fields[0].slot_index, 0);
    }

    #[test]
    fn apply_mutation_add_enum_value() {
        let plugin = FlatBuffersPlugin;
        let blob = plugin.parse("enum Status : byte { UNKNOWN = 0 }").unwrap();

        let payload = OpAddEnumValue {
            enum_name: "Status".into(),
            value_name: "ACTIVE".into(),
            value: 1,
            doc_comment: String::new(),
        }.encode_to_vec();
        let mutation = make_mutation("schema.fbs", "Status", op_tag::ADD_ENUM_VALUE, payload);
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let envelope = decode_envelope(new_blob.as_bytes()).unwrap();
        let decl = decl_from_result(&envelope, "Status");
        let e = unwrap_enum(&decode_decl_blob(&decl.blob_bytes).unwrap()).unwrap();
        assert_eq!(e.values.len(), 2);
        assert_eq!(e.values[1].name, "ACTIVE");
    }

    #[test]
    fn check_compat_full_add_field_ok() {
        use schemahub_types::{CompatibilityDirection, CompatibilityRules};

        let plugin = FlatBuffersPlugin;
        let src1 = "table Foo { id: string; }";
        let src2 = "table Foo { id: string; amount: int32; }";

        let blob1 = plugin.parse(src1).unwrap();
        let blob2 = plugin.parse(src2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Foo");
        let new_decl = get_decl_blob(&blob2, "Foo");

        let rules = CompatibilityRules { direction: CompatibilityDirection::Full };
        let result = plugin.check_compatibility(&old_decl, &new_decl, &rules);
        assert!(result.is_ok(), "adding field at end should be FULL compatible: {result:?}");
    }

    #[test]
    fn check_compat_enum_add_value_backward_ok() {
        use schemahub_types::{CompatibilityDirection, CompatibilityRules};

        let plugin = FlatBuffersPlugin;
        let src1 = "enum Status : byte { UNKNOWN = 0 }";
        let src2 = "enum Status : byte { UNKNOWN = 0, ACTIVE = 1 }";

        let blob1 = plugin.parse(src1).unwrap();
        let blob2 = plugin.parse(src2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Status");
        let new_decl = get_decl_blob(&blob2, "Status");

        let rules = CompatibilityRules { direction: CompatibilityDirection::Backward };
        let result = plugin.check_compatibility(&old_decl, &new_decl, &rules);
        assert!(result.is_ok(), "adding enum value should be BACKWARD ok: {result:?}");
    }

    #[test]
    fn check_compat_enum_add_value_full_violation() {
        use schemahub_types::{CompatibilityDirection, CompatibilityRules};

        let plugin = FlatBuffersPlugin;
        let src1 = "enum Status : byte { UNKNOWN = 0 }";
        let src2 = "enum Status : byte { UNKNOWN = 0, ACTIVE = 1 }";

        let blob1 = plugin.parse(src1).unwrap();
        let blob2 = plugin.parse(src2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Status");
        let new_decl = get_decl_blob(&blob2, "Status");

        let rules = CompatibilityRules { direction: CompatibilityDirection::Full };
        let result = plugin.check_compatibility(&old_decl, &new_decl, &rules);
        assert!(result.is_err(), "adding enum value should break FULL compat");
    }

    #[test]
    fn format_id_is_flatbuffers() {
        assert_eq!(FlatBuffersPlugin.format_id(), "flatbuffers");
    }
}
