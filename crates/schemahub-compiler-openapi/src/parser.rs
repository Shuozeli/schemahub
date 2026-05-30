//! OpenAPI 3.1 source (YAML/JSON) → [`ParsedSchema`].
//!
//! Ported from v1's `parser.rs`. Difference: instead of a flat
//! `OpenApiParseResult` envelope, this produces the trait's
//! `ParsedSchema { meta, decls }` — `__metadata__` becomes `meta` (a
//! [`MetaBlob`]), and every other declaration becomes a `(tree_key, DeclBlob)`
//! pair where the tree key is the stable path-model key
//! (`path:…`, `schema:…`, `param:…`, `response:…`, `requestBody:…`).

use schemahub_types::errors::ParseError;
use schemahub_types::parsed::ParsedSchema;
use serde_yaml::Value;

use crate::ast::{
    ComponentParameterBlob, ComponentRequestBodyBlob, ComponentResponseBlob, ComponentSchemaBlob,
    DeclPayload, DocumentMetadataBlob, Extensions, HeaderDef, HttpMethod, InfoObject, JsonSchemaDef,
    JsonSchemaType, MediaTypeEntry, OpenApiDecl, OperationDef, ParameterDef, ParameterLocation,
    ParameterOrRef, PathItemBlob, PropertyDef, RequestBodyDef, RequestBodyOrRef, ResponseDef,
    ResponseEntry, ResponseOrRef, SchemaOrRef, SchemaRef, ServerObject, BLOB_VERSION,
};
use crate::blob::{encode_decl, encode_meta};

/// Parse an OpenAPI 3.1 document (YAML or JSON — JSON is a subset of YAML).
pub fn parse_openapi(source: &str) -> Result<ParsedSchema, ParseError> {
    let root: Value = serde_yaml::from_str(source)
        .map_err(|e| ParseError::SyntaxError { line: 0, message: e.to_string() })?;

    let root_map = root
        .as_mapping()
        .ok_or_else(|| ParseError::Other("OpenAPI document must be a YAML/JSON mapping".into()))?;

    let openapi_version = root_map
        .get("openapi")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::Other("missing 'openapi' field".into()))?;

    if !openapi_version.starts_with("3.") {
        return Err(ParseError::UnsupportedVersion(openapi_version.to_owned()));
    }

    // ── Metadata (→ MetaBlob) ──────────────────────────────────────────────────
    let info_obj = parse_info_object(root_map.get("info"))?;
    let servers = root_map
        .get("servers")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|s| {
                    let url = s.get("url")?.as_str()?.to_owned();
                    let description = s
                        .get("description")
                        .and_then(|d| d.as_str())
                        .map(str::to_owned);
                    Some(ServerObject { url, description })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let metadata = DocumentMetadataBlob {
        blob_version: BLOB_VERSION,
        openapi_version: openapi_version.to_owned(),
        info: Some(info_obj),
        servers,
        extensions: extract_extensions(root_map),
    };

    let mut decls: Vec<(String, schemahub_types::blob::DeclBlob)> = Vec::new();

    // ── Paths ──────────────────────────────────────────────────────────────────
    if let Some(paths) = root_map.get("paths").and_then(|v| v.as_mapping()) {
        for (path_key, path_val) in paths.iter() {
            let path_str = path_key.as_str().unwrap_or_default().to_owned();
            let blob = parse_path_item(&path_str, path_val)?;
            decls.push((
                format!("path:{path_str}"),
                encode_decl(&OpenApiDecl::new(DeclPayload::PathItem(blob))),
            ));
        }
    }

    // ── Components ──────────────────────────────────────────────────────────────
    if let Some(components) = root_map.get("components").and_then(|v| v.as_mapping()) {
        if let Some(schemas) = components.get("schemas").and_then(|v| v.as_mapping()) {
            for (name_key, schema_val) in schemas.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let blob = ComponentSchemaBlob {
                    name: name.clone(),
                    schema: Some(parse_schema_def(schema_val)),
                    extensions: None,
                };
                decls.push((
                    format!("schema:{name}"),
                    encode_decl(&OpenApiDecl::new(DeclPayload::ComponentSchema(blob))),
                ));
            }
        }
        if let Some(params) = components.get("parameters").and_then(|v| v.as_mapping()) {
            for (name_key, param_val) in params.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let blob = ComponentParameterBlob {
                    name: name.clone(),
                    parameter: Some(parse_parameter_def(param_val)),
                };
                decls.push((
                    format!("param:{name}"),
                    encode_decl(&OpenApiDecl::new(DeclPayload::ComponentParameter(blob))),
                ));
            }
        }
        if let Some(responses) = components.get("responses").and_then(|v| v.as_mapping()) {
            for (name_key, resp_val) in responses.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let blob = ComponentResponseBlob {
                    name: name.clone(),
                    response: Some(parse_response_def(resp_val)),
                };
                decls.push((
                    format!("response:{name}"),
                    encode_decl(&OpenApiDecl::new(DeclPayload::ComponentResponse(blob))),
                ));
            }
        }
        if let Some(bodies) = components.get("requestBodies").and_then(|v| v.as_mapping()) {
            for (name_key, body_val) in bodies.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let blob = ComponentRequestBodyBlob {
                    name: name.clone(),
                    request_body: Some(parse_request_body_def(body_val)),
                };
                decls.push((
                    format!("requestBody:{name}"),
                    encode_decl(&OpenApiDecl::new(DeclPayload::ComponentRequestBody(blob))),
                ));
            }
        }
    }

    Ok(ParsedSchema {
        meta: encode_meta(&metadata),
        decls,
    })
}

