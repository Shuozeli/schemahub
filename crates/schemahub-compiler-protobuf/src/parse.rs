//! `parse(source)` → split a `FileDescriptorProto` into a `ParsedSchema`
//! (design.md §3.1 decl-split).
//!
//! One `DeclBlob` per top-level `message_type` / `enum_type` / `service`;
//! one `MetaBlob` for the file (package, deps, syntax/edition, file options,
//! and the full `source_code_info`). Nested types stay inside their parent
//! descriptor and are *not* emitted as separate decls.

use std::collections::HashSet;

use protoc_rs_schema::{DescriptorProto, FieldType, FileDescriptorProto};
use schemahub_types::{DeclBlob, MetaBlob, ParseError, ParsedSchema};

use crate::blob::{encode_decl, encode_meta, DeclPayload, MetaPayload};

/// File-level `source_code_info` field numbers (descriptor.proto).
pub const FILE_MESSAGE_TYPE: i32 = 4;
pub const FILE_ENUM_TYPE: i32 = 5;
pub const FILE_SERVICE: i32 = 6;

/// Parse `.proto` source into per-declaration objects.
pub fn parse_source(source: &str) -> Result<ParsedSchema, ParseError> {
    let result = protoc_rs_parser::parse_collecting(source).map_err(map_parse_error)?;
    if let Some(err) = result.errors.into_iter().next() {
        return Err(map_parse_error(err));
    }
    Ok(split_file(result.file))
}

/// Split a parsed `FileDescriptorProto` into a `ParsedSchema`.
pub fn split_file(mut file: FileDescriptorProto) -> ParsedSchema {
    // The parser leaves a field's `r#type` unset for named (message/enum/group)
    // references; resolution normally happens in the analyzer, which this
    // pipeline does not run. Without a `FieldType`, downstream wire-type
    // classification (typecompat) cannot tell an enum field (Varint) from a
    // message field (length-delimited). Resolve the kind locally against the
    // file's own declarations so the stored blob carries a correct `FieldType`.
    resolve_local_field_types(&mut file);

    let mut decls: Vec<(String, DeclBlob)> = Vec::new();

    for msg in &file.message_type {
        let name = msg.name.clone().unwrap_or_default();
        let blob = encode_decl(&DeclPayload::Message(msg.clone()));
        decls.push((name, DeclBlob::new(blob)));
    }
    for en in &file.enum_type {
        let name = en.name.clone().unwrap_or_default();
        let blob = encode_decl(&DeclPayload::Enum(en.clone()));
        decls.push((name, DeclBlob::new(blob)));
    }
    for svc in &file.service {
        let name = svc.name.clone().unwrap_or_default();
        let blob = encode_decl(&DeclPayload::Service(svc.clone()));
        decls.push((name, DeclBlob::new(blob)));
    }

    let message_order: Vec<String> = file
        .message_type
        .iter()
        .map(|m| m.name.clone().unwrap_or_default())
        .collect();
    let enum_order: Vec<String> = file
        .enum_type
        .iter()
        .map(|e| e.name.clone().unwrap_or_default())
        .collect();
    let service_order: Vec<String> = file
        .service
        .iter()
        .map(|s| s.name.clone().unwrap_or_default())
        .collect();

    let meta = MetaPayload {
        name: file.name,
        package: file.package,
        dependency: file.dependency,
        public_dependency: file.public_dependency,
        weak_dependency: file.weak_dependency,
        option_dependency: file.option_dependency,
        options: file.options,
        syntax: file.syntax,
        edition: file.edition,
        source_code_info: file.source_code_info,
        message_order,
        enum_order,
        service_order,
        extension: file.extension,
    };

    ParsedSchema {
        meta: MetaBlob::new(encode_meta(&meta)),
        decls,
    }
}

/// Set each unresolved field's `r#type` to `Enum` or `Message` by matching its
/// written `type_name` against the file's own enum/message declarations
/// (top-level and nested). References to types declared in other files (e.g.
/// `google.protobuf.Timestamp`) cannot be resolved here and are left as-is; the
/// conservative length-delimited default still applies to those.
fn resolve_local_field_types(file: &mut FileDescriptorProto) {
    let mut enum_names: HashSet<String> = HashSet::new();
    let mut message_names: HashSet<String> = HashSet::new();
    for e in &file.enum_type {
        if let Some(n) = &e.name {
            enum_names.insert(n.clone());
        }
    }
    for m in &file.message_type {
        collect_decl_names(m, &mut enum_names, &mut message_names);
    }
    for m in &mut file.message_type {
        resolve_message_fields(m, &enum_names, &message_names);
    }
}

/// Recursively collect message/enum names declared inside `msg` (and `msg`
/// itself) by their trailing simple name.
fn collect_decl_names(
    msg: &DescriptorProto,
    enum_names: &mut HashSet<String>,
    message_names: &mut HashSet<String>,
) {
    if let Some(n) = &msg.name {
        message_names.insert(n.clone());
    }
    for e in &msg.enum_type {
        if let Some(n) = &e.name {
            enum_names.insert(n.clone());
        }
    }
    for nested in &msg.nested_type {
        collect_decl_names(nested, enum_names, message_names);
    }
}

/// Resolve unresolved field types within `msg` and its nested messages.
fn resolve_message_fields(
    msg: &mut DescriptorProto,
    enum_names: &HashSet<String>,
    message_names: &HashSet<String>,
) {
    for f in &mut msg.field {
        if f.r#type.is_some() {
            continue;
        }
        let Some(type_name) = f.type_name.as_deref() else {
            continue;
        };
        // The written reference may be qualified (`pkg.Outer.Color`); match on
        // its trailing simple name against the file's local declarations.
        let simple = type_name.rsplit('.').next().unwrap_or(type_name);
        if enum_names.contains(simple) {
            f.r#type = Some(FieldType::Enum);
        } else if message_names.contains(simple) {
            f.r#type = Some(FieldType::Message);
        }
    }
    for nested in &mut msg.nested_type {
        resolve_message_fields(nested, enum_names, message_names);
    }
}

fn map_parse_error(e: protoc_rs_parser::ParseError) -> ParseError {
    ParseError::SyntaxError {
        line: e.span.start.line,
        message: e.message,
    }
}
