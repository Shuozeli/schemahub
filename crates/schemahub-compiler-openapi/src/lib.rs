//! `schemahub-compiler-openapi` — the OpenAPI 3.1 compiler, in-tree (no sibling
//! compiler exists). Keeps the in-tree AST/parser/printer (`docs/openapi-ast.md`)
//! and the v1 whole-document + granular mutation surface, adapted to the v2
//! `Compiler` trait + per-declaration split (design.md §2, §3.3,
//! crate-structure.md §3.5).
//!
//! ## Per-declaration key scheme
//! `parse` produces a [`schemahub_types::ParsedSchema`] whose `meta` holds the
//! document metadata (openapi version, info, servers, extensions) and whose
//! `decls` are keyed by the stable path model from `openapi-ast.md` §3:
//! `path:<pattern>`, `schema:<name>`, `param:<name>`, `response:<name>`,
//! `requestBody:<name>`. Each decl blob is a self-describing [`ast::OpenApiDecl`]
//! (kind-tagged) so single-blob methods work without the key.
//!
//! ## Status
//! - P0 AST + split + round-trip printer: done.
//! - P1 read/explore + diff: done (`imports(meta)` returns empty — external
//!   `$ref` imports are v2-modeled).
//! - P2 compatibility: done (field-level rule table in `compat.rs`).
//! - P3 mutations: whole-document `PushDocument` + the v1 granular ops
//!   (AddPath/RemovePath/AddOperation/RemoveOperation/AddComponentSchema/
//!   RemoveComponentSchema). Other granular ops `UnsupportedInV1`.
//! - P4 conflicts + descriptors: `render_conflict`/`validate_resolution` done;
//!   `generate_descriptors` assembles resolved YAML; `generate_code`
//!   `UnsupportedLanguage`.

pub mod ast;
pub mod blob;
pub mod compat;
pub mod diff;
pub mod operations;
pub mod parser;
pub mod printer;

use std::collections::BTreeMap;

use bytes::Bytes;

use schemahub_types::{
    CodegenError, CompatibilityRules, CompatibilityViolation, Compiler, ConflictError,
    ConflictSides, DeclBlob, DeclChange, DeclDetail, DeclKind, DeclSummary, DescriptorError,
    DiffError, Import, Language, MetaBlob, Mutation, MutationEffect, MutationError, ParseError,
    ParsedSchema, PrintError, ReadError, SchemaClosure, SchemaObjects, TypeRef,
};

use crate::ast::{
    ComponentSchemaBlob, DeclPayload, JsonSchemaDef, JsonSchemaType, OpenApiDecl, OperationDef,
    PathItemBlob, SchemaOrRef,
};
use crate::blob::{decode_decl, decode_meta, encode_decl};
use crate::operations::OpenApiOp;
use crate::parser::parse_openapi;
use crate::printer::{print_decl_detail, print_schema_objects};

/// The OpenAPI compiler front-end.
#[derive(Debug, Default, Clone)]
pub struct OpenApiCompiler;

impl OpenApiCompiler {
    pub fn new() -> Self {
        Self
    }
}

