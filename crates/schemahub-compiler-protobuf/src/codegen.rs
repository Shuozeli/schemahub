//! Codegen: assemble a `FileDescriptorSet` from a closure and reuse
//! `protoc-rs-codegen` (design.md §10).

use std::collections::HashMap;

use bytes::Bytes;
use protoc_rs_schema::{
    DescriptorProto, FieldDescriptorProto, FieldType, FileDescriptorProto, FileDescriptorSet,
};
use schemahub_types::{CodegenError, DescriptorError, Language, SchemaClosure};

use crate::codec::Writer;
use crate::reassemble::reassemble;

/// Build a `FileDescriptorSet` from every schema in the closure.
fn build_set(closure: &SchemaClosure) -> Result<FileDescriptorSet, String> {
    // Deterministic order by schema path.
    let mut entries: Vec<_> = closure.entries.iter().collect();
    entries.sort_by(|a, b| {
        (&a.0.project, &a.0.repo, &a.0.schema_name).cmp(&(
            &b.0.project,
            &b.0.repo,
            &b.0.schema_name,
        ))
    });

    let mut set = FileDescriptorSet::default();
    for (path, objects) in entries {
        let mut file = reassemble(objects).map_err(|e| e.to_string())?;
        // Ensure the file has a name for codegen module keys.
        if file.name.is_none() {
            file.name = Some(format!(
                "{}/{}/{}",
                path.project, path.repo, path.schema_name
            ));
        }
        set.file.push(file);
    }
    Ok(set)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SymbolKind {
    Message,
    Enum,
}

/// Resolve parser-level named references across the complete schema closure.
///
/// `protoc-rs-parser` intentionally leaves named references unresolved. The
/// standalone compiler normally runs its analyzer before codegen, but
/// SchemaHub stores parser descriptors independently and reconstructs the
/// import closure here. Resolve against that closure so imported and nested
/// types reach codegen with the same fully-qualified names as analyzed input.
fn resolve_named_types(set: &mut FileDescriptorSet) -> Result<(), String> {
    let mut symbols = HashMap::new();
    for file in &set.file {
        let package = file.package.as_deref().unwrap_or("");
        register_file_symbols(file, package, &mut symbols)?;
    }

    for file in &mut set.file {
        let package = file.package.clone().unwrap_or_default();
        for message in &mut file.message_type {
            let scope = join_name(&package, message.name.as_deref().unwrap_or(""));
            resolve_message_types(message, &scope, &symbols)?;
        }
        for extension in &mut file.extension {
            resolve_field_type(extension, &package, &symbols)?;
            resolve_message_reference(&mut extension.extendee, &package, &symbols, "extendee")?;
        }
        for service in &mut file.service {
            for method in &mut service.method {
                resolve_message_reference(&mut method.input_type, &package, &symbols, "RPC input")?;
                resolve_message_reference(
                    &mut method.output_type,
                    &package,
                    &symbols,
                    "RPC output",
                )?;
            }
        }
    }

    Ok(())
}

fn register_file_symbols(
    file: &FileDescriptorProto,
    package: &str,
    symbols: &mut HashMap<String, SymbolKind>,
) -> Result<(), String> {
    for message in &file.message_type {
        register_message_symbols(message, package, symbols)?;
    }
    for enumeration in &file.enum_type {
        register_symbol(
            symbols,
            join_name(package, enumeration.name.as_deref().unwrap_or("")),
            SymbolKind::Enum,
        )?;
    }
    Ok(())
}

fn register_message_symbols(
    message: &DescriptorProto,
    parent: &str,
    symbols: &mut HashMap<String, SymbolKind>,
) -> Result<(), String> {
    let name = join_name(parent, message.name.as_deref().unwrap_or(""));
    register_symbol(symbols, name.clone(), SymbolKind::Message)?;

    for nested in &message.nested_type {
        register_message_symbols(nested, &name, symbols)?;
    }
    for enumeration in &message.enum_type {
        register_symbol(
            symbols,
            join_name(&name, enumeration.name.as_deref().unwrap_or("")),
            SymbolKind::Enum,
        )?;
    }
    Ok(())
}

fn register_symbol(
    symbols: &mut HashMap<String, SymbolKind>,
    name: String,
    kind: SymbolKind,
) -> Result<(), String> {
    if name.is_empty() {
        return Err("protobuf declaration is missing a name".to_string());
    }
    if symbols.insert(name.clone(), kind).is_some() {
        return Err(format!("duplicate protobuf symbol: .{name}"));
    }
    Ok(())
}

fn resolve_message_types(
    message: &mut DescriptorProto,
    scope: &str,
    symbols: &HashMap<String, SymbolKind>,
) -> Result<(), String> {
    for field in &mut message.field {
        resolve_field_type(field, scope, symbols)?;
    }
    for extension in &mut message.extension {
        resolve_field_type(extension, scope, symbols)?;
        resolve_message_reference(&mut extension.extendee, scope, symbols, "extendee")?;
    }
    for nested in &mut message.nested_type {
        let nested_scope = join_name(scope, nested.name.as_deref().unwrap_or(""));
        resolve_message_types(nested, &nested_scope, symbols)?;
    }
    Ok(())
}

fn resolve_field_type(
    field: &mut FieldDescriptorProto,
    scope: &str,
    symbols: &HashMap<String, SymbolKind>,
) -> Result<(), String> {
    let Some(type_name) = field.type_name.as_deref() else {
        return Ok(());
    };
    let (resolved, kind) = resolve_symbol(type_name, scope, symbols)
        .ok_or_else(|| format!("unresolved protobuf type `{type_name}` from scope `.{scope}`"))?;

    field.type_name = Some(format!(".{resolved}"));
    field.r#type = Some(match (field.r#type, kind) {
        (Some(FieldType::Group), SymbolKind::Message) => FieldType::Group,
        (_, SymbolKind::Message) => FieldType::Message,
        (_, SymbolKind::Enum) => FieldType::Enum,
    });
    Ok(())
}