fn parse_info_object(info: Option<&Value>) -> Result<InfoObject, ParseError> {
    let info_map = info
        .and_then(|v| v.as_mapping())
        .ok_or_else(|| ParseError::Other("missing 'info' object".into()))?;

    let title = info_map
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::Other("missing 'info.title'".into()))?
        .to_owned();

    let version = info_map
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::Other("missing 'info.version'".into()))?
        .to_owned();

    let description = info_map
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let terms_of_service = info_map
        .get("termsOfService")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    Ok(InfoObject { title, description, version, terms_of_service })
}

fn parse_path_item(path: &str, val: &Value) -> Result<PathItemBlob, ParseError> {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => {
            return Ok(PathItemBlob {
                path_pattern: path.to_owned(),
                ..Default::default()
            });
        }
    };

    let summary = map.get("summary").and_then(|v| v.as_str()).map(str::to_owned);
    let description = map.get("description").and_then(|v| v.as_str()).map(str::to_owned);

    let parameters = map
        .get("parameters")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().map(parse_parameter_or_ref).collect())
        .unwrap_or_default();

    let extensions = extract_extensions(map);

    let mut operations: Vec<OperationDef> = Vec::new();
    let http_methods = ["get", "post", "put", "delete", "patch", "head", "options", "trace"];
    for method_str in &http_methods {
        if let Some(op_val) = map.get(*method_str) {
            let method = HttpMethod::from_str(method_str).unwrap();
            operations.push(parse_operation_def(method, op_val));
        }
    }

    Ok(PathItemBlob {
        path_pattern: path.to_owned(),
        summary,
        description,
        parameters,
        operations,
        extensions,
    })
}

fn parse_operation_def(method: HttpMethod, val: &Value) -> OperationDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => return OperationDef::empty(method),
    };

    let operation_id = map.get("operationId").and_then(|v| v.as_str()).map(str::to_owned);
    let summary = map.get("summary").and_then(|v| v.as_str()).map(str::to_owned);
    let description = map.get("description").and_then(|v| v.as_str()).map(str::to_owned);
    let deprecated = map.get("deprecated").and_then(|v| v.as_bool());

    let tags = map
        .get("tags")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();

    let parameters = map
        .get("parameters")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().map(parse_parameter_or_ref).collect())
        .unwrap_or_default();

    let request_body = map.get("requestBody").map(parse_request_body_or_ref);

    let responses = map
        .get("responses")
        .and_then(|v| v.as_mapping())
        .map(|rm| {
            rm.iter()
                .map(|(k, v)| ResponseEntry {
                    status_code: value_to_string(k),
                    response: Some(parse_response_or_ref(v)),
                })
                .collect()
        })
        .unwrap_or_default();

    OperationDef {
        method,
        operation_id,
        summary,
        description,
        tags,
        parameters,
        request_body,
        responses,
        deprecated,
        extensions: extract_extensions(map),
    }
}

fn parse_parameter_or_ref(val: &Value) -> ParameterOrRef {
    if let Some(ref_str) = get_ref(val) {
        let name = ref_str
            .strip_prefix("#/components/parameters/")
            .unwrap_or(&ref_str)
            .to_owned();
        ParameterOrRef::Ref(name)
    } else {
        ParameterOrRef::Inline(parse_parameter_def(val))
    }
}

