//! Codegen: reassemble the import closure into descriptors / generated code
//! (design.md §3.2, §10), reusing `flatc-rs-codegen`.
//!
//! `flatc-rs-codegen` consumes a `ResolvedSchema` — a schema whose type
//! references are resolved to indices. The sibling parser produces an
//! *unresolved* `Schema` (the analyzer in `flatc-rs-compiler` resolves it, but
//! that crate is not a dependency). So this module performs a lightweight
//! resolution pass sufficient for tables/structs/enums/unions/services with
//! scalar and user-defined references, then delegates to the sibling codegen.

use bytes::Bytes;

use flatc_rs_codegen::{
    generate_dart, generate_rust, generate_typescript, CodeGenOptions, DartCodeGenOptions,
    TsCodeGenOptions,
};
use flatc_rs_schema::resolved::ResolvedSchema;
use flatc_rs_schema::{BaseType, Enum, Object, Schema, Service};
use schemahub_types::{CodegenError, DescriptorError, Language, SchemaClosure, SchemaObjects};

use crate::blob::{decode_decl, decode_meta, DeclPayload};
use crate::printer::{print_decl, print_meta_header, print_root_type};

/// Reassemble the transitive closure into a single reconstructed `.fbs` bundle.
///
/// For FlatBuffers the "descriptor" artifact is the canonical source bundle:
/// each schema file's metadata header, its declarations, then its `root_type`.
pub fn generate_descriptors(closure: &SchemaClosure) -> Result<Bytes, DescriptorError> {
    // Deterministic order across files by full path.
    let mut paths: Vec<_> = closure.entries.keys().cloned().collect();
    paths.sort();

    let mut bundle = String::new();
    for path in paths {
        let schema = &closure.entries[&path];
        bundle.push_str(&format!("// ── {path} ──\n"));
        bundle.push_str(
            &render_source(schema).map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?,
        );
        bundle.push('\n');
    }
    Ok(Bytes::from(bundle.into_bytes()))
}

/// Generate code for a language by resolving the closure's primary schema and
/// delegating to `flatc-rs-codegen`.
pub fn generate_code(closure: &SchemaClosure, lang: Language) -> Result<String, CodegenError> {
    match lang {
        Language::Rust | Language::TypeScript => {} // dart handled below too
        Language::Go | Language::Python | Language::Java => {
            return Err(CodegenError::UnsupportedLanguage(lang));
        }
    }

    // Reassemble every closure entry into one combined Schema so cross-file type
    // references resolve. (Imports are pinned by the VCS layer before this call.)
    let mut combined = Schema::default();
    let mut paths: Vec<_> = closure.entries.keys().cloned().collect();
    paths.sort();
    for path in &paths {
        let schema = &closure.entries[path];
        let (objects, enums, services, _meta) =
            reassemble(schema).map_err(|e| CodegenError::MalformedBlob(e.to_string()))?;
        combined.objects.extend(objects);
        combined.enums.extend(enums);
        combined.services.extend(services);
    }

    resolve_indices(&mut combined);

    let resolved = ResolvedSchema::try_from_parsed(&combined).map_err(|e| {
        CodegenError::Other(format!(
            "schema could not be resolved for codegen: {e}; full layout resolution \
             (struct byte_size/min_align, root-type binding) is performed by the \
             flatc-rs-compiler analyzer, which is not wired into this crate"
        ))
    })?;

    match lang {
        Language::Rust => generate_rust(&resolved, &CodeGenOptions::default())
            .map_err(|e| CodegenError::Other(e.to_string())),
        Language::TypeScript => generate_typescript(&resolved, &TsCodeGenOptions::default())
            .map_err(|e| CodegenError::Other(e.to_string())),
        // Dart is supported by the sibling but not exposed in `Language`; reachable
        // only if `Language` gains a Dart variant. Kept for completeness.
        _ => generate_dart(&resolved, &DartCodeGenOptions::default())
            .map_err(|e| CodegenError::Other(e.to_string())),
    }
}

