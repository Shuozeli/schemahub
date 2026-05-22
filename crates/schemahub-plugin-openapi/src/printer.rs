use schemahub_types::errors::PrintError;

use crate::ast::{
    ComponentParameterBlob, ComponentRequestBodyBlob, ComponentResponseBlob, ComponentSchemaBlob,
    HttpMethod, JsonSchemaDef, JsonSchemaType, MediaTypeEntry, OperationDef, ParameterDef,
    ParameterLocation, ParameterOrRef, PathItemBlob, RequestBodyDef, RequestBodyOrRef,
    ResponseDef, ResponseOrRef, SchemaOrRef,
    decode_component_parameter, decode_component_request_body, decode_component_response,
    decode_component_schema, decode_metadata, decode_path_item,
    parameter_or_ref, request_body_or_ref, response_or_ref, schema_or_ref, OpenApiParseResult,
    ParsedDeclaration,
};

fn sp(n: usize) -> String {
    " ".repeat(n)
}

/// Quote a YAML scalar if it contains characters that require quoting.
fn ys(s: &str) -> String {
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

/// Render the full OpenApiParseResult envelope to a valid OpenAPI 3.x YAML document.
pub fn print_envelope(result: &OpenApiParseResult) -> Result<String, PrintError> {
    // ── Collect declarations by type ───────────────────────────────────────────
    let mut metadata = None;
    let mut paths: Vec<PathItemBlob> = Vec::new();
    let mut schemas: Vec<ComponentSchemaBlob> = Vec::new();
    let mut parameters: Vec<ComponentParameterBlob> = Vec::new();
    let mut responses: Vec<ComponentResponseBlob> = Vec::new();
    let mut request_bodies: Vec<ComponentRequestBodyBlob> = Vec::new();

    for decl in &result.declarations {
        let key = &decl.tree_key;
        if key == "__metadata__" {
            metadata = Some(
                decode_metadata(&decl.blob_bytes)
                    .map_err(|e| PrintError::MalformedBlob(format!("metadata: {e}")))?,
            );
        } else if key.starts_with("path:") {
            paths.push(
                decode_path_item(&decl.blob_bytes)
                    .map_err(|e| PrintError::MalformedBlob(format!("path item: {e}")))?,
            );
        } else if key.starts_with("schema:") {
            schemas.push(
                decode_component_schema(&decl.blob_bytes)
                    .map_err(|e| PrintError::MalformedBlob(format!("schema: {e}")))?,
            );
        } else if key.starts_with("param:") {
            parameters.push(
                decode_component_parameter(&decl.blob_bytes)
                    .map_err(|e| PrintError::MalformedBlob(format!("param: {e}")))?,
            );
        } else if key.starts_with("response:") {
            responses.push(
                decode_component_response(&decl.blob_bytes)
                    .map_err(|e| PrintError::MalformedBlob(format!("response: {e}")))?,
            );
        } else if key.starts_with("requestBody:") {
            request_bodies.push(
                decode_component_request_body(&decl.blob_bytes)
                    .map_err(|e| PrintError::MalformedBlob(format!("requestBody: {e}")))?,
            );
        }
    }

    let mut out = String::new();

    // ── openapi / info / servers ───────────────────────────────────────────────
    if let Some(meta) = &metadata {
        out.push_str(&format!("openapi: \"{}\"\n", meta.openapi_version));
        if let Some(info) = &meta.info {
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
        if !meta.servers.is_empty() {
            out.push_str("servers:\n");
            for server in &meta.servers {
                out.push_str(&format!("  - url: {}\n", ys(&server.url)));
                if let Some(desc) = &server.description {
                    out.push_str(&format!("    description: {}\n", ys(desc)));
                }
            }
        }
    }

    // ── paths ──────────────────────────────────────────────────────────────────
    if !paths.is_empty() {
        out.push_str("paths:\n");
        for blob in &paths {
            // path key at 2 spaces, path item contents at 4
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
                    // list base = 4 → items at 6
                    out.push_str(&print_param_or_ref_item(param, 4));
                }
            }
            for op in &blob.operations {
                // operation key at 4, contents at 6
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

/// Render one HTTP operation under its parent path item.
/// `indent` = spaces for the method key (e.g. 4 → `    get:`).
fn print_operation(op: &OperationDef, indent: usize) -> String {
    let method = HttpMethod::from_i32(op.method)
        .map(|m| m.to_str())
        .unwrap_or("get");
    let ip = sp(indent);
    let ip2 = sp(indent + 2);
    let ip4 = sp(indent + 4);

    let mut out = format!("{}{}:\n", ip, method);
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
            // list base = indent+2 → items at indent+4
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
/// `list_base` = indent of the `parameters:` key.
/// Items are emitted at `list_base + 2` (with `- `), continuation at `list_base + 4`.
fn print_param_or_ref_item(p_or_r: &ParameterOrRef, list_base: usize) -> String {
    let item = sp(list_base + 2);
    let cont = sp(list_base + 4);
    match &p_or_r.value {
        Some(parameter_or_ref::Value::Ref(r)) => {
            format!("{}- $ref: '#/components/parameters/{}'\n", item, r)
        }
        Some(parameter_or_ref::Value::Inline(param)) => {
            let mut s = format!("{}- name: {}\n", item, ys(&param.name));
            s.push_str(&print_param_def_cont(param, &cont, list_base + 4));
            s
        }
        None => String::new(),
    }
}

/// Render an inline parameter definition as a block (all fields including `name:`).
/// `indent` = number of leading spaces.
fn print_param_def(param: &ParameterDef, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = format!("{}name: {}\n", ip, ys(&param.name));
    out.push_str(&print_param_def_cont(param, &ip, indent));
    out
}

/// Render parameter fields after `name:`, at the given indent string / level.
fn print_param_def_cont(param: &ParameterDef, ip: &str, indent: usize) -> String {
    let loc = ParameterLocation::from_i32(param.location)
        .map(|l| l.to_str())
        .unwrap_or("query");
    let mut out = format!("{}in: {}\n", ip, loc);
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

/// Render a requestBody inline or $ref at the given indent.
fn print_request_body_or_ref(rb: &RequestBodyOrRef, indent: usize) -> String {
    match &rb.value {
        Some(request_body_or_ref::Value::Ref(r)) => {
            format!(
                "{}'$ref': '#/components/requestBodies/{}'\n",
                sp(indent),
                r
            )
        }
        Some(request_body_or_ref::Value::Inline(b)) => print_request_body_def(b, indent),
        None => String::new(),
    }
}

/// Render a RequestBodyDef block at the given indent.
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

/// Render a response inline or $ref at the given indent.
fn print_response_or_ref(r: &ResponseOrRef, indent: usize) -> String {
    match &r.value {
        Some(response_or_ref::Value::Ref(r)) => {
            format!(
                "{}'$ref': '#/components/responses/{}'\n",
                sp(indent),
                r
            )
        }
        Some(response_or_ref::Value::Inline(resp)) => print_response_def(resp, indent),
        None => String::new(),
    }
}

/// Render a ResponseDef block at the given indent.
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

/// Render one media-type entry (`application/json: ...`) at the given indent.
fn print_media_type_entry(entry: &MediaTypeEntry, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = format!("{}{}:\n", ip, ys(&entry.media_type));
    if let Some(schema) = &entry.schema {
        out.push_str(&format!("{}  schema:\n", ip));
        out.push_str(&print_schema_or_ref(schema, indent + 4));
    }
    out
}

/// Render a SchemaOrRef ($ref or inline) at the given indent.
fn print_schema_or_ref(s: &SchemaOrRef, indent: usize) -> String {
    match &s.value {
        Some(schema_or_ref::Value::Ref(r)) => {
            format!(
                "{}$ref: '#/components/schemas/{}'\n",
                sp(indent),
                r.local_name
            )
        }
        Some(schema_or_ref::Value::Inline(def)) => print_schema_def(def, indent),
        None => String::new(),
    }
}

/// Render a JsonSchemaDef block at the given indent.
fn print_schema_def(schema: &JsonSchemaDef, indent: usize) -> String {
    let ip = sp(indent);
    let mut out = String::new();

    if !schema.types.is_empty() {
        if schema.types.len() == 1 {
            let t = JsonSchemaType::from_i32(schema.types[0])
                .map(|jt| jt.to_str())
                .unwrap_or("string");
            out.push_str(&format!("{}type: {}\n", ip, t));
        } else {
            out.push_str(&format!("{}type:\n", ip));
            for &t in &schema.types {
                if let Some(jt) = JsonSchemaType::from_i32(t) {
                    out.push_str(&format!("{}  - {}\n", ip, jt.to_str()));
                }
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

/// Render a single ParsedDeclaration to a YAML fragment.
/// Used by `get_declaration` to show detail for one named declaration.
pub fn print_declaration(decl: &ParsedDeclaration) -> Result<String, PrintError> {
    let key = &decl.tree_key;
    if key == "__metadata__" {
        let blob = decode_metadata(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("metadata: {e}")))?;
        let mut out = String::new();
        out.push_str(&format!("openapi: \"{}\"\n", blob.openapi_version));
        if let Some(info) = &blob.info {
            out.push_str("info:\n");
            out.push_str(&format!("  title: {}\n", ys(&info.title)));
            out.push_str(&format!("  version: \"{}\"\n", info.version));
            if let Some(desc) = &info.description {
                out.push_str(&format!("  description: {}\n", ys(desc)));
            }
        }
        return Ok(out);
    }
    if key.starts_with("path:") {
        let blob = decode_path_item(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("path item: {e}")))?;
        let mut out = format!("{}:\n", blob.path_pattern);
        if let Some(s) = &blob.summary {
            out.push_str(&format!("  summary: {}\n", ys(s)));
        }
        if let Some(d) = &blob.description {
            out.push_str(&format!("  description: {}\n", ys(d)));
        }
        for op in &blob.operations {
            out.push_str(&print_operation(op, 2));
        }
        return Ok(out);
    }
    if key.starts_with("schema:") {
        let blob = decode_component_schema(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("schema: {e}")))?;
        let mut out = format!("{}:\n", blob.name);
        if let Some(schema) = &blob.schema {
            out.push_str(&print_schema_def(schema, 2));
        }
        return Ok(out);
    }
    if key.starts_with("param:") {
        let blob = decode_component_parameter(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("param: {e}")))?;
        let mut out = format!("{}:\n", blob.name);
        if let Some(param) = &blob.parameter {
            out.push_str(&print_param_def(param, 2));
        }
        return Ok(out);
    }
    if key.starts_with("response:") {
        let blob = decode_component_response(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("response: {e}")))?;
        let mut out = format!("{}:\n", blob.name);
        if let Some(resp) = &blob.response {
            out.push_str(&print_response_def(resp, 2));
        }
        return Ok(out);
    }
    if key.starts_with("requestBody:") {
        let blob = decode_component_request_body(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("requestBody: {e}")))?;
        let mut out = format!("{}:\n", blob.name);
        if let Some(rb) = &blob.request_body {
            out.push_str(&print_request_body_def(rb, 2));
        }
        return Ok(out);
    }
    Ok(format!("# unknown declaration: {key}\n"))
}