fn resolve_message_reference(
    reference: &mut Option<String>,
    scope: &str,
    symbols: &HashMap<String, SymbolKind>,
    role: &str,
) -> Result<(), String> {
    let Some(name) = reference.as_deref() else {
        return Ok(());
    };
    let (resolved, kind) = resolve_symbol(name, scope, symbols)
        .ok_or_else(|| format!("unresolved protobuf {role} type `{name}` from scope `.{scope}`"))?;
    if kind != SymbolKind::Message {
        return Err(format!(
            "protobuf {role} type `{name}` resolves to an enum, not a message"
        ));
    }
    *reference = Some(format!(".{resolved}"));
    Ok(())
}

fn resolve_symbol(
    name: &str,
    scope: &str,
    symbols: &HashMap<String, SymbolKind>,
) -> Option<(String, SymbolKind)> {
    if let Some(absolute) = name.strip_prefix('.') {
        return symbols
            .get(absolute)
            .copied()
            .map(|kind| (absolute.to_string(), kind));
    }

    let mut candidate_scope = scope;
    loop {
        let candidate = join_name(candidate_scope, name);
        if let Some(kind) = symbols.get(&candidate) {
            return Some((candidate, *kind));
        }
        let Some((parent, _)) = candidate_scope.rsplit_once('.') else {
            break;
        };
        candidate_scope = parent;
    }

    symbols
        .get(name)
        .copied()
        .map(|kind| (name.to_string(), kind))
}

fn join_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else if name.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Serialize the `FileDescriptorSet` to opaque, versioned bytes.
///
/// `FileDescriptorSet` is not a `prost::Message` in `protoc-rs-schema`, so we
/// serialize each contained file with this crate's stable codec rather than
/// protobuf wire format. These bytes are opaque to the VCS layer.
pub fn generate_descriptors(closure: &SchemaClosure) -> Result<Bytes, DescriptorError> {
    let set = build_set(closure).map_err(DescriptorError::Other)?;
    let mut w = Writer::new();
    w.write_u8(crate::blob::BLOB_VERSION);
    w.write_uvarint(set.file.len() as u64);
    for file in &set.file {
        // Reuse the parse split to encode each file deterministically as
        // meta + decls, concatenated.
        let parsed = crate::parse::split_file(file.clone());
        let meta = parsed.meta.into_bytes();
        w.write_bytes(&meta);
        w.write_uvarint(parsed.decls.len() as u64);
        for (name, blob) in parsed.decls {
            w.write_str(&name);
            w.write_bytes(blob.as_bytes());
        }
    }
    Ok(Bytes::from(w.into_bytes()))
}

/// Generate source code for `lang` over the closure.
pub fn generate_code(closure: &SchemaClosure, lang: Language) -> Result<String, CodegenError> {
    match lang {
        Language::Rust => {
            let mut set = build_set(closure).map_err(CodegenError::Other)?;
            resolve_named_types(&mut set).map_err(CodegenError::Other)?;
            let modules = protoc_rs_codegen::generate_rust(&set)
                .map_err(|e| CodegenError::Other(e.to_string()))?;
            // Concatenate modules deterministically by key.
            let mut keys: Vec<_> = modules.keys().cloned().collect();
            keys.sort();
            let mut out = String::new();
            for k in keys {
                out.push_str(&format!("// ===== module {k} =====\n"));
                out.push_str(&modules[&k]);
                out.push('\n');
            }
            Ok(out)
        }
        other => Err(CodegenError::UnsupportedLanguage(other)),
    }
}
