use schemahub_types::errors::PrintError;

use crate::ast::{
    ComponentParameterBlob, ComponentRequestBodyBlob, ComponentResponseBlob, ComponentSchemaBlob,
    DocumentMetadataBlob, HeaderDef, HttpMethod, JsonSchemaDef, JsonSchemaType, MediaTypeEntry,
    OperationDef, ParameterDef, ParameterLocation, ParameterOrRef, PathItemBlob, PropertyDef,
    RequestBodyDef, RequestBodyOrRef, ResponseDef, ResponseEntry, ResponseOrRef, SchemaOrRef,
    decode_component_parameter, decode_component_request_body, decode_component_response,
    decode_component_schema, decode_metadata, decode_path_item, decode_parse_result,
    parameter_or_ref, request_body_or_ref, response_or_ref, schema_or_ref, OpenApiParseResult,
    ParsedDeclaration,
};

/// Render the full OpenApiParseResult envelope to a YAML string.
pub fn print_envelope(result: &OpenApiParseResult) -> Result<String, PrintError> {
    let mut out = String::new();
    out.push_str("# OpenAPI Parse Result\n");
    out.push_str(&format!("blob_version: {}\n", result.blob_version));
    out.push_str(&format!("declarations: {} total\n\n", result.declarations.len()));

    for decl in &result.declarations {
        let fragment = print_declaration(decl)?;
        out.push_str(&fragment);
        out.push('\n');
    }

    Ok(out)
}

/// Render a single ParsedDeclaration to a human-readable YAML-like string.
pub fn print_declaration(decl: &ParsedDeclaration) -> Result<String, PrintError> {
    let key = &decl.tree_key;

    if key == "__metadata__" {
        let blob = decode_metadata(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("metadata: {e}")))?;
        return Ok(print_metadata(&blob));
    }

    if let Some(path) = key.strip_prefix("path:") {
        let blob = decode_path_item(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("path item: {e}")))?;
        return Ok(print_path_item(&blob));
    }

    if let Some(name) = key.strip_prefix("schema:") {
        let blob = decode_component_schema(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("schema: {e}")))?;
        return Ok(print_component_schema(&blob));
    }

    if let Some(name) = key.strip_prefix("param:") {
        let blob = decode_component_parameter(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("param: {e}")))?;
        return Ok(print_component_parameter(&blob));
    }

    if let Some(name) = key.strip_prefix("response:") {
        let blob = decode_component_response(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("response: {e}")))?;
        return Ok(print_component_response(&blob));
    }

    if let Some(name) = key.strip_prefix("requestBody:") {
        let blob = decode_component_request_body(&decl.blob_bytes)
            .map_err(|e| PrintError::MalformedBlob(format!("requestBody: {e}")))?;
        return Ok(print_component_request_body(&blob));
    }

    Ok(format!("# Unknown declaration: {key}\n"))
}

fn print_metadata(blob: &DocumentMetadataBlob) -> String {
    let mut out = String::new();
    out.push_str("# DocumentMetadata\n");
    out.push_str(&format!("openapi: {}\n", blob.openapi_version));
    if let Some(info) = &blob.info {
        out.push_str("info:\n");
        out.push_str(&format!("  title: {}\n", info.title));
        out.push_str(&format!("  version: {}\n", info.version));
        if let Some(desc) = &info.description {
            out.push_str(&format!("  description: {desc}\n"));
        }
    }
    if !blob.servers.is_empty() {
        out.push_str("servers:\n");
        for server in &blob.servers {
            out.push_str(&format!("  - url: {}\n", server.url));
            if let Some(desc) = &server.description {
                out.push_str(&format!("    description: {desc}\n"));
            }
        }
    }
    out
}

fn print_path_item(blob: &PathItemBlob) -> String {
    let mut out = String::new();
    out.push_str(&format!("# PathItem: {}\n", blob.path_pattern));
    out.push_str(&format!("path: {}\n", blob.path_pattern));
    if let Some(summary) = &blob.summary {
        out.push_str(&format!("summary: {summary}\n"));
    }
    if let Some(desc) = &blob.description {
        out.push_str(&format!("description: {desc}\n"));
    }
    if !blob.parameters.is_empty() {
        out.push_str("parameters:\n");
        for p in &blob.parameters {
            out.push_str(&format!("  - {}\n", print_parameter_or_ref(p)));
        }
    }
    for op in &blob.operations {
        out.push_str(&print_operation(op));
    }
    out
}

fn print_operation(op: &OperationDef) -> String {
    let method = HttpMethod::from_i32(op.method)
        .map(|m| m.to_str())
        .unwrap_or("unknown");
    let mut out = format!("{method}:\n");
    if let Some(id) = &op.operation_id {
        out.push_str(&format!("  operationId: {id}\n"));
    }
    if let Some(s) = &op.summary {
        out.push_str(&format!("  summary: {s}\n"));
    }
    if !op.parameters.is_empty() {
        out.push_str("  parameters:\n");
        for p in &op.parameters {
            out.push_str(&format!("    - {}\n", print_parameter_or_ref(p)));
        }
    }
    if let Some(rb) = &op.request_body {
        out.push_str("  requestBody:\n");
        out.push_str(&format!("    {}\n", print_request_body_or_ref(rb)));
    }
    if !op.responses.is_empty() {
        out.push_str("  responses:\n");
        for re in &op.responses {
            out.push_str(&format!("    '{}': {}\n", re.status_code,
                re.response.as_ref().map(print_response_or_ref).unwrap_or_default()));
        }
    }
    out
}