impl Compiler for OpenApiCompiler {
    fn format_id(&self) -> &'static str {
        "openapi"
    }

    // ── parse / print ───────────────────────────────────────────────────────────

    fn parse(&self, source: &str) -> Result<ParsedSchema, ParseError> {
        parse_openapi(source)
    }

    fn print(&self, schema: &SchemaObjects) -> Result<String, PrintError> {
        print_schema_objects(schema)
    }

    // ── diff ──────────────────────────────────────────────────────────────────

    fn diff_decl(&self, old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError> {
        diff::diff_decl(old, new)
    }

    // ── mutations ───────────────────────────────────────────────────────────────

    fn apply_mutation(
        &self,
        schema: &SchemaObjects,
        op: &Mutation,
    ) -> Result<MutationEffect, MutationError> {
        let decoded = OpenApiOp::decode(&op.operation)?;
        apply_one(schema, &decoded)
    }

    fn apply_mutations(
        &self,
        schema: &SchemaObjects,
        ops: &[Mutation],
    ) -> Result<MutationEffect, MutationError> {
        // Fold the ops over a working copy, then return the net effect. Only the
        // final state is validated (the trait's transaction contract).
        let mut working = schema.clone();
        let mut net_meta: Option<MetaBlob> = None;
        let mut net_upserts: BTreeMap<String, DeclBlob> = BTreeMap::new();
        let mut net_removes: Vec<String> = Vec::new();

        for op in ops {
            let decoded = OpenApiOp::decode(&op.operation)?;
            let effect = apply_one(&working, &decoded)?;

            if let Some(meta) = effect.meta {
                working.meta = meta.clone();
                net_meta = Some(meta);
            }
            for (name, blob) in effect.upserts {
                working.decls.insert(name.clone(), blob.clone());
                net_removes.retain(|r| r != &name);
                net_upserts.insert(name, blob);
            }
            for name in effect.removes {
                working.decls.remove(&name);
                net_upserts.remove(&name);
                if !net_removes.contains(&name) {
                    net_removes.push(name);
                }
            }
        }

        Ok(MutationEffect {
            meta: net_meta,
            upserts: net_upserts.into_iter().collect(),
            removes: net_removes,
        })
    }

    // ── compatibility ───────────────────────────────────────────────────────────

    fn check_compatibility(
        &self,
        old: &DeclBlob,
        new: &DeclBlob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>> {
        compat::check_compatibility(old, new, rules)
    }

    // ── conflicts ─────────────────────────────────────────────────────────────

    fn render_conflict(&self, sides: &ConflictSides) -> Result<String, ConflictError> {
        if sides.sides.is_empty() {
            return Err(ConflictError::EmptyConflict);
        }

        let mut out = String::new();
        if let Some(base) = &sides.base {
            out.push_str("# ===== base =====\n");
            out.push_str(&render_side(base)?);
        }
        for (i, side) in sides.sides.iter().enumerate() {
            out.push_str(&format!("# ===== side {i} =====\n"));
            out.push_str(&render_side(side)?);
        }
        Ok(out)
    }

    fn validate_resolution(&self, resolved: &DeclBlob) -> Result<(), ConflictError> {
        // A valid resolution must decode to a well-formed decl payload.
        decode_decl(resolved)
            .map(|_| ())
            .map_err(|e| ConflictError::InvalidResolution(e.0))
    }

    // ── read / exploration ──────────────────────────────────────────────────────

    fn summarize_decl(&self, blob: &DeclBlob) -> Result<DeclSummary, ReadError> {
        let decl = decode_decl(blob)?;
        let (name, kind, doc) = match &decl.kind {
            DeclPayload::PathItem(b) => (
                format!("path:{}", b.path_pattern),
                DeclKind::PathItem,
                b.summary.clone().unwrap_or_default(),
            ),
            DeclPayload::ComponentSchema(b) => (
                format!("schema:{}", b.name),
                DeclKind::ComponentSchema,
                b.schema
                    .as_ref()
                    .and_then(|s| s.description.clone())
                    .unwrap_or_default(),
            ),
            DeclPayload::ComponentParameter(b) => (
                format!("param:{}", b.name),
                DeclKind::ComponentParameter,
                b.parameter
                    .as_ref()
                    .and_then(|p| p.description.clone())
                    .unwrap_or_default(),
            ),
            DeclPayload::ComponentResponse(b) => (
                format!("response:{}", b.name),
                DeclKind::ComponentResponse,
                b.response
                    .as_ref()
                    .map(|r| r.description.clone())
                    .unwrap_or_default(),
            ),
            DeclPayload::ComponentRequestBody(b) => (
                format!("requestBody:{}", b.name),
                DeclKind::ComponentRequestBody,
                b.request_body
                    .as_ref()
                    .and_then(|r| r.description.clone())
                    .unwrap_or_default(),
            ),
        };
        Ok(DeclSummary {
            name,
            kind,
            doc_comment: doc,
        })
    }

    fn decl_detail(&self, blob: &DeclBlob) -> Result<DeclDetail, ReadError> {
        let decl = decode_decl(blob)?;
        let text = print_decl_detail(&decl.kind);
        Ok(DeclDetail::new(text))
    }

    fn imports(&self, meta: &MetaBlob) -> Result<Vec<Import>, ReadError> {
        // External cross-file `$ref` imports are modeled in the AST
        // (`SchemaRef.external_import`) but live inside decl blobs, not the meta
        // blob. The trait scopes `imports` to the meta blob; for OpenAPI v1 there
        // are no document-level imports, so this is empty. (BFS closure for
        // `generate_descriptors` resolves local refs directly.)
        let _ = decode_meta(meta).map_err(ReadError::from)?;
        Ok(Vec::new())
    }

    fn type_refs(&self, blob: &DeclBlob) -> Result<Vec<TypeRef>, ReadError> {
        let decl = decode_decl(blob)?;
        let mut refs: Vec<TypeRef> = Vec::new();
        collect_type_refs(&decl.kind, &mut refs);
        // Deduplicate while preserving first-seen order.
        let mut seen = std::collections::HashSet::new();
        refs.retain(|r| seen.insert(r.name.clone()));
        Ok(refs)
    }

    // ── codegen ───────────────────────────────────────────────────────────────

    fn generate_descriptors(&self, closure: &SchemaClosure) -> Result<Bytes, DescriptorError> {
        // Assemble each schema file's reassembled YAML. For a single schema this
        // is the resolved document; for a multi-file closure (cross-file $ref) the
        // documents are concatenated as a YAML multi-document stream. Local
        // ($ref) component references stay symbolic — valid within each document.
        let mut sorted: Vec<(&schemahub_types::SchemaPath, &SchemaObjects)> =
            closure.entries.iter().collect();
        sorted.sort_by(|(a, _), (b, _)| a.schema_name.cmp(&b.schema_name));

        let mut out = String::new();
        for (_, schema) in sorted {
            let yaml =
                print_schema_objects(schema).map_err(|e| DescriptorError::Other(e.to_string()))?;
            if !out.is_empty() {
                out.push_str("---\n");
            }
            out.push_str(&yaml);
        }

        Ok(Bytes::from(out.into_bytes()))
    }

    fn generate_code(
        &self,
        _closure: &SchemaClosure,
        lang: Language,
    ) -> Result<String, CodegenError> {
        // OpenAPI server/client codegen is out of scope (deferred to v2).
        Err(CodegenError::UnsupportedLanguage(lang))
    }
}

// ── Mutation application ──────────────────────────────────────────────────────

fn apply_one(schema: &SchemaObjects, op: &OpenApiOp) -> Result<MutationEffect, MutationError> {
    match op {
        OpenApiOp::PushDocument { source } => {
            // Whole-document replace: re-parse and emit the full decl set as an
            // effect that upserts every new decl, replaces meta, and removes any
            // decl no longer present.
            let parsed = parse_openapi(source)
                .map_err(|e| MutationError::InvalidOperation(format!("parse error: {e}")))?;

            let new_keys: std::collections::HashSet<&str> =
                parsed.decls.iter().map(|(k, _)| k.as_str()).collect();
            let removes: Vec<String> = schema
                .decls
                .keys()
                .filter(|k| !new_keys.contains(k.as_str()))
                .cloned()
                .collect();

            Ok(MutationEffect {
                meta: Some(parsed.meta),
                upserts: parsed.decls,
                removes,
            })
        }

        OpenApiOp::AddPath {
            path_pattern,
            summary,
            description,
        } => {
            let key = format!("path:{path_pattern}");
            if schema.decls.contains_key(&key) {
                return Err(MutationError::InvalidOperation(format!(
                    "path '{path_pattern}' already exists"
                )));
            }
            let blob = PathItemBlob {
                path_pattern: path_pattern.clone(),
                summary: opt(summary),
                description: opt(description),
                ..Default::default()
            };
            Ok(upsert(key, DeclPayload::PathItem(blob)))
        }

        OpenApiOp::RemovePath { path_pattern } => {
            let key = format!("path:{path_pattern}");
            if !schema.decls.contains_key(&key) {
                return Err(MutationError::DeclarationNotFound(key));
            }
            Ok(MutationEffect {
                removes: vec![key],
                ..Default::default()
            })
        }

        OpenApiOp::AddOperation {
            path_pattern,
            method,
            operation_id,
            summary,
            description,
        } => {
            let key = format!("path:{path_pattern}");
            let mut path_blob = load_path_item(schema, &key)?;
            let method_enum: ast::HttpMethod = method.parse().map_err(|_| {
                MutationError::InvalidOperation(format!("unknown HTTP method: '{method}'"))
            })?;
            if path_blob.operations.iter().any(|o| o.method == method_enum) {
                return Err(MutationError::InvalidOperation(format!(
                    "operation '{method}' already exists on path '{path_pattern}'"
                )));
            }
            let mut new_op = OperationDef::empty(method_enum);
            new_op.operation_id = opt(operation_id);
            new_op.summary = opt(summary);
            new_op.description = opt(description);
            path_blob.operations.push(new_op);
            Ok(upsert(key, DeclPayload::PathItem(path_blob)))
        }

        OpenApiOp::RemoveOperation {
            path_pattern,
            method,
        } => {
            let key = format!("path:{path_pattern}");
            let mut path_blob = load_path_item(schema, &key)?;
            let method_enum: ast::HttpMethod = method.parse().map_err(|_| {
                MutationError::InvalidOperation(format!("unknown HTTP method: '{method}'"))
            })?;
            let before = path_blob.operations.len();
            path_blob.operations.retain(|o| o.method != method_enum);
            if path_blob.operations.len() == before {
                return Err(MutationError::InvalidOperation(format!(
                    "operation '{method}' not found on path '{path_pattern}'"
                )));
            }
            Ok(upsert(key, DeclPayload::PathItem(path_blob)))
        }

        OpenApiOp::AddComponentSchema {
            schema_name,
            schema_type,
            description,
        } => {
            let key = format!("schema:{schema_name}");
            if schema.decls.contains_key(&key) {
                return Err(MutationError::InvalidOperation(format!(
                    "component schema '{schema_name}' already exists"
                )));
            }
            let types = if schema_type.is_empty() {
                vec![]
            } else {
                match schema_type.parse::<JsonSchemaType>() {
                    Ok(t) => vec![t],
                    Err(_) => {
                        return Err(MutationError::InvalidOperation(format!(
                            "unknown JSON schema type: '{schema_type}'"
                        )))
                    }
                }
            };
            let blob = ComponentSchemaBlob {
                name: schema_name.clone(),
                schema: Some(JsonSchemaDef {
                    types,
                    description: opt(description),
                    ..Default::default()
                }),
                extensions: None,
            };
            Ok(upsert(key, DeclPayload::ComponentSchema(blob)))
        }

        OpenApiOp::RemoveComponentSchema { schema_name } => {
            let key = format!("schema:{schema_name}");
            if !schema.decls.contains_key(&key) {
                return Err(MutationError::DeclarationNotFound(key));
            }
            Ok(MutationEffect {
                removes: vec![key],
                ..Default::default()
            })
        }
    }
}

/// Build a single-upsert effect from a decl payload.
fn upsert(key: String, payload: DeclPayload) -> MutationEffect {
    MutationEffect {
        upserts: vec![(key, encode_decl(&OpenApiDecl::new(payload)))],
        ..Default::default()
    }
}

/// Load and decode a path-item decl by key, mapping errors to `MutationError`.
fn load_path_item(schema: &SchemaObjects, key: &str) -> Result<PathItemBlob, MutationError> {
    let blob = schema
        .decls
        .get(key)
        .ok_or_else(|| MutationError::DeclarationNotFound(key.to_string()))?;
    match decode_decl(blob)
        .map_err(|e| MutationError::MalformedBlob(e.0))?
        .kind
    {
        DeclPayload::PathItem(b) => Ok(b),
        _ => Err(MutationError::InvalidOperation(format!(
            "'{key}' is not a path item"
        ))),
    }
}

/// Convert an empty-string field into `None`.
fn opt(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Render one conflict side to a YAML fragment for human display.
fn render_side(blob: &DeclBlob) -> Result<String, ConflictError> {
    let decl = decode_decl(blob).map_err(|e| ConflictError::MalformedBlob(e.0))?;
    Ok(print_decl_detail(&decl.kind))
}

/// Collect the local component-schema names a declaration references.
fn collect_type_refs(payload: &DeclPayload, refs: &mut Vec<TypeRef>) {
    match payload {
        DeclPayload::PathItem(b) => {
            for op in &b.operations {
                if let Some(rb) = &op.request_body {
                    if let ast::RequestBodyOrRef::Inline(body) = rb {
                        for entry in &body.content {
                            if let Some(s) = &entry.schema {
                                collect_schema_refs(s, refs);
                            }
                        }
                    } else if let ast::RequestBodyOrRef::Ref(name) = rb {
                        refs.push(TypeRef::new(format!("requestBody:{name}")));
                    }
                }
                for resp in &op.responses {
                    match &resp.response {
                        Some(ast::ResponseOrRef::Inline(rd)) => {
                            for entry in &rd.content {
                                if let Some(s) = &entry.schema {
                                    collect_schema_refs(s, refs);
                                }
                            }
                        }
                        Some(ast::ResponseOrRef::Ref(name)) => {
                            refs.push(TypeRef::new(format!("response:{name}")));
                        }
                        None => {}
                    }
                }
                for p in &op.parameters {
                    match p {
                        ast::ParameterOrRef::Inline(param) => {
                            if let Some(s) = &param.schema {
                                collect_schema_refs(s, refs);
                            }
                        }
                        ast::ParameterOrRef::Ref(name) => {
                            refs.push(TypeRef::new(format!("param:{name}")));
                        }
                    }
                }
            }
        }
        DeclPayload::ComponentSchema(b) => {
            if let Some(s) = &b.schema {
                collect_schema_def_refs(s, refs);
            }
        }
        DeclPayload::ComponentParameter(b) => {
            if let Some(p) = &b.parameter {
                if let Some(s) = &p.schema {
                    collect_schema_refs(s, refs);
                }
            }
        }
        DeclPayload::ComponentResponse(b) => {
            if let Some(r) = &b.response {
                for entry in &r.content {
                    if let Some(s) = &entry.schema {
                        collect_schema_refs(s, refs);
                    }
                }
            }
        }
        DeclPayload::ComponentRequestBody(b) => {
            if let Some(r) = &b.request_body {
                for entry in &r.content {
                    if let Some(s) = &entry.schema {
                        collect_schema_refs(s, refs);
                    }
                }
            }
        }
    }
}

fn collect_schema_refs(s: &SchemaOrRef, refs: &mut Vec<TypeRef>) {
    match s {
        SchemaOrRef::Ref(r) => {
            if !r.local_name.is_empty() {
                refs.push(TypeRef::new(format!("schema:{}", r.local_name)));
            }
        }
        SchemaOrRef::Inline(def) => collect_schema_def_refs(def, refs),
    }
}

fn collect_schema_def_refs(schema: &JsonSchemaDef, refs: &mut Vec<TypeRef>) {
    if let Some(items) = &schema.items {
        collect_schema_refs(items, refs);
    }
    for prop in &schema.properties {
        if let Some(s) = &prop.schema {
            collect_schema_refs(s, refs);
        }
    }
    for s in schema
        .all_of
        .iter()
        .chain(&schema.any_of)
        .chain(&schema.one_of)
    {
        collect_schema_refs(s, refs);
    }
    if let Some(not) = &schema.not {
        collect_schema_refs(not, refs);
    }
    if let Some(ap) = &schema.additional_properties_schema {
        collect_schema_refs(ap, refs);
    }
}

#[cfg(test)]
mod tests;
