use schemahub_types::errors::ParseError;
use serde_yaml::Value;

use crate::ast::{
    ComponentParameterBlob, ComponentRequestBodyBlob, ComponentResponseBlob, ComponentSchemaBlob,
    DocumentMetadataBlob, Extensions, HeaderDef, HttpMethod, InfoObject, JsonSchemaDef,
    JsonSchemaType, MediaTypeEntry, OperationDef, ParameterDef, ParameterLocation, ParameterOrRef,
    ParsedDeclaration, PathItemBlob, PropertyDef, RequestBodyDef, RequestBodyOrRef, ResponseDef,
    ResponseEntry, ResponseOrRef, SchemaOrRef, SchemaRef, ServerObject,
    decode_component_parameter, decode_component_request_body, decode_component_response,
    decode_component_schema, decode_metadata, decode_path_item, encode_component_parameter,
    encode_component_request_body, encode_component_response, encode_component_schema,
    encode_metadata, encode_path_item, parameter_or_ref, request_body_or_ref, response_or_ref,
    schema_or_ref, OpenApiParseResult,
};

pub fn parse_openapi(source: &str) -> Result<OpenApiParseResult, ParseError> {
    let root: Value = serde_yaml::from_str(source)
        .map_err(|e| ParseError::SyntaxError { line: 0, message: e.to_string() })?;

    let root_map = root.as_mapping().ok_or_else(|| {
        ParseError::Other("OpenAPI document must be a YAML mapping".into())
    })?;

    // Validate openapi version
    let openapi_version = root_map
        .get("openapi")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::Other("missing 'openapi' field".into()))?;

    if !openapi_version.starts_with("3.") {
        return Err(ParseError::UnsupportedVersion(openapi_version.to_owned()));
    }

    let mut declarations: Vec<ParsedDeclaration> = Vec::new();

    // Build DocumentMetadataBlob
    let info = root_map.get("info");
    let info_obj = parse_info_object(info)?;

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

    let meta_extensions = extract_extensions(root_map);

    let metadata = DocumentMetadataBlob {
        blob_version: 1,
        openapi_version: openapi_version.to_owned(),
        info: Some(info_obj),
        servers,
        extensions: meta_extensions,
    };
    declarations.push(ParsedDeclaration {
        tree_key: "__metadata__".into(),
        blob_bytes: encode_metadata(&metadata),
    });

    // Parse paths
    if let Some(paths) = root_map.get("paths").and_then(|v| v.as_mapping()) {
        for (path_key, path_val) in paths.iter() {
            let path_str = path_key.as_str().unwrap_or_default().to_owned();
            let blob = parse_path_item(&path_str, path_val)?;
            declarations.push(ParsedDeclaration {
                tree_key: format!("path:{path_str}"),
                blob_bytes: encode_path_item(&blob),
            });
        }
    }

    // Parse components
    if let Some(components) = root_map.get("components").and_then(|v| v.as_mapping()) {
        // schemas
        if let Some(schemas) = components.get("schemas").and_then(|v| v.as_mapping()) {
            for (name_key, schema_val) in schemas.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let schema = parse_schema_def(schema_val);
                let blob = ComponentSchemaBlob {
                    blob_version: 1,
                    name: name.clone(),
                    schema: Some(schema),
                    extensions: None,
                };
                declarations.push(ParsedDeclaration {
                    tree_key: format!("schema:{name}"),
                    blob_bytes: encode_component_schema(&blob),
                });
            }
        }

        // parameters
        if let Some(params) = components.get("parameters").and_then(|v| v.as_mapping()) {
            for (name_key, param_val) in params.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let parameter = parse_parameter_def(param_val);
                let blob = ComponentParameterBlob {
                    blob_version: 1,
                    name: name.clone(),
                    parameter: Some(parameter),
                };
                declarations.push(ParsedDeclaration {
                    tree_key: format!("param:{name}"),
                    blob_bytes: encode_component_parameter(&blob),
                });
            }
        }

        // responses
        if let Some(responses) = components.get("responses").and_then(|v| v.as_mapping()) {
            for (name_key, resp_val) in responses.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let response = parse_response_def(resp_val);
                let blob = ComponentResponseBlob {
                    blob_version: 1,
                    name: name.clone(),
                    response: Some(response),
                };
                declarations.push(ParsedDeclaration {
                    tree_key: format!("response:{name}"),
                    blob_bytes: encode_component_response(&blob),
                });
            }
        }

        // requestBodies
        if let Some(bodies) = components.get("requestBodies").and_then(|v| v.as_mapping()) {
            for (name_key, body_val) in bodies.iter() {
                let name = name_key.as_str().unwrap_or_default().to_owned();
                let request_body = parse_request_body_def(body_val);
                let blob = ComponentRequestBodyBlob {
                    blob_version: 1,
                    name: name.clone(),
                    request_body: Some(request_body),
                };
                declarations.push(ParsedDeclaration {
                    tree_key: format!("requestBody:{name}"),
                    blob_bytes: encode_component_request_body(&blob),
                });
            }
        }
    }

    Ok(OpenApiParseResult { blob_version: 1, declarations })
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
                blob_version: 1,
                path_pattern: path.to_owned(),
                summary: None,
                description: None,
                parameters: vec![],
                operations: vec![],
                extensions: None,
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
            let op = parse_operation_def(method, op_val);
            operations.push(op);
        }
    }

    Ok(PathItemBlob {
        blob_version: 1,
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
        None => return OperationDef { method: method as i32, ..Default::default() },
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
                .map(|(k, v)| {
                    let status_code = value_to_string(k);
                    let response = Some(parse_response_or_ref(v));
                    ResponseEntry { status_code, response }
                })
                .collect()
        })
        .unwrap_or_default();

    let extensions = extract_extensions(map);

    OperationDef {
        method: method as i32,
        operation_id,
        summary,
        description,
        tags,
        parameters,
        request_body,
        responses,
        deprecated,
        extensions,
    }
}