fn print_parameter_or_ref(p: &ParameterOrRef) -> String {
    match &p.value {
        Some(parameter_or_ref::Value::Ref(r)) => format!("$ref: '#/components/parameters/{r}'"),
        Some(parameter_or_ref::Value::Inline(p)) => print_parameter_inline(p),
        None => String::new(),
    }
}

fn print_parameter_inline(p: &ParameterDef) -> String {
    let loc = ParameterLocation::from_i32(p.location)
        .map(|l| l.to_str())
        .unwrap_or("query");
    format!("name: {}, in: {}, required: {}", p.name, loc, p.required)
}

fn print_request_body_or_ref(rb: &RequestBodyOrRef) -> String {
    match &rb.value {
        Some(request_body_or_ref::Value::Ref(r)) => {
            format!("$ref: '#/components/requestBodies/{r}'")
        }
        Some(request_body_or_ref::Value::Inline(b)) => {
            format!("required: {}, content: {} media types", b.required, b.content.len())
        }
        None => String::new(),
    }
}

fn print_response_or_ref(r: &ResponseOrRef) -> String {
    match &r.value {
        Some(response_or_ref::Value::Ref(r)) => format!("$ref: '#/components/responses/{r}'"),
        Some(response_or_ref::Value::Inline(resp)) => {
            format!("description: {}", resp.description)
        }
        None => String::new(),
    }
}

fn print_component_schema(blob: &ComponentSchemaBlob) -> String {
    let mut out = format!("# ComponentSchema: {}\n", blob.name);
    out.push_str(&format!("name: {}\n", blob.name));
    if let Some(schema) = &blob.schema {
        out.push_str(&print_schema_def(schema, 0));
    }
    out
}

fn print_schema_def(schema: &JsonSchemaDef, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    let mut out = String::new();
    if !schema.types.is_empty() {
        let types: Vec<&str> = schema
            .types
            .iter()
            .filter_map(|&t| JsonSchemaType::from_i32(t).map(|jt| jt.to_str()))
            .collect();
        out.push_str(&format!("{pad}type: {}\n", types.join(", ")));
    }
    if let Some(desc) = &schema.description {
        out.push_str(&format!("{pad}description: {desc}\n"));
    }
    if !schema.properties.is_empty() {
        out.push_str(&format!("{pad}properties:\n"));
        for prop in &schema.properties {
            out.push_str(&format!("{pad}  {}:\n", prop.name));
            if let Some(s) = &prop.schema {
                out.push_str(&print_schema_or_ref(s, indent + 2));
            }
        }
    }
    if !schema.required.is_empty() {
        out.push_str(&format!("{pad}required: [{}]\n", schema.required.join(", ")));
    }
    out
}

fn print_schema_or_ref(s: &SchemaOrRef, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    match &s.value {
        Some(schema_or_ref::Value::Ref(r)) => {
            format!("{pad}$ref: '#/components/schemas/{}'\n", r.local_name)
        }
        Some(schema_or_ref::Value::Inline(def)) => print_schema_def(def, indent),
        None => String::new(),
    }
}

fn print_component_parameter(blob: &ComponentParameterBlob) -> String {
    let mut out = format!("# ComponentParameter: {}\n", blob.name);
    if let Some(param) = &blob.parameter {
        let loc = ParameterLocation::from_i32(param.location)
            .map(|l| l.to_str())
            .unwrap_or("query");
        out.push_str(&format!("name: {}\nin: {}\nrequired: {}\n", param.name, loc, param.required));
        if let Some(desc) = &param.description {
            out.push_str(&format!("description: {desc}\n"));
        }
    }
    out
}

fn print_component_response(blob: &ComponentResponseBlob) -> String {
    let mut out = format!("# ComponentResponse: {}\n", blob.name);
    if let Some(resp) = &blob.response {
        out.push_str(&format!("description: {}\n", resp.description));
        if !resp.content.is_empty() {
            out.push_str(&format!("content: {} media types\n", resp.content.len()));
        }
    }
    out
}

fn print_component_request_body(blob: &ComponentRequestBodyBlob) -> String {
    let mut out = format!("# ComponentRequestBody: {}\n", blob.name);
    if let Some(body) = &blob.request_body {
        out.push_str(&format!("required: {}\n", body.required));
        if let Some(desc) = &body.description {
            out.push_str(&format!("description: {desc}\n"));
        }
        out.push_str(&format!("content: {} media types\n", body.content.len()));
    }
    out
}
