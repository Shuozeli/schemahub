//! Read / exploration: summarize, detail, imports, type_refs (design.md §2, §9).

use bytes::Bytes;
use protoc_rs_schema::{DescriptorProto, FieldType};
use schemahub_types::{
    DeclBlob, DeclDetail, DeclKind, DeclSummary, Import, MetaBlob, ReadError, TypeRef,
};

use crate::blob::{decode_decl, decode_meta, DeclPayload};

/// Lightweight one-line summary of a declaration.
pub fn summarize(blob: &DeclBlob) -> Result<DeclSummary, ReadError> {
    let p = decode_decl(blob.as_bytes()).map_err(|e| ReadError::MalformedBlob(e.to_string()))?;
    let (name, kind) = match &p {
        DeclPayload::Message(m) => (m.name.clone().unwrap_or_default(), DeclKind::Message),
        DeclPayload::Enum(e) => (e.name.clone().unwrap_or_default(), DeclKind::Enum),
        DeclPayload::Service(s) => (s.name.clone().unwrap_or_default(), DeclKind::Service),
    };
    Ok(DeclSummary {
        name,
        kind,
        doc_comment: String::new(),
    })
}

/// Full detail: the printed declaration text as opaque bytes for the client.
pub fn detail(blob: &DeclBlob) -> Result<DeclDetail, ReadError> {
    let p = decode_decl(blob.as_bytes()).map_err(|e| ReadError::MalformedBlob(e.to_string()))?;
    let text = render_decl(&p);
    Ok(DeclDetail::new(Bytes::from(text.into_bytes())))
}

/// Render a single declaration in isolation (no comment lookup).
fn render_decl(p: &DeclPayload) -> String {
    use protoc_rs_schema::{FileDescriptorProto, SourceCodeInfo};
    let mut file = FileDescriptorProto {
        syntax: Some("proto3".to_string()),
        source_code_info: Some(SourceCodeInfo::default()),
        ..Default::default()
    };
    match p.clone() {
        DeclPayload::Message(m) => file.message_type.push(m),
        DeclPayload::Enum(e) => file.enum_type.push(e),
        DeclPayload::Service(s) => file.service.push(s),
    }
    // Reuse the printer but strip the leading syntax/package preamble.
    let full = crate::printer::print_file(&file);
    full.split_once("\n\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or(full)
}

/// Imports declared in the file's metadata.
pub fn imports(meta: &MetaBlob) -> Result<Vec<Import>, ReadError> {
    let m = decode_meta(meta.as_bytes()).map_err(|e| ReadError::MalformedBlob(e.to_string()))?;
    Ok(m.dependency
        .iter()
        .map(|path| Import {
            path: path.clone(),
            resolved_commit: String::new(),
            decl_name: String::new(),
        })
        .collect())
}

/// Type names referenced by a declaration (for FollowType / rename propagation).
pub fn type_refs(blob: &DeclBlob) -> Result<Vec<TypeRef>, ReadError> {
    let p = decode_decl(blob.as_bytes()).map_err(|e| ReadError::MalformedBlob(e.to_string()))?;
    let mut refs: Vec<String> = Vec::new();
    match &p {
        DeclPayload::Message(m) => collect_message_refs(m, &mut refs),
        DeclPayload::Enum(_) => {}
        DeclPayload::Service(s) => {
            for method in &s.method {
                if let Some(t) = &method.input_type {
                    refs.push(short(t));
                }
                if let Some(t) = &method.output_type {
                    refs.push(short(t));
                }
            }
        }
    }
    refs.sort();
    refs.dedup();
    Ok(refs.into_iter().map(TypeRef::new).collect())
}

fn collect_message_refs(m: &DescriptorProto, out: &mut Vec<String>) {
    // Names of synthetic map-entry nested types: `map<K,V>` fields reference
    // these but they are not real type dependencies.
    let map_entry_names: std::collections::HashSet<&str> = m
        .nested_type
        .iter()
        .filter(|n| {
            n.options
                .as_ref()
                .and_then(|o| o.map_entry)
                .unwrap_or(false)
        })
        .filter_map(|n| n.name.as_deref())
        .collect();

    for f in &m.field {
        // A message/enum-typed field carries a `type_name`. Before the analyzer
        // resolves it, `r#type` may be `None` (parser leaves it unset) while
        // `type_name` is set; after resolution it is Message/Enum/Group. Treat
        // any non-empty `type_name` as a reference, except synthetic map-entry
        // types (handled below via nested_type recursion).
        let is_named_type = matches!(
            f.r#type,
            None | Some(FieldType::Message) | Some(FieldType::Enum) | Some(FieldType::Group)
        );
        if is_named_type {
            if let Some(t) = &f.type_name {
                let s = short(t);
                if !s.is_empty() && !map_entry_names.contains(s.as_str()) {
                    out.push(s);
                }
            }
        }
    }
    // Nested types may reference others too.
    for nested in &m.nested_type {
        collect_message_refs(nested, out);
    }
}

fn short(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_string()
}