fn parse_parameter_or_ref(val: &Value) -> ParameterOrRef {
    if let Some(ref_str) = get_ref(val) {
        // $ref to components/parameters
        let name = ref_str
            .strip_prefix("#/components/parameters/")
            .unwrap_or(&ref_str)
            .to_owned();
        ParameterOrRef {
            value: Some(parameter_or_ref::Value::Ref(name)),
        }
    } else {
        ParameterOrRef {
            value: Some(parameter_or_ref::Value::Inline(parse_parameter_def(val))),
        }
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
    let extensions = extract_extensions(map);

    ParameterDef {
        name,
        location: location as i32,
        description,
        required,
        deprecated,
        schema,
        extensions,
    }
}

fn parse_request_body_or_ref(val: &Value) -> RequestBodyOrRef {
    if let Some(ref_str) = get_ref(val) {
        let name = ref_str
            .strip_prefix("#/components/requestBodies/")
            .unwrap_or(&ref_str)
            .to_owned();
        RequestBodyOrRef {
            value: Some(request_body_or_ref::Value::Ref(name)),
        }
    } else {
        RequestBodyOrRef {
            value: Some(request_body_or_ref::Value::Inline(parse_request_body_def(val))),
        }
    }
}

fn parse_request_body_def(val: &Value) -> RequestBodyDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => return RequestBodyDef::default(),
    };

    let description = map.get("description").and_then(|v| v.as_str()).map(str::to_owned);
    let required = map.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
    let content = parse_content(map.get("content"));
    let extensions = extract_extensions(map);

    RequestBodyDef { description, required, content, extensions }
}

fn parse_response_or_ref(val: &Value) -> ResponseOrRef {
    if let Some(ref_str) = get_ref(val) {
        let name = ref_str
            .strip_prefix("#/components/responses/")
            .unwrap_or(&ref_str)
            .to_owned();
        ResponseOrRef {
            value: Some(response_or_ref::Value::Ref(name)),
        }
    } else {
        ResponseOrRef {
            value: Some(response_or_ref::Value::Inline(parse_response_def(val))),
        }
    }
}

fn parse_response_def(val: &Value) -> ResponseDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => {
            return ResponseDef {
                description: String::new(),
                content: vec![],
                headers: vec![],
                extensions: None,
            };
        }
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

    let extensions = extract_extensions(map);

    ResponseDef { description, content, headers, extensions }
}

fn parse_content(val: Option<&Value>) -> Vec<MediaTypeEntry> {
    val.and_then(|v| v.as_mapping())
        .map(|cm| {
            cm.iter()
                .map(|(k, v)| {
                    let media_type = k.as_str().unwrap_or("").to_owned();
                    let cmap = v.as_mapping();
                    let schema = cmap.and_then(|m| m.get("schema")).map(parse_schema_or_ref);
                    let extensions = cmap
                        .map(|m| extract_extensions(m))
                        .unwrap_or_default();
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
        SchemaOrRef {
            value: Some(schema_or_ref::Value::Ref(SchemaRef {
                local_name,
                external_import: None,
            })),
        }
    } else {
        SchemaOrRef {
            value: Some(schema_or_ref::Value::Inline(parse_schema_def(val))),
        }
    }
}

pub fn parse_schema_def(val: &Value) -> JsonSchemaDef {
    let map = match val.as_mapping() {
        Some(m) => m,
        None => return Default::default(),
    };

    // type: can be string or array
    let types: Vec<i32> = if let Some(type_val) = map.get("type") {
        if let Some(s) = type_val.as_str() {
            JsonSchemaType::from_str(s).map(|t| vec![t as i32]).unwrap_or_default()
        } else if let Some(seq) = type_val.as_sequence() {
            seq.iter()
                .filter_map(|t| t.as_str().and_then(JsonSchemaType::from_str).map(|t| t as i32))
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
                .map(|(k, v)| {
                    let name = k.as_str().unwrap_or("").to_owned();
                    let schema = Some(parse_schema_or_ref(v));
                    PropertyDef { name, schema }
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

    // additionalProperties
    let additional_properties_allowed;
    let additional_properties_schema;
    if let Some(ap) = map.get("additionalProperties") {
        if let Some(b) = ap.as_bool() {
            additional_properties_allowed = Some(b);
            additional_properties_schema = None;
        } else {
            additional_properties_allowed = None;
            additional_properties_schema = Some(Box::new(parse_schema_or_ref(ap)));
        }
    } else {
        additional_properties_allowed = None;
        additional_properties_schema = None;
    }

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

    let const_value = map
        .get("const")
        .map(|v| serde_json::to_string(v).unwrap_or_default());

    let default = map.get("default").map(|v| serde_json::to_string(v).unwrap_or_default());

    let extensions = extract_extensions(map);

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
        extensions,
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
        let json_bytes = serde_json::to_vec(&ext_map).unwrap_or_default();
        Some(Extensions { json_bytes })
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