/// Render one schema file's canonical source (header + decls + root_type).
fn render_source(schema: &SchemaObjects) -> Result<String, crate::blob::BlobError> {
    let meta = decode_meta(&schema.meta)?;
    let mut out = print_meta_header(&meta);
    for blob in schema.decls.values() {
        let payload = decode_decl(blob)?;
        out.push_str(&print_decl(&payload));
        out.push('\n');
    }
    out.push_str(&print_root_type(&meta));
    Ok(out)
}

/// Reassemble a `SchemaObjects` into the typed AST collections.
#[allow(clippy::type_complexity)]
fn reassemble(
    schema: &SchemaObjects,
) -> Result<(Vec<Object>, Vec<Enum>, Vec<Service>, crate::blob::FbsMeta), crate::blob::BlobError> {
    let meta = decode_meta(&schema.meta)?;
    let mut objects = Vec::new();
    let mut enums = Vec::new();
    let mut services = Vec::new();
    for blob in schema.decls.values() {
        match decode_decl(blob)? {
            DeclPayload::Object(o) => objects.push(*o),
            DeclPayload::Enum(e) => enums.push(*e),
            DeclPayload::Service(s) => services.push(*s),
        }
    }
    Ok((objects, enums, services, meta))
}

/// Lightweight resolution pass: populate the `index` fields on user-defined
/// type references and the `request_index`/`response_index` on rpc calls, so
/// `flatc-rs-codegen` (which indexes into `objects`/`enums`) can run.
///
/// Object-typed fields index into `objects`; enum/union-typed references index
/// into `enums`. Names are matched on their short (or fully-qualified) form.
fn resolve_indices(schema: &mut Schema) {
    use std::collections::HashMap;

    let obj_index: HashMap<String, i32> = schema
        .objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| o.name.clone().map(|n| (n, i as i32)))
        .collect();
    let enum_index: HashMap<String, i32> = schema
        .enums
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.name.clone().map(|n| (n, i as i32)))
        .collect();

    let resolve_name = |name: &str| -> Option<(i32, bool)> {
        // (index, is_enum)
        if let Some(idx) = enum_index.get(name) {
            return Some((*idx, true));
        }
        if let Some(idx) = obj_index.get(name) {
            return Some((*idx, false));
        }
        // Try short name match (strip namespace).
        let short = name.rsplit('.').next().unwrap_or(name);
        if let Some(idx) = enum_index.get(short) {
            return Some((*idx, true));
        }
        obj_index.get(short).map(|idx| (*idx, false))
    };

    for obj in &mut schema.objects {
        // Table fields need a wire `id` (slot) for codegen; the parser only
        // assigns ids when given explicitly. Fill the gaps positionally.
        if !obj.is_struct {
            for (i, field) in obj.fields.iter_mut().enumerate() {
                if field.id.is_none() {
                    field.id = Some(i as u32);
                }
            }
        }
        for field in &mut obj.fields {
            if let Some(ty) = &mut field.type_ {
                if let Some(name) = ty.unresolved_name.clone() {
                    if let Some((idx, is_enum)) = resolve_name(&name) {
                        ty.index = Some(idx);
                        // For a vector/array of user types the base_type stays
                        // VECTOR/ARRAY; otherwise mark enum-typed fields.
                        if is_enum
                            && matches!(
                                ty.base_type,
                                Some(BaseType::BASE_TYPE_TABLE) | Some(BaseType::BASE_TYPE_STRUCT)
                            )
                        {
                            // enum-typed scalar field: keep base_type as is; index points to enums.
                        }
                    }
                }
            }
        }
    }

    for en in &mut schema.enums {
        for val in &mut en.values {
            if let Some(ty) = &mut val.union_type {
                if let Some(name) = ty.unresolved_name.clone() {
                    if let Some((idx, _)) = resolve_name(&name) {
                        ty.index = Some(idx);
                    }
                }
            }
        }
    }

    for svc in &mut schema.services {
        for call in &mut svc.calls {
            if let Some(req) = &call.request {
                if let Some(name) = &req.name {
                    if let Some((idx, _)) = resolve_name(name) {
                        call.request_index = Some(idx);
                    }
                }
            }
            if let Some(resp) = &call.response {
                if let Some(name) = &resp.name {
                    if let Some((idx, _)) = resolve_name(name) {
                        call.response_index = Some(idx);
                    }
                }
            }
        }
    }
}
