//! [`SchemaObjects`] → canonical OpenAPI 3.1 YAML.
//!
//! Ported from v1's `printer.rs`. Differences: it reassembles from the trait's
//! `SchemaObjects` (a `MetaBlob` plus a `BTreeMap<name, DeclBlob>`) instead of
//! the old flat envelope, and the AST `oneof` wrappers are now plain Rust enums.
//! Canonical key order follows OpenAPI structure (`openapi`, `info`, `servers`,
//! `paths`, `components`); within decls, declaration order is preserved.

use schemahub_types::errors::PrintError;
use schemahub_types::parsed::SchemaObjects;

use crate::ast::{
    ComponentParameterBlob, ComponentRequestBodyBlob, ComponentResponseBlob, ComponentSchemaBlob,
    DeclPayload, DocumentMetadataBlob, JsonSchemaDef, MediaTypeEntry, OperationDef, ParameterDef,
    ParameterOrRef, PathItemBlob, RequestBodyDef, RequestBodyOrRef, ResponseDef, ResponseOrRef,
    SchemaOrRef,
};
use crate::blob::{decode_decl, decode_meta};

fn sp(n: usize) -> String {
    " ".repeat(n)
}

/// Quote a YAML scalar if it contains characters that require quoting.
pub(crate) fn ys(s: &str) -> String {
    if s.is_empty()
        || s.contains(": ")
        || s.starts_with('#')
        || s.starts_with(|c: char| c.is_ascii_digit())
        || s == "true"
        || s == "false"
        || s == "null"
    {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Reassemble [`SchemaObjects`] into a complete OpenAPI 3.x YAML document.
pub fn print_schema_objects(schema: &SchemaObjects) -> Result<String, PrintError> {
    let metadata = decode_meta(&schema.meta)?;

    let mut paths: Vec<PathItemBlob> = Vec::new();
    let mut schemas: Vec<ComponentSchemaBlob> = Vec::new();
    let mut parameters: Vec<ComponentParameterBlob> = Vec::new();
    let mut responses: Vec<ComponentResponseBlob> = Vec::new();
    let mut request_bodies: Vec<ComponentRequestBodyBlob> = Vec::new();

    // BTreeMap iteration is sorted by key; the `path:` / `schema:` / … prefixes
    // keep kinds grouped and sorted within each kind.
    for blob in schema.decls.values() {
        match decode_decl(blob)?.kind {
            DeclPayload::PathItem(b) => paths.push(b),
            DeclPayload::ComponentSchema(b) => schemas.push(*b),
            DeclPayload::ComponentParameter(b) => parameters.push(b),
            DeclPayload::ComponentResponse(b) => responses.push(b),
            DeclPayload::ComponentRequestBody(b) => request_bodies.push(b),
        }
    }

    let mut out = String::new();
    print_metadata_header(&metadata, &mut out);

    // ── paths ──────────────────────────────────────────────────────────────────
    if !paths.is_empty() {
        out.push_str("paths:\n");
        for blob in &paths {
            out.push_str(&format!("  {}:\n", blob.path_pattern));
            if let Some(s) = &blob.summary {
                out.push_str(&format!("    summary: {}\n", ys(s)));
            }
            if let Some(d) = &blob.description {
                out.push_str(&format!("    description: {}\n", ys(d)));
            }
            if !blob.parameters.is_empty() {
                out.push_str("    parameters:\n");
                for param in &blob.parameters {
                    out.push_str(&print_param_or_ref_item(param, 4));
                }
            }
            for op in &blob.operations {
                out.push_str(&print_operation(op, 4));
            }
        }
    }

    // ── components ─────────────────────────────────────────────────────────────
    let has_components = !schemas.is_empty()
        || !parameters.is_empty()
        || !responses.is_empty()
        || !request_bodies.is_empty();
    if has_components {
        out.push_str("components:\n");
        if !schemas.is_empty() {
            out.push_str("  schemas:\n");
            for blob in &schemas {
                out.push_str(&format!("    {}:\n", blob.name));
                if let Some(schema) = &blob.schema {
                    out.push_str(&print_schema_def(schema, 6));
                }
            }
        }
        if !parameters.is_empty() {
            out.push_str("  parameters:\n");
            for blob in &parameters {
                out.push_str(&format!("    {}:\n", blob.name));
                if let Some(param) = &blob.parameter {
                    out.push_str(&print_param_def(param, 6));
                }
            }
        }
        if !responses.is_empty() {
            out.push_str("  responses:\n");
            for blob in &responses {
                out.push_str(&format!("    {}:\n", blob.name));
                if let Some(resp) = &blob.response {
                    out.push_str(&print_response_def(resp, 6));
                }
            }
        }
        if !request_bodies.is_empty() {
            out.push_str("  requestBodies:\n");
            for blob in &request_bodies {
                out.push_str(&format!("    {}:\n", blob.name));
                if let Some(rb) = &blob.request_body {
                    out.push_str(&print_request_body_def(rb, 6));
                }
            }
        }
    }

    Ok(out)
}

fn print_metadata_header(metadata: &DocumentMetadataBlob, out: &mut String) {
    if metadata.openapi_version.is_empty() && metadata.info.is_none() {
        return;
    }
    out.push_str(&format!("openapi: \"{}\"\n", metadata.openapi_version));
    if let Some(info) = &metadata.info {
        out.push_str("info:\n");
        out.push_str(&format!("  title: {}\n", ys(&info.title)));
        out.push_str(&format!("  version: \"{}\"\n", info.version));
        if let Some(desc) = &info.description {
            out.push_str(&format!("  description: {}\n", ys(desc)));
        }
        if let Some(tos) = &info.terms_of_service {
            out.push_str(&format!("  termsOfService: {}\n", ys(tos)));
        }
    }
    if !metadata.servers.is_empty() {
        out.push_str("servers:\n");
        for server in &metadata.servers {
            out.push_str(&format!("  - url: {}\n", ys(&server.url)));
            if let Some(desc) = &server.description {
                out.push_str(&format!("    description: {}\n", ys(desc)));
            }
        }
    }
}

/// Render one HTTP operation under its parent path item.
/// `indent` = spaces for the method key (e.g. 4 → `    get:`).
fn print_operation(op: &OperationDef, indent: usize) -> String {
    let ip = sp(indent);
    let ip2 = sp(indent + 2);
    let ip4 = sp(indent + 4);

    let mut out = format!("{}{}:\n", ip, op.method.to_str());
    if let Some(id) = &op.operation_id {
        out.push_str(&format!("{}operationId: {}\n", ip2, ys(id)));
    }
    if let Some(s) = &op.summary {
        out.push_str(&format!("{}summary: {}\n", ip2, ys(s)));
    }
    if let Some(d) = &op.description {
        out.push_str(&format!("{}description: {}\n", ip2, ys(d)));
    }
    if op.deprecated.unwrap_or(false) {
        out.push_str(&format!("{}deprecated: true\n", ip2));
    }
    if !op.tags.is_empty() {
        out.push_str(&format!("{}tags:\n", ip2));
        for tag in &op.tags {
            out.push_str(&format!("{}- {}\n", ip4, ys(tag)));
        }
    }
    if !op.parameters.is_empty() {
        out.push_str(&format!("{}parameters:\n", ip2));
        for param in &op.parameters {
            out.push_str(&print_param_or_ref_item(param, indent + 2));
        }
    }
    if let Some(rb) = &op.request_body {
        out.push_str(&format!("{}requestBody:\n", ip2));
        out.push_str(&print_request_body_or_ref(rb, indent + 4));
    }
    if !op.responses.is_empty() {
        out.push_str(&format!("{}responses:\n", ip2));
        for entry in &op.responses {
            out.push_str(&format!("{}\"{}\":\n", ip4, entry.status_code));
            if let Some(resp) = &entry.response {
                out.push_str(&print_response_or_ref(resp, indent + 6));
            }
        }
    }
    out
}

/// Render a parameter list item (inside a `parameters:` sequence).
fn print_param_or_ref_item(p_or_r: &ParameterOrRef, list_base: usize) -> String {
    let item = sp(list_base + 2);
    let cont = sp(list_base + 4);
    match p_or_r {
        ParameterOrRef::Ref(r) => {
            format!("{}- $ref: '#/components/parameters/{}'\n", item, r)
        }
        ParameterOrRef::Inline(param) => {
            let mut s = format!("{}- name: {}\n", item, ys(&param.name));
            s.push_str(&print_param_def_cont(param, &cont, list_base + 4));
            s
        }
    }
}

/// Render an inline parameter definition as a block (all fields including `name:`).
fn print_param_def(param: &ParameterDef, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = format!("{}name: {}\n", ip, ys(&param.name));
    out.push_str(&print_param_def_cont(param, &ip, indent));
    out
}

/// Render parameter fields after `name:`, at the given indent string / level.
fn print_param_def_cont(param: &ParameterDef, ip: &str, indent: usize) -> String {
    let mut out = format!("{}in: {}\n", ip, param.location.to_str());
    out.push_str(&format!("{}required: {}\n", ip, param.required));
    if let Some(desc) = &param.description {
        out.push_str(&format!("{}description: {}\n", ip, ys(desc)));
    }
    if param.deprecated.unwrap_or(false) {
        out.push_str(&format!("{}deprecated: true\n", ip));
    }
    if let Some(schema) = &param.schema {
        out.push_str(&format!("{}schema:\n", ip));
        out.push_str(&print_schema_or_ref(schema, indent + 2));
    }
    out
}

fn print_request_body_or_ref(rb: &RequestBodyOrRef, indent: usize) -> String {
    match rb {
        RequestBodyOrRef::Ref(r) => {
            format!("{}$ref: '#/components/requestBodies/{}'\n", sp(indent), r)
        }
        RequestBodyOrRef::Inline(b) => print_request_body_def(b, indent),
    }
}

fn print_request_body_def(rb: &RequestBodyDef, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = String::new();
    if let Some(desc) = &rb.description {
        out.push_str(&format!("{}description: {}\n", ip, ys(desc)));
    }
    out.push_str(&format!("{}required: {}\n", ip, rb.required));
    if !rb.content.is_empty() {
        out.push_str(&format!("{}content:\n", ip));
        for entry in &rb.content {
            out.push_str(&print_media_type_entry(entry, indent + 2));
        }
    }
    out
}

fn print_response_or_ref(r: &ResponseOrRef, indent: usize) -> String {
    match r {
        ResponseOrRef::Ref(r) => {
            format!("{}$ref: '#/components/responses/{}'\n", sp(indent), r)
        }
        ResponseOrRef::Inline(resp) => print_response_def(resp, indent),
    }
}

fn print_response_def(resp: &ResponseDef, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = format!("{}description: {}\n", ip, ys(&resp.description));
    if !resp.headers.is_empty() {
        out.push_str(&format!("{}headers:\n", ip));
        for header in &resp.headers {
            out.push_str(&format!("{}  {}:\n", ip, header.name));
            if let Some(desc) = &header.description {
                out.push_str(&format!("{}    description: {}\n", ip, ys(desc)));
            }
            if let Some(schema) = &header.schema {
                out.push_str(&format!("{}    schema:\n", ip));
                out.push_str(&print_schema_or_ref(schema, indent + 6));
            }
        }
    }
    if !resp.content.is_empty() {
        out.push_str(&format!("{}content:\n", ip));
        for entry in &resp.content {
            out.push_str(&print_media_type_entry(entry, indent + 2));
        }
    }
    out
}

fn print_media_type_entry(entry: &MediaTypeEntry, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = format!("{}{}:\n", ip, ys(&entry.media_type));
    if let Some(schema) = &entry.schema {
        out.push_str(&format!("{}  schema:\n", ip));
        out.push_str(&print_schema_or_ref(schema, indent + 4));
    }
    out
}

fn print_schema_or_ref(s: &SchemaOrRef, indent: usize) -> String {
    match s {
        SchemaOrRef::Ref(r) => {
            format!(
                "{}$ref: '#/components/schemas/{}'\n",
                sp(indent),
                r.local_name
            )
        }
        SchemaOrRef::Inline(def) => print_schema_def(def, indent),
    }
}

fn print_schema_def(schema: &JsonSchemaDef, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = String::new();

    if !schema.types.is_empty() {
        if schema.types.len() == 1 {
            out.push_str(&format!("{}type: {}\n", ip, schema.types[0].to_str()));
        } else {
            out.push_str(&format!("{}type:\n", ip));
            for t in &schema.types {
                out.push_str(&format!("{}  - {}\n", ip, t.to_str()));
            }
        }
    }
    if let Some(title) = &schema.title {
        out.push_str(&format!("{}title: {}\n", ip, ys(title)));
    }
    if let Some(desc) = &schema.description {
        out.push_str(&format!("{}description: {}\n", ip, ys(desc)));
    }
    if let Some(fmt) = &schema.format {
        out.push_str(&format!("{}format: {}\n", ip, ys(fmt)));
    }
    if !schema.enum_values.is_empty() {
        out.push_str(&format!("{}enum:\n", ip));
        for v in &schema.enum_values {
            out.push_str(&format!("{}  - {}\n", ip, ys(v)));
        }
    }
    if let Some(cv) = &schema.const_value {
        out.push_str(&format!("{}const: {}\n", ip, ys(cv)));
    }
    if !schema.properties.is_empty() {
        out.push_str(&format!("{}properties:\n", ip));
        for prop in &schema.properties {
            out.push_str(&format!("{}  {}:\n", ip, prop.name));
            if let Some(s) = &prop.schema {
                out.push_str(&print_schema_or_ref(s, indent + 4));
            }
        }
    }
    if !schema.required.is_empty() {
        out.push_str(&format!("{}required:\n", ip));
        for r in &schema.required {
            out.push_str(&format!("{}  - {}\n", ip, ys(r)));
        }
    }
    if let Some(items) = &schema.items {
        out.push_str(&format!("{}items:\n", ip));
        out.push_str(&print_schema_or_ref(items, indent + 2));
    }
    if !schema.all_of.is_empty() {
        out.push_str(&format!("{}allOf:\n", ip));
        for s in &schema.all_of {
            out.push_str(&format!("{}  -\n", ip));
            out.push_str(&print_schema_or_ref(s, indent + 4));
        }
    }
    if !schema.any_of.is_empty() {
        out.push_str(&format!("{}anyOf:\n", ip));
        for s in &schema.any_of {
            out.push_str(&format!("{}  -\n", ip));
            out.push_str(&print_schema_or_ref(s, indent + 4));
        }
    }
    if !schema.one_of.is_empty() {
        out.push_str(&format!("{}oneOf:\n", ip));
        for s in &schema.one_of {
            out.push_str(&format!("{}  -\n", ip));
            out.push_str(&print_schema_or_ref(s, indent + 4));
        }
    }
    if let Some(not) = &schema.not {
        out.push_str(&format!("{}not:\n", ip));
        out.push_str(&print_schema_or_ref(not, indent + 2));
    }
    if schema.deprecated.unwrap_or(false) {
        out.push_str(&format!("{}deprecated: true\n", ip));
    }
    out
}

/// Render a single decl (by tree key + payload) to a YAML fragment.
/// Used by `decl_detail` to show one named declaration in isolation.
pub fn print_decl_detail(payload: &DeclPayload) -> String {
    match payload {
        DeclPayload::PathItem(blob) => {
            let mut out = format!("{}:\n", blob.path_pattern);
            if let Some(s) = &blob.summary {
                out.push_str(&format!("  summary: {}\n", ys(s)));
            }
            if let Some(d) = &blob.description {
                out.push_str(&format!("  description: {}\n", ys(d)));
            }
            if !blob.parameters.is_empty() {
                out.push_str("  parameters:\n");
                for param in &blob.parameters {
                    out.push_str(&print_param_or_ref_item(param, 2));
                }
            }
            for op in &blob.operations {
                out.push_str(&print_operation(op, 2));
            }
            out
        }
        DeclPayload::ComponentSchema(blob) => {
            let mut out = format!("{}:\n", blob.name);
            if let Some(schema) = &blob.schema {
                out.push_str(&print_schema_def(schema, 2));
            }
            out
        }
        DeclPayload::ComponentParameter(blob) => {
            let mut out = format!("{}:\n", blob.name);
            if let Some(param) = &blob.parameter {
                out.push_str(&print_param_def(param, 2));
            }
            out
        }
        DeclPayload::ComponentResponse(blob) => {
            let mut out = format!("{}:\n", blob.name);
            if let Some(resp) = &blob.response {
                out.push_str(&print_response_def(resp, 2));
            }
            out
        }
        DeclPayload::ComponentRequestBody(blob) => {
            let mut out = format!("{}:\n", blob.name);
            if let Some(rb) = &blob.request_body {
                out.push_str(&print_request_body_def(rb, 2));
            }
            out
        }
    }
}