fn parse_parameter_def(val: &Value) -> ParameterDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => return ParameterDef::default(),
    };

    let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
    let location_str = map.get("in").and_then(|v| v.as_str()).unwrap_or("query");
    let location = ParameterLocation::from_str(location_str).unwrap_or(ParameterLocation::Query);
    let description = map.get("description").and_then(|v| v.as_str()).map(str::to_owned);
    let required = map.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
    let deprecated = map.get("deprecated").and_then(|v| v.as_bool());
    let schema = map.get("schema").map(parse_schema_or_ref);

    ParameterDef {
        name,
        location,
        description,
        required,
        deprecated,
        schema,
        extensions: extract_extensions(map),
    }
}

fn parse_request_body_or_ref(val: &Value) -> RequestBodyOrRef {
    if let Some(ref_str) = get_ref(val) {
        let name = ref_str
            .strip_prefix("#/components/requestBodies/")
            .unwrap_or(&ref_str)
            .to_owned();
        RequestBodyOrRef::Ref(name)
    } else {
        RequestBodyOrRef::Inline(parse_request_body_def(val))
    }
}

fn parse_request_body_def(val: &Value) -> RequestBodyDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => return RequestBodyDef::default(),
    };

    RequestBodyDef {
        description: map.get("description").and_then(|v| v.as_str()).map(str::to_owned),
        required: map.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
        content: parse_content(map.get("content")),
        extensions: extract_extensions(map),
    }
}

fn parse_response_or_ref(val: &Value) -> ResponseOrRef {
    if let Some(ref_str) = get_ref(val) {
        let name = ref_str
            .strip_prefix("#/components/responses/")
            .unwrap_or(&ref_str)
            .to_owned();
        ResponseOrRef::Ref(name)
    } else {
        ResponseOrRef::Inline(parse_response_def(val))
    }
}

fn parse_response_def(val: &Value) -> ResponseDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => return ResponseDef::default(),
    };

    let description = map.get("description").and_then(|v| v.as_str()).unwrap_or("").to_owned();
    let content = parse_content(map.get("content"));

    let headers = map
        .get("headers")
        .and_then(|v| v.as_mapping())
        .map(|hm| {
            hm.iter()
                .map(|(k, v)| {
                    let name = k.as_str().unwrap_or("").to_owned();
                    let hmap = v.as_mapping();
                    let description = hmap
                        .and_then(|m| m.get("description"))
                        .and_then(|d| d.as_str())
                        .map(str::to_owned);
                    let required = hmap.and_then(|m| m.get("required")).and_then(|r| r.as_bool());
                    let schema = hmap.and_then(|m| m.get("schema")).map(parse_schema_or_ref);
                    HeaderDef { name, description, required, schema }
                })
                .collect()
        })
        .unwrap_or_default();

    ResponseDef {
        description,
        content,
        headers,
        extensions: extract_extensions(map),
    }
}

