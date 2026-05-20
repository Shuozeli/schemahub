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
    DeclBlob, FileMetadataBlob, ImportDef, ParseEnvelope, ParsedDecl, KIND_ENUM, KIND_MESSAGE,
    KIND_METADATA, KIND_SERVICE, decode_decl_blob, decode_decl_blob_print, decode_decl_blob_read,
    decode_envelope, decode_envelope_print, encode_decl_blob,
    encode_envelope, unwrap_enum, unwrap_enum_print, unwrap_enum_read, unwrap_message,
    unwrap_message_print, unwrap_message_read, unwrap_metadata_print,
    unwrap_metadata_read, unwrap_service, unwrap_service_print, unwrap_service_read, wrap_enum,
    wrap_message, wrap_metadata, wrap_service,
};
use crate::compat::{check_enum_compat, check_message_compat, check_service_compat};
use crate::diff::diff_decl_blobs;
use crate::mutations::{apply_add_enum_value, apply_to_message};
use crate::operations::{ProtoOp, decode_operation};
use crate::parser::parse_proto;
use crate::printer::{print_enum, print_message, print_service};

// ── ProtobufPlugin ────────────────────────────────────────────────────────────

pub struct ProtobufPlugin;

impl FormatPlugin for ProtobufPlugin {
    fn format_id(&self) -> &'static str {
        "protobuf"
    }

    // ── parse ─────────────────────────────────────────────────────────────────

    fn parse(&self, source: &str) -> Result<Blob, ParseError> {
        let proto_file = parse_proto(source)?;

        let mut declarations = Vec::new();

        // Metadata blob
        let metadata = FileMetadataBlob {
            blob_version: 1,
            syntax: proto_file.syntax.clone(),
            package: proto_file.package.clone(),
            imports: proto_file
                .imports
                .iter()
                .map(|p| ImportDef {
                    path: p.clone(),
                    resolved_commit: String::new(),
                })
                .collect(),
        };
        let meta_decl_blob = wrap_metadata(&metadata);
        declarations.push(ParsedDecl {
            tree_key: "__metadata__".into(),
            blob_bytes: encode_decl_blob(&meta_decl_blob),
        });

        // Messages
        for msg in &proto_file.messages {
            let decl_blob = wrap_message(msg);
            declarations.push(ParsedDecl {
                tree_key: msg.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        // Enums
        for e in &proto_file.enums {
            let decl_blob = wrap_enum(e);
            declarations.push(ParsedDecl {
                tree_key: e.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        // Services
        for svc in &proto_file.services {
            let decl_blob = wrap_service(svc);
            declarations.push(ParsedDecl {
                tree_key: svc.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        let envelope = ParseEnvelope {
            blob_version: 1,
            declarations,
        };

        Ok(Blob::from(encode_envelope(&envelope)))
    }

    // ── print ─────────────────────────────────────────────────────────────────

    fn print(&self, blob: &Blob) -> Result<String, PrintError> {
        let data = blob.as_bytes();

        // Try to decode as ParseEnvelope first
        if let Ok(envelope) = decode_envelope_print(data) {
            if envelope.blob_version > 0 || !envelope.declarations.is_empty() {
                return print_envelope(&envelope);
            }
        }

        // Fall back to single DeclBlob
        let decl_blob = decode_decl_blob_print(data)?;
        print_single_decl_blob(&decl_blob)
    }

    // ── diff ──────────────────────────────────────────────────────────────────

    fn diff(&self, old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError> {
        // Both blobs are individual DeclBlobs (called per-declaration by the core)
        let maybe_detail = diff_decl_blobs(old.as_bytes(), new.as_bytes())?;

        if let Some(detail) = maybe_detail {
            // Determine the declaration name from the new blob
            let name = decl_name_from_blob(new.as_bytes()).unwrap_or_else(|| "unknown".into());
            Ok(vec![SchemaChange::DeclarationModified { name, detail }])
        } else {
            Ok(vec![])
        }
    }

    // ── apply_mutation ────────────────────────────────────────────────────────

    fn apply_mutation(&self, blob: &Blob, mutation: &Mutation) -> Result<Blob, MutationError> {
        let op = decode_operation(&mutation.operation)?;

        // Handle declaration-level ops that create/delete blobs
        match &op {
            ProtoOp::AddMessage(o) => {
                let new_msg = crate::ast::MessageBlob {
                    blob_version: 1,
                    name: o.message_name.clone(),
                    fields: vec![],
                    reserved_numbers: vec![],
                    reserved_names: vec![],
                    doc_comment: o.doc_comment.clone(),
                };
                let decl_blob = wrap_message(&new_msg);
                return Ok(Blob::from(encode_decl_blob(&decl_blob)));
            }
            ProtoOp::AddEnum(o) => {
                let new_enum = crate::ast::EnumBlob {
                    blob_version: 1,
                    name: o.enum_name.clone(),
                    values: vec![],
                    doc_comment: o.doc_comment.clone(),
                };
                let decl_blob = wrap_enum(&new_enum);
                return Ok(Blob::from(encode_decl_blob(&decl_blob)));
            }
            ProtoOp::RemoveMessage(_) => {
                // Signal deletion with a special sentinel
                let sentinel = crate::ast::MessageBlob {
                    blob_version: 1,
                    name: "__deleted__".into(),
                    fields: vec![],
                    reserved_numbers: vec![],
                    reserved_names: vec![],
                    doc_comment: String::new(),
                };
                let decl_blob = wrap_message(&sentinel);
                return Ok(Blob::from(encode_decl_blob(&decl_blob)));
            }
            _ => {}
        }

        // Decode the current DeclBlob
        let decl_blob = decode_decl_blob(blob.as_bytes())?;

        let new_decl_blob = match decl_blob.kind {
            KIND_MESSAGE => {
                let msg = unwrap_message(&decl_blob)?;
                let new_msg = apply_to_message(&msg, &op)?;
                wrap_message(&new_msg)
            }
            KIND_ENUM => {
                let e = unwrap_enum(&decl_blob)?;
                let new_enum = match &op {
                    ProtoOp::AddEnumValue(o) => apply_add_enum_value(&e, o)?,
                    _ => {
                        return Err(MutationError::InvalidOperation(format!(
                            "operation {:?} cannot be applied to an enum blob",
                            std::mem::discriminant(&op)
                        )));
                    }
                };
                wrap_enum(&new_enum)
            }
            KIND_SERVICE => {
                return Err(MutationError::UnsupportedInV1);
            }
            KIND_METADATA => {
                return Err(MutationError::InvalidOperation(
                    "cannot mutate __metadata__ blob directly".into(),
                ));
            }
            other => {
                return Err(MutationError::MalformedBlob(format!(
                    "unknown DeclBlob kind {other}"
                )));
            }
        };

        Ok(Blob::from(encode_decl_blob(&new_decl_blob)))
    }

    // ── apply_mutations ───────────────────────────────────────────────────────

    fn apply_mutations(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        mutations: &[Mutation],
    ) -> Result<HashMap<SchemaPath, Blob>, MutationError> {
        let mut result: HashMap<SchemaPath, Blob> = HashMap::new();

        // Work on a mutable copy that merges blobs + result
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

        let old_decl = decode_decl_blob(old.as_bytes())
            .map_err(|e| vec![CompatibilityViolation {
                declaration_name: "unknown".into(),
                field_name: None,
                message: format!("malformed old blob: {e}"),
            }])?;

        let new_decl = decode_decl_blob(new.as_bytes())
            .map_err(|e| vec![CompatibilityViolation {
                declaration_name: "unknown".into(),
                field_name: None,
                message: format!("malformed new blob: {e}"),
            }])?;

        let violations = match (old_decl.kind, new_decl.kind) {
            (KIND_MESSAGE, KIND_MESSAGE) => {
                let old_msg = unwrap_message(&old_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                let new_msg = unwrap_message(&new_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                check_message_compat(&old_msg, &new_msg, direction)
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
            (KIND_SERVICE, KIND_SERVICE) => {
                let old_svc = unwrap_service(&old_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                let new_svc = unwrap_service(&new_decl).map_err(|e| {
                    vec![CompatibilityViolation {
                        declaration_name: "unknown".into(),
                        field_name: None,
                        message: e.to_string(),
                    }]
                })?;
                check_service_compat(&old_svc, &new_svc, direction)
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
                KIND_MESSAGE => {
                    let msg = unwrap_message_read(&decl_blob)?;
                    DeclSummary {
                        name: msg.name.clone(),
                        kind: DeclKind::Message,
                        doc_comment: first_line(&msg.doc_comment),
                    }
                }
                KIND_ENUM => {
                    let e = unwrap_enum_read(&decl_blob)?;
                    DeclSummary {
                        name: e.name.clone(),
                        kind: DeclKind::Enum,
                        doc_comment: first_line(&e.doc_comment),
                    }
                }
                KIND_SERVICE => {
                    let svc = unwrap_service_read(&decl_blob)?;
                    DeclSummary {
                        name: svc.name.clone(),
                        kind: DeclKind::Service,
                        doc_comment: first_line(&svc.doc_comment),
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

        // Try envelope first
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

        // Single DeclBlob
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

        let meta_blob = decode_decl_blob_read(&meta_decl.blob_bytes)?;
        let metadata = unwrap_metadata_read(&meta_blob)?;

        Ok(metadata
            .imports
            .into_iter()
            .map(|imp| Import {
                path: imp.path,
                resolved_commit: imp.resolved_commit,
                decl_name: String::new(), // proto imports are file-level
            })
            .collect())
    }

    // ── generate_descriptors ──────────────────────────────────────────────────

    fn generate_descriptors(
        &self,
        _blobs: &HashMap<SchemaPath, Blob>,
    ) -> Result<Bytes, DescriptorError> {
        // TODO: implement FileDescriptorSet generation using prost-types
        Err(DescriptorError::Other("not yet implemented".into()))
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
        KIND_MESSAGE => {
            let msg = unwrap_message(&decl_blob).ok()?;
            Some(msg.name)
        }
        KIND_ENUM => {
            let e = unwrap_enum(&decl_blob).ok()?;
            Some(e.name)
        }
        KIND_SERVICE => {
            let svc = unwrap_service(&decl_blob).ok()?;
            Some(svc.name)
        }
        _ => None,
    }
}

fn print_envelope(envelope: &ParseEnvelope) -> Result<String, PrintError> {
    let mut out = String::new();

    // Find metadata
    let mut syntax = "proto3".to_owned();
    let mut package = String::new();
    let mut imports: Vec<String> = Vec::new();

    if let Some(meta_decl) = envelope.declarations.iter().find(|d| d.tree_key == "__metadata__") {
        let meta_blob = decode_decl_blob_print(&meta_decl.blob_bytes)?;
        let meta = unwrap_metadata_print(&meta_blob)?;
        syntax = meta.syntax;
        package = meta.package;
        imports = meta.imports.into_iter().map(|i| i.path).collect();
    }

    out.push_str(&format!("syntax = \"{syntax}\";\n"));
    if !package.is_empty() {
        out.push_str(&format!("package {package};\n"));
    }
    for import in &imports {
        out.push_str(&format!("import \"{import}\";\n"));
    }
    if !imports.is_empty() || !package.is_empty() {
        out.push('\n');
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
        KIND_MESSAGE => {
            let msg = unwrap_message_print(decl_blob)?;
            Ok(print_message(&msg))
        }
        KIND_ENUM => {
            let e = unwrap_enum_print(decl_blob)?;
            Ok(print_enum(&e))
        }
        KIND_SERVICE => {
            let svc = unwrap_service_print(decl_blob)?;
            Ok(print_service(&svc))
        }
        KIND_METADATA => Ok(String::new()),
        other => Err(PrintError::MalformedBlob(format!(
            "unknown DeclBlob kind {other}"
        ))),
    }
}

fn print_single_decl_blob_read(decl_blob: &DeclBlob) -> Result<String, ReadError> {
    match decl_blob.kind {
        KIND_MESSAGE => {
            let msg = unwrap_message_read(decl_blob)?;
            Ok(print_message(&msg))
        }
        KIND_ENUM => {
            let e = unwrap_enum_read(decl_blob)?;
            Ok(print_enum(&e))
        }
        KIND_SERVICE => {
            let svc = unwrap_service_read(decl_blob)?;
            Ok(print_service(&svc))
        }
        KIND_METADATA => Ok(String::new()),
        other => Err(ReadError::MalformedBlob(format!(
            "unknown DeclBlob kind {other}"
        ))),
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_proto;

    const SOURCE: &str = r#"
        syntax = "proto3";
        package payments;

        // A payment creation request
        message CreatePaymentRequest {
          string user_id = 1;
          int64 amount_cents = 2;
        }

        message CreatePaymentResponse {
          string payment_id = 1;
        }

        enum PaymentStatus {
          UNKNOWN = 0;
          PENDING = 1;
          COMPLETED = 2;
        }

        service PaymentService {
          rpc CreatePayment (CreatePaymentRequest) returns (CreatePaymentResponse);
        }
    "#;

    #[test]
    fn plugin_parse_returns_envelope() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let decls = plugin.list_declarations(&blob).unwrap();
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"CreatePaymentRequest"), "missing message: {names:?}");
        assert!(names.contains(&"PaymentStatus"), "missing enum: {names:?}");
        assert!(names.contains(&"PaymentService"), "missing service: {names:?}");
    }

    #[test]
    fn plugin_list_declarations_kinds() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let decls = plugin.list_declarations(&blob).unwrap();

        let msg = decls.iter().find(|d| d.name == "CreatePaymentRequest").unwrap();
        assert_eq!(msg.kind, DeclKind::Message);

        let e = decls.iter().find(|d| d.name == "PaymentStatus").unwrap();
        assert_eq!(e.kind, DeclKind::Enum);

        let svc = decls.iter().find(|d| d.name == "PaymentService").unwrap();
        assert_eq!(svc.kind, DeclKind::Service);
    }

    #[test]
    fn plugin_imports() {
        let source = r#"
            syntax = "proto3";
            import "google/protobuf/timestamp.proto";
            message Foo { string x = 1; }
        "#;
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(source).unwrap();
        let imports = plugin.imports(&blob).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, "google/protobuf/timestamp.proto");
    }

    #[test]
    fn plugin_get_declaration() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let detail = plugin.get_declaration(&blob, "CreatePaymentRequest").unwrap();
        let text = std::str::from_utf8(detail.as_bytes()).unwrap();
        assert!(text.contains("CreatePaymentRequest"), "got: {text}");
        assert!(text.contains("user_id"));
    }

    #[test]
    fn plugin_print_roundtrip() {
        let plugin = ProtobufPlugin;
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
    fn parse_print_parse_roundtrip() {
        let plugin = ProtobufPlugin;
        let source = r#"
            syntax = "proto3";
            package test;
            message Foo {
              string name = 1;
              int32 count = 2;
            }
        "#;
        let blob1 = plugin.parse(source).unwrap();
        let printed = plugin.print(&blob1).unwrap();
        let blob2 = plugin.parse(&printed).unwrap();

        let parsed1 = parse_proto(source).unwrap();
        let parsed2 = parse_proto(&printed).unwrap();

        // field names and numbers should match
        assert_eq!(parsed1.messages[0].fields.len(), parsed2.messages[0].fields.len());
        for (f1, f2) in parsed1.messages[0].fields.iter().zip(parsed2.messages[0].fields.iter()) {
            assert_eq!(f1.name, f2.name);
            assert_eq!(f1.number, f2.number);
            assert_eq!(f1.field_type, f2.field_type);
        }

        // blobs should also match declaration names
        let decls1 = plugin.list_declarations(&blob1).unwrap();
        let decls2 = plugin.list_declarations(&blob2).unwrap();
        assert_eq!(decls1.len(), decls2.len());
    }

    #[test]
    fn plugin_diff_identical() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();

        // Get a single declaration blob to diff
        let envelope = crate::ast::decode_envelope(blob.as_bytes()).unwrap();
        let decl = envelope.declarations.iter().find(|d| d.tree_key == "CreatePaymentRequest").unwrap();
        let decl_blob = Blob::from(decl.blob_bytes.clone());
        let changes = plugin.diff(&decl_blob, &decl_blob).unwrap();
        assert!(changes.is_empty(), "identical blobs should produce no changes");
    }

    #[test]
    fn plugin_diff_detects_change() {
        let plugin = ProtobufPlugin;

        let source1 = r#"syntax = "proto3"; message Foo { string a = 1; }"#;
        let source2 = r#"syntax = "proto3"; message Foo { string a = 1; int32 b = 2; }"#;

        let blob1 = plugin.parse(source1).unwrap();
        let blob2 = plugin.parse(source2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = crate::ast::decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Foo");
        let new_decl = get_decl_blob(&blob2, "Foo");
        let changes = plugin.diff(&old_decl, &new_decl).unwrap();
        assert!(!changes.is_empty(), "should detect field addition");
    }

    #[test]
    fn plugin_check_compat_full_add_field_ok() {
        use schemahub_types::{CompatibilityDirection, CompatibilityRules};

        let plugin = ProtobufPlugin;
        let source1 = r#"syntax = "proto3"; message Foo { string a = 1; }"#;
        let source2 = r#"syntax = "proto3"; message Foo { string a = 1; int32 b = 2; }"#;

        let blob1 = plugin.parse(source1).unwrap();
        let blob2 = plugin.parse(source2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = crate::ast::decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Foo");
        let new_decl = get_decl_blob(&blob2, "Foo");

        let rules = CompatibilityRules { direction: CompatibilityDirection::Full };
        let result = plugin.check_compatibility(&old_decl, &new_decl, &rules);
        assert!(result.is_ok(), "adding a field should be FULL compatible: {result:?}");
    }
}
