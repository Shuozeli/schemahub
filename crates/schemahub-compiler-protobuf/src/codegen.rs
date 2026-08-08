//! Codegen: assemble a `FileDescriptorSet` from a closure and reuse
//! `protoc-rs-codegen` (design.md §10).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bytes::Bytes;
use heck::ToSnakeCase;
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

fn requested_root_package(closure: &SchemaClosure) -> Result<String, String> {
    let root = match closure.root.as_ref() {
        Some(root) => root,
        None if closure.entries.len() == 1 => closure
            .entries
            .keys()
            .next()
            .expect("one-entry closure has one root"),
        None if closure.entries.is_empty() => {
            return Err("protobuf codegen closure is empty".to_string());
        }
        None => {
            return Err("multi-file Protobuf codegen requires an explicit root schema".to_string());
        }
    };
    let objects = closure.entries.get(root).ok_or_else(|| {
        format!("protobuf codegen root {root} is not present in its schema closure")
    })?;
    let file = reassemble(objects).map_err(|error| error.to_string())?;
    Ok(file.package.unwrap_or_default())
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

fn register_symbol_packages(
    file: &FileDescriptorProto,
    package: &str,
    packages: &mut HashMap<String, String>,
) {
    for message in &file.message_type {
        register_message_symbol_packages(message, package, package, packages);
    }
    for enumeration in &file.enum_type {
        packages.insert(
            join_name(package, enumeration.name.as_deref().unwrap_or("")),
            package.to_string(),
        );
    }
}

fn register_message_symbol_packages(
    message: &DescriptorProto,
    parent: &str,
    package: &str,
    packages: &mut HashMap<String, String>,
) {
    let name = join_name(parent, message.name.as_deref().unwrap_or(""));
    packages.insert(name.clone(), package.to_string());
    for nested in &message.nested_type {
        register_message_symbol_packages(nested, &name, package, packages);
    }
    for enumeration in &message.enum_type {
        packages.insert(
            join_name(&name, enumeration.name.as_deref().unwrap_or("")),
            package.to_string(),
        );
    }
}

fn collect_cross_package_imports(
    set: &FileDescriptorSet,
) -> Result<HashMap<String, BTreeSet<String>>, String> {
    let mut symbol_packages = HashMap::new();
    for file in &set.file {
        let package = file.package.as_deref().unwrap_or("");
        register_symbol_packages(file, package, &mut symbol_packages);
    }

    let mut imports = HashMap::new();
    for file in &set.file {
        let package = file.package.as_deref().unwrap_or("");
        for message in &file.message_type {
            collect_message_imports(message, package, &symbol_packages, &mut imports)?;
        }
        for extension in &file.extension {
            collect_field_imports(extension, package, &symbol_packages, &mut imports)?;
        }
        for service in &file.service {
            for method in &service.method {
                collect_reference_import(
                    method.input_type.as_deref(),
                    package,
                    &symbol_packages,
                    &mut imports,
                )?;
                collect_reference_import(
                    method.output_type.as_deref(),
                    package,
                    &symbol_packages,
                    &mut imports,
                )?;
            }
        }
    }
    Ok(imports)
}

fn collect_message_imports(
    message: &DescriptorProto,
    package: &str,
    symbol_packages: &HashMap<String, String>,
    imports: &mut HashMap<String, BTreeSet<String>>,
) -> Result<(), String> {
    for field in &message.field {
        collect_field_imports(field, package, symbol_packages, imports)?;
    }
    for extension in &message.extension {
        collect_field_imports(extension, package, symbol_packages, imports)?;
    }
    for nested in &message.nested_type {
        collect_message_imports(nested, package, symbol_packages, imports)?;
    }
    Ok(())
}

fn collect_field_imports(
    field: &FieldDescriptorProto,
    package: &str,
    symbol_packages: &HashMap<String, String>,
    imports: &mut HashMap<String, BTreeSet<String>>,
) -> Result<(), String> {
    collect_reference_import(
        field.type_name.as_deref(),
        package,
        symbol_packages,
        imports,
    )?;
    collect_reference_import(field.extendee.as_deref(), package, symbol_packages, imports)
}

fn collect_reference_import(
    reference: Option<&str>,
    package: &str,
    symbol_packages: &HashMap<String, String>,
    imports: &mut HashMap<String, BTreeSet<String>>,
) -> Result<(), String> {
    let Some(reference) = reference else {
        return Ok(());
    };
    let normalized = reference.strip_prefix('.').unwrap_or(reference);
    let target_package = symbol_packages.get(normalized).ok_or_else(|| {
        format!("resolved protobuf reference `.{normalized}` has no defining package")
    })?;
    if package.is_empty() || target_package == package {
        return Ok(());
    }

    let rust_path = protoc_rs_codegen::rust_gen::fqn_to_rust_path(reference, package);
    let import = rust_path.split("::").next().unwrap_or("");
    if import.is_empty() || matches!(import, "crate" | "self" | "super") {
        return Err(format!(
            "cross-package protobuf reference `.{normalized}` produced invalid Rust path `{rust_path}`"
        ));
    }
    imports
        .entry(package.to_string())
        .or_default()
        .insert(import.to_string());
    Ok(())
}

#[derive(Default)]
struct RustModuleNode {
    source: String,
    imports: BTreeSet<String>,
    children: BTreeMap<String, RustModuleNode>,
}

fn render_rust_bundle(
    set: &FileDescriptorSet,
    modules: &HashMap<String, String>,
    root_package: &str,
) -> Result<String, String> {
    let cross_package_imports = collect_cross_package_imports(set)?;
    let mut package_by_key = HashMap::new();
    for file in &set.file {
        let package = file.package.as_deref().unwrap_or("");
        let key = rust_module_key(file);
        if let Some(previous) = package_by_key.insert(key.clone(), package.to_string()) {
            if previous != package {
                return Err(format!(
                    "protobuf codegen module `{key}` represents both `{previous}` and `{package}`"
                ));
            }
        }
    }

    let mut root = RustModuleNode::default();
    let mut rust_paths: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut keys: Vec<_> = modules.keys().cloned().collect();
    keys.sort();
    for key in keys {
        let source = &modules[&key];
        let package = package_by_key
            .get(&key)
            .ok_or_else(|| format!("protobuf codegen returned unknown module `{key}`"))?;
        let labelled_source = format!("// ===== module {key} =====\n{source}\n");
        if package.is_empty() {
            root.source.push_str(&labelled_source);
            continue;
        }

        let path = package
            .split('.')
            .map(rust_module_segment)
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(previous) = rust_paths.insert(path.clone(), package.clone()) {
            if previous != *package {
                return Err(format!(
                    "protobuf packages `{previous}` and `{package}` map to the same Rust module"
                ));
            }
        }
        let mut node = &mut root;
        for segment in &path {
            node = node.children.entry(segment.clone()).or_default();
        }
        if !node.source.is_empty() {
            return Err(format!(
                "protobuf package `{package}` produced more than one generated module"
            ));
        }
        node.source = labelled_source;
        node.imports = cross_package_imports
            .get(package)
            .cloned()
            .unwrap_or_default();
    }

    let mut output = root.source;
    for (name, node) in &root.children {
        render_rust_module(name, node, 0, &mut output);
    }
    if !root_package.is_empty() {
        let root_path = root_package
            .split('.')
            .map(rust_module_segment)
            .collect::<Result<Vec<_>, _>>()?;
        if !rust_paths.contains_key(&root_path) {
            return Err(format!(
                "requested Protobuf root package `{root_package}` produced no Rust module"
            ));
        }
        output.push_str(&format!("pub use {}::*;\n", root_path.join("::")));
    }
    Ok(output)
}

fn rust_module_key(file: &FileDescriptorProto) -> String {
    let package = file.package.as_deref().unwrap_or("");
    let stem = if package.is_empty() {
        file.name
            .as_deref()
            .unwrap_or("unknown")
            .trim_end_matches(".proto")
            .replace('/', ".")
    } else {
        package.to_string()
    };
    format!("{stem}.rs")
}

fn rust_module_segment(segment: &str) -> Result<String, String> {
    let segment = segment.to_snake_case();
    if segment.is_empty() {
        return Err("protobuf package contains an empty Rust module segment".to_string());
    }
    Ok(match segment.as_str() {
        "self" | "super" => format!("{segment}_"),
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false"
        | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move"
        | "mut" | "pub" | "ref" | "return" | "static" | "struct" | "trait" | "true" | "type"
        | "unsafe" | "use" | "where" | "while" | "async" | "await" | "dyn" | "abstract"
        | "become" | "box" | "do" | "final" | "macro" | "override" | "priv" | "typeof"
        | "unsized" | "virtual" | "yield" | "try" => {
            format!("r#{segment}")
        }
        _ => segment,
    })
}

fn render_rust_module(name: &str, node: &RustModuleNode, depth: usize, output: &mut String) {
    let indent = "    ".repeat(depth);
    output.push_str(&format!("{indent}pub mod {name} {{\n"));
    let body_indent = "    ".repeat(depth + 1);
    for import in &node.imports {
        let container = vec!["super"; depth + 1].join("::");
        output.push_str(&format!("{body_indent}use {container}::{import};\n"));
    }
    if !node.imports.is_empty() && (!node.source.is_empty() || !node.children.is_empty()) {
        output.push('\n');
    }
    for line in node.source.lines() {
        output.push_str(&body_indent);
        output.push_str(line);
        output.push('\n');
    }
    for (child_name, child) in &node.children {
        render_rust_module(child_name, child, depth + 1, output);
    }
    output.push_str(&format!("{indent}}}\n"));
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
            let root_package = requested_root_package(closure).map_err(CodegenError::Other)?;
            let mut set = build_set(closure).map_err(CodegenError::Other)?;
            resolve_named_types(&mut set).map_err(CodegenError::Other)?;
            let modules = protoc_rs_codegen::generate_rust(&set)
                .map_err(|e| CodegenError::Other(e.to_string()))?;
            render_rust_bundle(&set, &modules, &root_package).map_err(CodegenError::Other)
        }
        other => Err(CodegenError::UnsupportedLanguage(other)),
    }
}