fn parse_content(val: Option<&Value>) -> Vec<MediaTypeEntry> {
    val.and_then(|v| v.as_mapping())
        .map(|cm| {
            cm.iter()
                .map(|(k, v)| {
                    let media_type = k.as_str().unwrap_or("").to_owned();
                    let cmap = v.as_mapping();
                    let schema = cmap.and_then(|m| m.get("schema")).map(parse_schema_or_ref);
                    let extensions = cmap.and_then(extract_extensions);
                    MediaTypeEntry { media_type, schema, extensions }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_schema_or_ref(val: &Value) -> SchemaOrRef {
    if let Some(ref_str) = get_ref(val) {
        let local_name = ref_str
            .strip_prefix("#/components/schemas/")
            .map(str::to_owned)
            .unwrap_or_else(|| ref_str.clone());
        SchemaOrRef::Ref(SchemaRef { local_name, external_import: None })
    } else {
        SchemaOrRef::Inline(parse_schema_def(val))
    }
}

/// Parse a JSON Schema definition. `pub` so mutations can reuse it.
pub fn parse_schema_def(val: &Value) -> JsonSchemaDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => return JsonSchemaDef::default(),
    };

    let types: Vec<JsonSchemaType> = if let Some(type_val) = map.get("type") {
        if let Some(s) = type_val.as_str() {
            JsonSchemaType::from_str(s).map(|t| vec![t]).unwrap_or_default()
        } else if let Some(seq) = type_val.as_sequence() {
            seq.iter()
                .filter_map(|t| t.as_str().and_then(JsonSchemaType::from_str))
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let format = map.get("format").and_then(|v| v.as_str()).map(str::to_owned);
    let description = map.get("description").and_then(|v| v.as_str()).map(str::to_owned);
    let title = map.get("title").and_then(|v| v.as_str()).map(str::to_owned);
    let deprecated = map.get("deprecated").and_then(|v| v.as_bool());
    let read_only = map.get("readOnly").and_then(|v| v.as_bool());
    let write_only = map.get("writeOnly").and_then(|v| v.as_bool());

    let pattern = map.get("pattern").and_then(|v| v.as_str()).map(str::to_owned);
    let min_length = map.get("minLength").and_then(|v| v.as_u64());
    let max_length = map.get("maxLength").and_then(|v| v.as_u64());

    let minimum = map.get("minimum").and_then(|v| v.as_f64());
    let maximum = map.get("maximum").and_then(|v| v.as_f64());
    let exclusive_minimum = map.get("exclusiveMinimum").and_then(|v| v.as_bool());
    let exclusive_maximum = map.get("exclusiveMaximum").and_then(|v| v.as_bool());
    let multiple_of = map.get("multipleOf").and_then(|v| v.as_f64());

    let min_items = map.get("minItems").and_then(|v| v.as_u64());
    let max_items = map.get("maxItems").and_then(|v| v.as_u64());
    let unique_items = map.get("uniqueItems").and_then(|v| v.as_bool());

    let items = map.get("items").map(|v| Box::new(parse_schema_or_ref(v)));

    let properties: Vec<PropertyDef> = map
        .get("properties")
        .and_then(|v| v.as_mapping())
        .map(|pm| {
            pm.iter()
                .map(|(k, v)| PropertyDef {
                    name: k.as_str().unwrap_or("").to_owned(),
                    schema: Some(parse_schema_or_ref(v)),
                })
                .collect()
        })
        .unwrap_or_default();

    let required: Vec<String> = map
        .get("required")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();

    let min_properties = map.get("minProperties").and_then(|v| v.as_u64());
    let max_properties = map.get("maxProperties").and_then(|v| v.as_u64());

    let (additional_properties_allowed, additional_properties_schema) =
        match map.get("additionalProperties") {
            Some(ap) => {
                if let Some(b) = ap.as_bool() {
                    (Some(b), None)
                } else {
                    (None, Some(Box::new(parse_schema_or_ref(ap))))
                }
            }
            None => (None, None),
        };

    let all_of = map
        .get("allOf")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().map(parse_schema_or_ref).collect())
        .unwrap_or_default();

    let any_of = map
        .get("anyOf")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().map(parse_schema_or_ref).collect())
        .unwrap_or_default();

    let one_of = map
        .get("oneOf")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().map(parse_schema_or_ref).collect())
        .unwrap_or_default();

    let not = map.get("not").map(|v| Box::new(parse_schema_or_ref(v)));

    let enum_values: Vec<String> = map
        .get("enum")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().map(|v| serde_json::to_string(v).unwrap_or_default()).collect())
        .unwrap_or_default();

    let const_value = map.get("const").map(|v| serde_json::to_string(v).unwrap_or_default());
    let default = map.get("default").map(|v| serde_json::to_string(v).unwrap_or_default());

    JsonSchemaDef {
        types,
        format,
        min_length,
        max_length,
        pattern,
        minimum,
        maximum,
        exclusive_minimum,
        exclusive_maximum,
        multiple_of,
        items,
        min_items,
        max_items,
        unique_items,
        properties,
        required,
        additional_properties_allowed,
        additional_properties_schema,
        min_properties,
        max_properties,
        all_of,
        any_of,
        one_of,
        not,
        enum_values,
        const_value,
        title,
        description,
        default,
        deprecated,
        read_only,
        write_only,
        extensions: extract_extensions(map),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_ref(val: &Value) -> Option<String> {
    val.as_mapping()
        .and_then(|m| m.get("$ref"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

fn extract_extensions(map: &serde_yaml::Mapping) -> Option<Extensions> {
    let ext_map: serde_yaml::Mapping = map
        .iter()
        .filter(|(k, _)| k.as_str().map(|s| s.starts_with("x-")).unwrap_or(false))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if ext_map.is_empty() {
        None
    } else {
        Some(Extensions { json_bytes: serde_json::to_vec(&ext_map).unwrap_or_default() })
    }
}

fn value_to_string(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}
