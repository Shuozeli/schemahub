//! OpenAPI 3.1 source (YAML/JSON) → [`ParsedSchema`].
//!
//! Ported from v1's `parser.rs`. Difference: instead of a flat
//! `OpenApiParseResult` envelope, this produces the trait's
//! `ParsedSchema { meta, decls }` — `__metadata__` becomes `meta` (a
//! [`MetaBlob`]), and every other declaration becomes a `(tree_key, DeclBlob)`
//! pair where the tree key is the stable path-model key
//! (`path:…`, `schema:…`, `param:…`, `response:…`, `requestBody:…`).

use std::str::FromStr as _;

use schemahub_types::errors::ParseError;
use schemahub_types::parsed::ParsedSchema;
use serde_yaml::Value;

use crate::ast::{
    ComponentParameterBlob, ComponentRequestBodyBlob, ComponentResponseBlob, ComponentSchemaBlob,
    DeclPayload, DocumentMetadataBlob, Extensions, HeaderDef, HttpMethod, InfoObject,
    JsonSchemaDef, JsonSchemaType, MediaTypeEntry, OpenApiDecl, OperationDef, ParameterDef,
    ParameterLocation, ParameterOrRef, PathItemBlob, PropertyDef, RequestBodyDef, RequestBodyOrRef,
    ResponseDef, ResponseEntry, ResponseOrRef, SchemaOrRef, SchemaRef, ServerObject, BLOB_VERSION,
};
use crate::blob::{encode_decl, encode_meta};
use crate::reference::{parse_source_component_reference, ComponentReference};

/// Parse an OpenAPI 3.1 document (YAML or JSON — JSON is a subset of YAML).
pub fn parse_openapi(source: &str) -> Result<ParsedSchema, ParseError> {
    let root: Value = serde_yaml::from_str(source).map_err(|e| ParseError::SyntaxError {
        line: 0,
        message: e.to_string(),
    })?;

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
    let servers = parse_servers(root_map.get("servers"))?;

    let metadata = DocumentMetadataBlob {
        blob_version: BLOB_VERSION,
        openapi_version: openapi_version.to_owned(),
        info: Some(info_obj),
        servers,
        extensions: extract_extensions(root_map, "OpenAPI document")?,
    };

    let mut decls: Vec<(String, schemahub_types::blob::DeclBlob)> = Vec::new();

    // ── Paths ──────────────────────────────────────────────────────────────────
    if let Some(paths) = optional_mapping(root_map, "paths", "OpenAPI document")? {
        for (path_key, path_val) in paths.iter() {
            let path_str = string_key(path_key, "paths")?.to_owned();
            let blob = parse_path_item(&path_str, path_val)?;
            decls.push((
                format!("path:{path_str}"),
                encode_decl(&OpenApiDecl::new(DeclPayload::PathItem(blob))),
            ));
        }
    }

    // ── Components ──────────────────────────────────────────────────────────────
    if let Some(components) = optional_mapping(root_map, "components", "OpenAPI document")? {
        if let Some(schemas) = optional_mapping(components, "schemas", "components")? {
            for (name_key, schema_val) in schemas.iter() {
                let name = string_key(name_key, "components.schemas")?.to_owned();
                let blob = ComponentSchemaBlob {
                    name: name.clone(),
                    schema: Some(parse_schema_def(schema_val)?),
                    extensions: None,
                };
                decls.push((
                    format!("schema:{name}"),
                    encode_decl(&OpenApiDecl::new(DeclPayload::ComponentSchema(Box::new(
                        blob,
                    )))),
                ));
            }
        }
        if let Some(params) = optional_mapping(components, "parameters", "components")? {
            for (name_key, param_val) in params.iter() {
                let name = string_key(name_key, "components.parameters")?.to_owned();
                let blob = ComponentParameterBlob {
                    name: name.clone(),
                    parameter: Some(parse_parameter_def(param_val)?),
                };
                decls.push((
                    format!("param:{name}"),
                    encode_decl(&OpenApiDecl::new(DeclPayload::ComponentParameter(blob))),
                ));
            }
        }
        if let Some(responses) = optional_mapping(components, "responses", "components")? {
            for (name_key, resp_val) in responses.iter() {
                let name = string_key(name_key, "components.responses")?.to_owned();
                let blob = ComponentResponseBlob {
                    name: name.clone(),
                    response: Some(parse_response_def(resp_val)?),
                };
                decls.push((
                    format!("response:{name}"),
                    encode_decl(&OpenApiDecl::new(DeclPayload::ComponentResponse(blob))),
                ));
            }
        }
        if let Some(bodies) = optional_mapping(components, "requestBodies", "components")? {
            for (name_key, body_val) in bodies.iter() {
                let name = string_key(name_key, "components.requestBodies")?.to_owned();
                let blob = ComponentRequestBodyBlob {
                    name: name.clone(),
                    request_body: Some(parse_request_body_def(body_val)?),
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

    Ok(InfoObject {
        title,
        description,
        version,
        terms_of_service,
    })
}

fn parse_servers(servers: Option<&Value>) -> Result<Vec<ServerObject>, ParseError> {
    let Some(value) = servers else {
        return Ok(Vec::new());
    };
    let sequence = value
        .as_sequence()
        .ok_or_else(|| ParseError::Other("'servers' must be an array".into()))?;

    sequence
        .iter()
        .enumerate()
        .map(|(index, server)| {
            let context = format!("servers[{index}]");
            let map = mapping(server, &context)?;
            Ok(ServerObject {
                url: required_string(map, "url", &context)?.to_owned(),
                description: optional_string(map, "description", &context)?,
            })
        })
        .collect()
}

fn parse_path_item(path: &str, val: &Value) -> Result<PathItemBlob, ParseError> {
    let context = format!("path {path:?}");
    reject_standalone_reference(val, &context)?;
    let map = mapping(val, &context)?;

    let summary = map
        .get("summary")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let description = map
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let parameters = parse_optional_array(map.get("parameters"), &format!("{context}.parameters"))?
        .iter()
        .map(parse_parameter_or_ref)
        .collect::<Result<Vec<_>, _>>()?;

    let extensions = extract_extensions(map, &context)?;

    let mut operations: Vec<OperationDef> = Vec::new();
    let http_methods = [
        "get", "post", "put", "delete", "patch", "head", "options", "trace",
    ];
    for method_str in &http_methods {
        if let Some(op_val) = map.get(*method_str) {
            // The input is a string literal from `http_methods`, so this
            // never errors — but use `expect` rather than `unwrap` for
            // clarity in case the literal list ever drifts.
            let method = HttpMethod::from_str(method_str)
                .expect("http_methods literal must be a valid HttpMethod");
            operations.push(parse_operation_def(method, op_val)?);
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

fn parse_operation_def(method: HttpMethod, val: &Value) -> Result<OperationDef, ParseError> {
    let context = format!("{} operation", method.to_str());
    let map = mapping(val, &context)?;

    let operation_id = map
        .get("operationId")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let summary = map
        .get("summary")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let description = map
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let deprecated = map.get("deprecated").and_then(|v| v.as_bool());

    let tags = parse_optional_array(map.get("tags"), &format!("{context}.tags"))?
        .iter()
        .enumerate()
        .map(|(index, tag)| {
            tag.as_str().map(str::to_owned).ok_or_else(|| {
                ParseError::Other(format!("{context}.tags[{index}] must be a string"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let parameters = parse_optional_array(map.get("parameters"), &format!("{context}.parameters"))?
        .iter()
        .map(parse_parameter_or_ref)
        .collect::<Result<Vec<_>, _>>()?;

    let request_body = map
        .get("requestBody")
        .map(parse_request_body_or_ref)
        .transpose()?;

    let responses = optional_mapping(map, "responses", &context)?
        .map(|response_map| {
            response_map
                .iter()
                .map(|(key, value)| {
                    Ok(ResponseEntry {
                        status_code: response_key_to_string(key)?,
                        response: Some(parse_response_or_ref(value)?),
                    })
                })
                .collect::<Result<Vec<_>, ParseError>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(OperationDef {
        method,
        operation_id,
        summary,
        description,
        tags,
        parameters,
        request_body,
        responses,
        deprecated,
        extensions: extract_extensions(map, &context)?,
    })
}

fn parse_parameter_or_ref(val: &Value) -> Result<ParameterOrRef, ParseError> {
    if let Some(ref_str) = get_ref(val)? {
        let reference =
            parse_source_component_reference(&ref_str, "parameters").map_err(ParseError::Other)?;
        Ok(ParameterOrRef::Ref(match reference {
            ComponentReference::Local(name) => name,
            ComponentReference::External(_) => ref_str,
        }))
    } else {
        Ok(ParameterOrRef::Inline(Box::new(parse_parameter_def(val)?)))
    }
}

fn parse_parameter_def(val: &Value) -> Result<ParameterDef, ParseError> {
    reject_standalone_reference(val, "parameter")?;
    let map = mapping(val, "parameter")?;

    let name = required_string(map, "name", "parameter")?.to_owned();
    let location_str = required_string(map, "in", "parameter")?;
    let location = ParameterLocation::from_str(location_str)
        .map_err(|error| ParseError::Other(error.to_string()))?;
    let description = optional_string(map, "description", "parameter")?;
    let required = map
        .get("required")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let deprecated = map.get("deprecated").and_then(|v| v.as_bool());
    let schema = map.get("schema").map(parse_schema_or_ref).transpose()?;

    Ok(ParameterDef {
        name,
        location,
        description,
        required,
        deprecated,
        schema,
        extensions: extract_extensions(map, "parameter")?,
    })
}

fn parse_request_body_or_ref(val: &Value) -> Result<RequestBodyOrRef, ParseError> {
    if let Some(ref_str) = get_ref(val)? {
        let reference = parse_source_component_reference(&ref_str, "requestBodies")
            .map_err(ParseError::Other)?;
        Ok(RequestBodyOrRef::Ref(match reference {
            ComponentReference::Local(name) => name,
            ComponentReference::External(_) => ref_str,
        }))
    } else {
        Ok(RequestBodyOrRef::Inline(parse_request_body_def(val)?))
    }
}

fn parse_request_body_def(val: &Value) -> Result<RequestBodyDef, ParseError> {
    reject_standalone_reference(val, "request body")?;
    let map = mapping(val, "request body")?;

    Ok(RequestBodyDef {
        description: optional_string(map, "description", "request body")?,
        required: map
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        content: parse_content(map.get("content"))?,
        extensions: extract_extensions(map, "request body")?,
    })
}

fn parse_response_or_ref(val: &Value) -> Result<ResponseOrRef, ParseError> {
    if let Some(ref_str) = get_ref(val)? {
        let reference =
            parse_source_component_reference(&ref_str, "responses").map_err(ParseError::Other)?;
        Ok(ResponseOrRef::Ref(match reference {
            ComponentReference::Local(name) => name,
            ComponentReference::External(_) => ref_str,
        }))
    } else {
        Ok(ResponseOrRef::Inline(parse_response_def(val)?))
    }
}

fn parse_response_def(val: &Value) -> Result<ResponseDef, ParseError> {
    reject_standalone_reference(val, "response")?;
    let map = mapping(val, "response")?;

    let description = required_string(map, "description", "response")?.to_owned();
    let content = parse_content(map.get("content"))?;

    let headers = optional_mapping(map, "headers", "response")?
        .map(|hm| {
            hm.iter()
                .map(|(k, v)| {
                    let name = string_key(k, "response.headers")?.to_owned();
                    if get_ref(v)?.is_some() {
                        return Err(ParseError::Other(format!(
                            "response header {name:?} uses an unsupported reference"
                        )));
                    }
                    let hmap = mapping(v, &format!("response header {name:?}"))?;
                    let description =
                        optional_string(hmap, "description", &format!("response header {name:?}"))?;
                    let required = hmap.get("required").and_then(|r| r.as_bool());
                    let schema = hmap.get("schema").map(parse_schema_or_ref).transpose()?;
                    Ok(HeaderDef {
                        name,
                        description,
                        required,
                        schema,
                    })
                })
                .collect::<Result<Vec<_>, ParseError>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(ResponseDef {
        description,
        content,
        headers,
        extensions: extract_extensions(map, "response")?,
    })
}

fn parse_content(val: Option<&Value>) -> Result<Vec<MediaTypeEntry>, ParseError> {
    let Some(value) = val else {
        return Ok(Vec::new());
    };
    let content = mapping(value, "content")?;
    content
        .iter()
        .map(|(key, value)| {
            let media_type = string_key(key, "content")?.to_owned();
            let context = format!("media type {media_type:?}");
            let media_map = mapping(value, &context)?;
            Ok(MediaTypeEntry {
                media_type,
                schema: media_map
                    .get("schema")
                    .map(parse_schema_or_ref)
                    .transpose()?,
                extensions: extract_extensions(media_map, &context)?,
            })
        })
        .collect()
}

fn parse_schema_or_ref(val: &Value) -> Result<SchemaOrRef, ParseError> {
    if let Some(ref_str) = get_ref(val)? {
        let reference =
            parse_source_component_reference(&ref_str, "schemas").map_err(ParseError::Other)?;
        Ok(SchemaOrRef::Ref(match reference {
            ComponentReference::Local(local_name) => SchemaRef {
                local_name,
                external_import: None,
            },
            ComponentReference::External(external_import) => SchemaRef {
                local_name: String::new(),
                external_import: Some(external_import),
            },
        }))
    } else {
        Ok(SchemaOrRef::Inline(Box::new(parse_schema_def(val)?)))
    }
}

/// Parse a JSON Schema definition. `pub` so mutations can reuse it.
pub fn parse_schema_def(val: &Value) -> Result<JsonSchemaDef, ParseError> {
    reject_standalone_reference(val, "component schema definition")?;
    let map = mapping(val, "JSON Schema")?;

    let types: Vec<JsonSchemaType> = if let Some(type_val) = map.get("type") {
        if let Some(s) = type_val.as_str() {
            vec![JsonSchemaType::from_str(s)
                .map_err(|error| ParseError::Other(error.to_string()))?]
        } else if let Some(seq) = type_val.as_sequence() {
            seq.iter()
                .enumerate()
                .map(|(index, value)| {
                    let schema_type = value.as_str().ok_or_else(|| {
                        ParseError::Other(format!("JSON Schema type[{index}] must be a string"))
                    })?;
                    JsonSchemaType::from_str(schema_type)
                        .map_err(|error| ParseError::Other(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            return Err(ParseError::Other(
                "JSON Schema 'type' must be a string or array of strings".into(),
            ));
        }
    } else {
        vec![]
    };

    let format = map
        .get("format")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let description = map
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let title = map.get("title").and_then(|v| v.as_str()).map(str::to_owned);
    let deprecated = map.get("deprecated").and_then(|v| v.as_bool());
    let read_only = map.get("readOnly").and_then(|v| v.as_bool());
    let write_only = map.get("writeOnly").and_then(|v| v.as_bool());

    let pattern = map
        .get("pattern")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
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

    let items = map
        .get("items")
        .map(parse_schema_or_ref)
        .transpose()?
        .map(Box::new);

    let properties: Vec<PropertyDef> = optional_mapping(map, "properties", "JSON Schema")?
        .map(|pm| {
            pm.iter()
                .map(|(key, value)| {
                    Ok(PropertyDef {
                        name: string_key(key, "JSON Schema properties")?.to_owned(),
                        schema: Some(parse_schema_or_ref(value)?),
                    })
                })
                .collect::<Result<Vec<_>, ParseError>>()
        })
        .transpose()?
        .unwrap_or_default();

    let required: Vec<String> = parse_optional_array(map.get("required"), "JSON Schema required")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                ParseError::Other(format!("JSON Schema required[{index}] must be a string"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let min_properties = map.get("minProperties").and_then(|v| v.as_u64());
    let max_properties = map.get("maxProperties").and_then(|v| v.as_u64());

    let (additional_properties_allowed, additional_properties_schema) =
        match map.get("additionalProperties") {
            Some(ap) => {
                if let Some(b) = ap.as_bool() {
                    (Some(b), None)
                } else {
                    (None, Some(Box::new(parse_schema_or_ref(ap)?)))
                }
            }
            None => (None, None),
        };

    let all_of = parse_schema_array(map.get("allOf"), "JSON Schema allOf")?;
    let any_of = parse_schema_array(map.get("anyOf"), "JSON Schema anyOf")?;
    let one_of = parse_schema_array(map.get("oneOf"), "JSON Schema oneOf")?;

    let not = map
        .get("not")
        .map(parse_schema_or_ref)
        .transpose()?
        .map(Box::new);

    let enum_values: Vec<String> = parse_optional_array(map.get("enum"), "JSON Schema enum")?
        .iter()
        .map(|value| json_value_to_string(value, "JSON Schema enum value"))
        .collect::<Result<Vec<_>, _>>()?;

    let const_value = map
        .get("const")
        .map(|value| json_value_to_string(value, "JSON Schema const"))
        .transpose()?;
    let default = map
        .get("default")
        .map(|value| json_value_to_string(value, "JSON Schema default"))
        .transpose()?;

    Ok(JsonSchemaDef {
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
        extensions: extract_extensions(map, "JSON Schema")?,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn mapping<'a>(value: &'a Value, context: &str) -> Result<&'a serde_yaml::Mapping, ParseError> {
    value
        .as_mapping()
        .ok_or_else(|| ParseError::Other(format!("{context} must be an object")))
}

fn optional_mapping<'a>(
    map: &'a serde_yaml::Mapping,
    field: &str,
    context: &str,
) -> Result<Option<&'a serde_yaml::Mapping>, ParseError> {
    map.get(field)
        .map(|value| mapping(value, &format!("{context}.{field}")))
        .transpose()
}

fn parse_optional_array<'a>(
    value: Option<&'a Value>,
    context: &str,
) -> Result<&'a [Value], ParseError> {
    match value {
        Some(value) => value
            .as_sequence()
            .map(Vec::as_slice)
            .ok_or_else(|| ParseError::Other(format!("{context} must be an array"))),
        None => Ok(&[]),
    }
}

fn required_string<'a>(
    map: &'a serde_yaml::Mapping,
    field: &str,
    context: &str,
) -> Result<&'a str, ParseError> {
    map.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ParseError::Other(format!("{context}.{field} must be a string")))
}

fn optional_string(
    map: &serde_yaml::Mapping,
    field: &str,
    context: &str,
) -> Result<Option<String>, ParseError> {
    match map.get(field) {
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| ParseError::Other(format!("{context}.{field} must be a string"))),
        None => Ok(None),
    }
}

fn string_key<'a>(key: &'a Value, context: &str) -> Result<&'a str, ParseError> {
    key.as_str()
        .ok_or_else(|| ParseError::Other(format!("{context} key must be a string")))
}

fn get_ref(val: &Value) -> Result<Option<String>, ParseError> {
    let Some(map) = val.as_mapping() else {
        return Ok(None);
    };
    let Some(reference) = map.get("$ref") else {
        return Ok(None);
    };
    if map.len() != 1 {
        return Err(ParseError::Other(
            "'$ref' objects with sibling fields are outside the selected OpenAPI AST surface"
                .into(),
        ));
    }
    let reference = reference
        .as_str()
        .filter(|reference| !reference.is_empty())
        .ok_or_else(|| ParseError::Other("'$ref' must be a non-empty string".into()))?;
    Ok(Some(reference.to_owned()))
}

fn reject_standalone_reference(val: &Value, context: &str) -> Result<(), ParseError> {
    if let Some(reference) = get_ref(val)? {
        return Err(ParseError::Other(format!(
            "{context} uses standalone $ref {reference:?}, which is outside the selected \
             OpenAPI AST surface"
        )));
    }
    Ok(())
}

fn parse_schema_array(
    value: Option<&Value>,
    context: &str,
) -> Result<Vec<SchemaOrRef>, ParseError> {
    parse_optional_array(value, context)?
        .iter()
        .map(parse_schema_or_ref)
        .collect()
}

fn json_value_to_string(value: &Value, context: &str) -> Result<String, ParseError> {
    serde_json::to_string(value)
        .map_err(|error| ParseError::Other(format!("{context} is not valid JSON: {error}")))
}

fn extract_extensions(
    map: &serde_yaml::Mapping,
    context: &str,
) -> Result<Option<Extensions>, ParseError> {
    let ext_map: serde_yaml::Mapping = map
        .iter()
        .filter(|(k, _)| k.as_str().map(|s| s.starts_with("x-")).unwrap_or(false))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if ext_map.is_empty() {
        Ok(None)
    } else {
        let json_bytes = serde_json::to_vec(&ext_map).map_err(|error| {
            ParseError::Other(format!("{context} extensions are not valid JSON: {error}"))
        })?;
        Ok(Some(Extensions { json_bytes }))
    }
}

fn response_key_to_string(val: &Value) -> Result<String, ParseError> {
    match val {
        Value::String(s) => Ok(s.clone()),
        // YAML commonly decodes an unquoted `200` response key as a number.
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(ParseError::Other(
            "response status key must be a string or number".into(),
        )),
    }
}
