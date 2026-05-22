pub mod ast;
pub mod compat;
pub mod diff;
pub mod operations;
pub mod parser;
pub mod printer;

use std::collections::HashMap;

use bytes::Bytes;
use schemahub_types::{
    Blob, CompatibilityRules, CompatibilityViolation, DeclDetail, DeclKind, DeclSummary,
    DescriptorError, DiffError, FormatPlugin, Import, Language, Mutation, MutationError,
    ParseError, PrintError, ReadError, SchemaChange, SchemaPath,
    errors::CodegenError,
};

use crate::ast::{
    ParsedDeclaration,
    decode_component_parameter, decode_component_request_body, decode_component_response,
    decode_component_schema, decode_metadata, decode_parse_result, decode_path_item,
    encode_parse_result, schema_or_ref,
};
use crate::compat::check_compatibility as check_compat;
use crate::diff::diff_envelopes;
use crate::operations::decode_operation;
use crate::parser::parse_openapi;
use crate::printer::{print_declaration, print_envelope};

// ── OpenApiPlugin ─────────────────────────────────────────────────────────────

pub struct OpenApiPlugin;

impl FormatPlugin for OpenApiPlugin {
    fn format_id(&self) -> &'static str {
        "openapi"
    }

    // ── parse ─────────────────────────────────────────────────────────────────

    fn parse(&self, source: &str) -> Result<Blob, ParseError> {
        let result = parse_openapi(source)?;
        Ok(Blob::from(encode_parse_result(&result)))
    }

    // ── print ─────────────────────────────────────────────────────────────────

    fn print(&self, blob: &Blob) -> Result<String, PrintError> {
        let result = decode_parse_result(blob.as_bytes())
            .map_err(|e| PrintError::MalformedBlob(format!("OpenApiParseResult: {e}")))?;
        print_envelope(&result)
    }

    // ── diff ──────────────────────────────────────────────────────────────────

    fn diff(&self, old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError> {
        let old_result = decode_parse_result(old.as_bytes())
            .map_err(|e| DiffError::MalformedBlob(format!("old blob: {e}")))?;
        let new_result = decode_parse_result(new.as_bytes())
            .map_err(|e| DiffError::MalformedBlob(format!("new blob: {e}")))?;

        Ok(diff_envelopes(&old_result, &new_result))
    }

    // ── apply_mutation ────────────────────────────────────────────────────────

    fn apply_mutation(&self, blob: &Blob, mutation: &Mutation) -> Result<Blob, MutationError> {
        let op = decode_operation(&mutation.operation)?;

        // PushDocument: parse the new source and return as the new blob
        let source_str = std::str::from_utf8(&op.source)
            .map_err(|e| MutationError::InvalidOperation(format!("source is not valid UTF-8: {e}")))?;

        let result = parse_openapi(source_str)
            .map_err(|e| MutationError::InvalidOperation(format!("parse error: {e}")))?;

        Ok(Blob::from(encode_parse_result(&result)))
    }

    // ── apply_mutations ───────────────────────────────────────────────────────

    fn apply_mutations(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        mutations: &[Mutation],
    ) -> Result<HashMap<SchemaPath, Blob>, MutationError> {
        let mut working: HashMap<SchemaPath, Blob> = blobs.clone();
        let mut result: HashMap<SchemaPath, Blob> = HashMap::new();

        for mutation in mutations {
            let current = working
                .get(&mutation.schema_path)
                .cloned()
                .unwrap_or_else(|| Blob::from(vec![]));
            let new_blob = self.apply_mutation(&current, mutation)?;
            working.insert(mutation.schema_path.clone(), new_blob.clone());
            result.insert(mutation.schema_path.clone(), new_blob);
        }

        Ok(result)
    }

    // ── check_compatibility ───────────────────────────────────────────────────

    fn check_compatibility(
        &self,
        old: &Blob,
        new: &Blob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>> {
        let old_result = decode_parse_result(old.as_bytes()).map_err(|e| {
            vec![CompatibilityViolation {
                declaration_name: "unknown".into(),
                field_name: None,
                message: format!("malformed old blob: {e}"),
            }]
        })?;

        let new_result = decode_parse_result(new.as_bytes()).map_err(|e| {
            vec![CompatibilityViolation {
                declaration_name: "unknown".into(),
                field_name: None,
                message: format!("malformed new blob: {e}"),
            }]
        })?;

        let violations = check_compat(&old_result, &new_result, rules);

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }

    // ── list_declarations ─────────────────────────────────────────────────────

    fn list_declarations(&self, blob: &Blob) -> Result<Vec<DeclSummary>, ReadError> {
        let result = decode_parse_result(blob.as_bytes())
            .map_err(|e| ReadError::MalformedBlob(format!("OpenApiParseResult: {e}")))?;

        let mut summaries = Vec::new();

        for decl in &result.declarations {
            let summary = decl_summary_from_declaration(decl)?;
            summaries.push(summary);
        }

        Ok(summaries)
    }

    // ── get_declaration ───────────────────────────────────────────────────────

    fn get_declaration(&self, blob: &Blob, name: &str) -> Result<DeclDetail, ReadError> {
        let result = decode_parse_result(blob.as_bytes())
            .map_err(|e| ReadError::MalformedBlob(format!("OpenApiParseResult: {e}")))?;

        let decl = result
            .declarations
            .iter()
            .find(|d| d.tree_key == name)
            .ok_or_else(|| ReadError::NotFound(name.to_owned()))?;

        let text = print_declaration(decl)
            .map_err(|e| ReadError::MalformedBlob(format!("print error: {e}")))?;

        Ok(DeclDetail::new(text))
    }

    // ── imports ───────────────────────────────────────────────────────────────

    fn imports(&self, blob: &Blob) -> Result<Vec<Import>, ReadError> {
        let result = decode_parse_result(blob.as_bytes())
            .map_err(|e| ReadError::MalformedBlob(format!("OpenApiParseResult: {e}")))?;

        let mut imports = Vec::new();

        for decl in &result.declarations {
            collect_imports_from_decl(decl, &mut imports)?;
        }

        // Deduplicate
        imports.dedup_by(|a, b| a.path == b.path && a.decl_name == b.decl_name);

        Ok(imports)
    }

    // ── generate_descriptors ──────────────────────────────────────────────────

    fn generate_descriptors(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
    ) -> Result<Bytes, DescriptorError> {
        // For a single schema (the common case) return its full YAML.
        // For multiple schemas (cross-file $ref), concatenate their YAML documents
        // separated by "---" so the result is a valid multi-document YAML stream.
        let mut sorted: Vec<(&SchemaPath, &Blob)> = blobs.iter().collect();
        sorted.sort_by_key(|(path, _)| path.schema_name.as_str());

        let mut out = String::new();
        for (_, blob) in sorted {
            let result = decode_parse_result(blob.as_bytes())
                .map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?;
            let yaml = print_envelope(&result)
                .map_err(|e| DescriptorError::Other(e.to_string()))?;
            if !out.is_empty() {
                out.push_str("---\n");
            }
            out.push_str(&yaml);
        }

        Ok(Bytes::from(out.into_bytes()))
    }

    // ── generate_code ─────────────────────────────────────────────────────────

    fn generate_code(
        &self,
        _blobs: &HashMap<SchemaPath, Blob>,
        language: Language,
    ) -> Result<String, CodegenError> {
        Err(CodegenError::UnsupportedLanguage(language))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn decl_summary_from_declaration(decl: &ParsedDeclaration) -> Result<DeclSummary, ReadError> {
    let key = &decl.tree_key;

    if key == "__metadata__" {
        let meta = decode_metadata(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("metadata: {e}")))?;
        let doc = meta.info.as_ref().map(|i| i.title.clone()).unwrap_or_default();
        return Ok(DeclSummary {
            name: key.clone(),
            kind: DeclKind::DocumentMetadata,
            doc_comment: doc,
        });
    }

    if let Some(_path) = key.strip_prefix("path:") {
        let blob = decode_path_item(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("path item: {e}")))?;
        let doc = blob.summary.unwrap_or_default();
        return Ok(DeclSummary {
            name: key.clone(),
            kind: DeclKind::PathItem,
            doc_comment: doc,
        });
    }

    if let Some(_name) = key.strip_prefix("schema:") {
        let blob = decode_component_schema(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("schema: {e}")))?;
        let doc = blob
            .schema
            .as_ref()
            .and_then(|s| s.description.clone())
            .unwrap_or_default();
        return Ok(DeclSummary {
            name: key.clone(),
            kind: DeclKind::ComponentSchema,
            doc_comment: doc,
        });
    }

    if let Some(_name) = key.strip_prefix("param:") {
        let blob = decode_component_parameter(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("param: {e}")))?;
        let doc = blob
            .parameter
            .as_ref()
            .and_then(|p| p.description.clone())
            .unwrap_or_default();
        return Ok(DeclSummary {
            name: key.clone(),
            kind: DeclKind::ComponentParameter,
            doc_comment: doc,
        });
    }

    if let Some(_name) = key.strip_prefix("response:") {
        let blob = decode_component_response(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("response: {e}")))?;
        let doc = blob.response.as_ref().map(|r| r.description.clone()).unwrap_or_default();
        return Ok(DeclSummary {
            name: key.clone(),
            kind: DeclKind::ComponentResponse,
            doc_comment: doc,
        });
    }

    if let Some(_name) = key.strip_prefix("requestBody:") {
        let blob = decode_component_request_body(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("requestBody: {e}")))?;
        let doc = blob
            .request_body
            .as_ref()
            .and_then(|b| b.description.clone())
            .unwrap_or_default();
        return Ok(DeclSummary {
            name: key.clone(),
            kind: DeclKind::ComponentRequestBody,
            doc_comment: doc,
        });
    }

    Err(ReadError::MalformedBlob(format!("unknown declaration key prefix: {key}")))
}

fn collect_imports_from_decl(
    decl: &ParsedDeclaration,
    imports: &mut Vec<Import>,
) -> Result<(), ReadError> {
    let key = &decl.tree_key;

    if key == "__metadata__" {
        return Ok(());
    }

    if key.starts_with("path:") {
        let blob = decode_path_item(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("path item: {e}")))?;
        for param in &blob.parameters {
            if let Some(crate::ast::parameter_or_ref::Value::Inline(p)) = &param.value {
                if let Some(schema) = &p.schema {
                    collect_imports_from_schema_or_ref(schema, imports);
                }
            }
        }
        for op in &blob.operations {
            for param in &op.parameters {
                if let Some(crate::ast::parameter_or_ref::Value::Inline(p)) = &param.value {
                    if let Some(schema) = &p.schema {
                        collect_imports_from_schema_or_ref(schema, imports);
                    }
                }
            }
            if let Some(rb) = &op.request_body {
                if let Some(crate::ast::request_body_or_ref::Value::Inline(b)) = &rb.value {
                    for entry in &b.content {
                        if let Some(schema) = &entry.schema {
                            collect_imports_from_schema_or_ref(schema, imports);
                        }
                    }
                }
            }
            for resp_entry in &op.responses {
                if let Some(r) = &resp_entry.response {
                    if let Some(crate::ast::response_or_ref::Value::Inline(rd)) = &r.value {
                        for entry in &rd.content {
                            if let Some(schema) = &entry.schema {
                                collect_imports_from_schema_or_ref(schema, imports);
                            }
                        }
                    }
                }
            }
        }
    }

    if key.starts_with("schema:") {
        let blob = decode_component_schema(&decl.blob_bytes)
            .map_err(|e| ReadError::MalformedBlob(format!("schema: {e}")))?;
        if let Some(schema) = &blob.schema {
            collect_imports_from_schema_def(schema, imports);
        }
    }

    Ok(())
}

fn collect_imports_from_schema_or_ref(
    s: &crate::ast::SchemaOrRef,
    imports: &mut Vec<Import>,
) {
    match &s.value {
        Some(schema_or_ref::Value::Ref(r)) => {
            if let Some(ext) = &r.external_import {
                imports.push(Import {
                    path: ext.path.clone(),
                    resolved_commit: ext.resolved_commit.clone(),
                    decl_name: ext.decl_name.clone(),
                });
            }
        }
        Some(schema_or_ref::Value::Inline(def)) => {
            collect_imports_from_schema_def(def, imports);
        }
        None => {}
    }
}

fn collect_imports_from_schema_def(
    schema: &crate::ast::JsonSchemaDef,
    imports: &mut Vec<Import>,
) {
    if let Some(items) = &schema.items {
        collect_imports_from_schema_or_ref(items, imports);
    }
    for prop in &schema.properties {
        if let Some(s) = &prop.schema {
            collect_imports_from_schema_or_ref(s, imports);
        }
    }
    for s in &schema.all_of {
        collect_imports_from_schema_or_ref(s, imports);
    }
    for s in &schema.any_of {
        collect_imports_from_schema_or_ref(s, imports);
    }
    for s in &schema.one_of {
        collect_imports_from_schema_or_ref(s, imports);
    }
    if let Some(not) = &schema.not {
        collect_imports_from_schema_or_ref(not, imports);
    }
    if let Some(ap) = &schema.additional_properties_schema {
        collect_imports_from_schema_or_ref(ap, imports);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use schemahub_types::{CompatibilityDirection, CompatibilityRules};

    const MINIMAL_DOC: &str = r#"
openapi: "3.1.0"
info:
  title: "Test API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      summary: List all users
      responses:
        '200':
          description: Success
"#;

    const FULL_DOC: &str = r#"
openapi: "3.1.0"
info:
  title: "Full API"
  version: "2.0.0"
  description: "A full API example"
servers:
  - url: "https://api.example.com"
    description: "Production"
paths:
  /users:
    get:
      operationId: listUsers
      summary: List users
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
      responses:
        '200':
          description: Users list
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/User'
    post:
      operationId: createUser
      summary: Create a user
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/User'
      responses:
        '201':
          description: Created
  /users/{id}:
    get:
      operationId: getUser
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
      responses:
        '200':
          description: User found
        '404':
          $ref: '#/components/responses/NotFound'
components:
  schemas:
    User:
      type: object
      description: A user object
      properties:
        id:
          type: string
          format: uuid
        name:
          type: string
        email:
          type: string
          format: email
      required:
        - id
        - name
    Error:
      type: object
      properties:
        code:
          type: integer
        message:
          type: string
  parameters:
    PageSize:
      name: pageSize
      in: query
      description: Number of results per page
      required: false
      schema:
        type: integer
  responses:
    NotFound:
      description: Resource not found
      content:
        application/json:
          schema:
            $ref: '#/components/schemas/Error'
  requestBodies:
    CreateUser:
      required: true
      content:
        application/json:
          schema:
            $ref: '#/components/schemas/User'
"#;

    // Test 1: Parse a minimal OpenAPI document
    #[test]
    fn test_parse_minimal_document() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(MINIMAL_DOC).unwrap();
        assert!(!blob.as_bytes().is_empty());
    }

    // Test 2: Parse returns expected tree keys for minimal doc
    #[test]
    fn test_parse_minimal_tree_keys() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(MINIMAL_DOC).unwrap();
        let result = decode_parse_result(blob.as_bytes()).unwrap();
        let keys: Vec<&str> = result.declarations.iter().map(|d| d.tree_key.as_str()).collect();
        assert!(keys.contains(&"__metadata__"), "missing __metadata__: {keys:?}");
        assert!(keys.contains(&"path:/users"), "missing path:/users: {keys:?}");
    }

    // Test 3: Parse a document with components (schemas, parameters, responses)
    #[test]
    fn test_parse_full_document_with_components() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let result = decode_parse_result(blob.as_bytes()).unwrap();
        let keys: Vec<&str> = result.declarations.iter().map(|d| d.tree_key.as_str()).collect();

        assert!(keys.contains(&"__metadata__"), "missing __metadata__");
        assert!(keys.contains(&"path:/users"), "missing path:/users");
        assert!(keys.contains(&"path:/users/{id}"), "missing path:/users/{{id}}");
        assert!(keys.contains(&"schema:User"), "missing schema:User");
        assert!(keys.contains(&"schema:Error"), "missing schema:Error");
        assert!(keys.contains(&"param:PageSize"), "missing param:PageSize");
        assert!(keys.contains(&"response:NotFound"), "missing response:NotFound");
        assert!(keys.contains(&"requestBody:CreateUser"), "missing requestBody:CreateUser");
    }

    // Test 4: list_declarations returns all expected tree keys
    #[test]
    fn test_list_declarations_returns_all() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let summaries = plugin.list_declarations(&blob).unwrap();
        let names: Vec<&str> = summaries.iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"__metadata__"), "missing __metadata__: {names:?}");
        assert!(names.contains(&"path:/users"), "missing path:/users: {names:?}");
        assert!(names.contains(&"schema:User"), "missing schema:User: {names:?}");
        assert!(names.contains(&"param:PageSize"), "missing param:PageSize: {names:?}");
        assert!(names.contains(&"response:NotFound"), "missing response:NotFound: {names:?}");
        assert!(names.contains(&"requestBody:CreateUser"), "missing requestBody:CreateUser: {names:?}");
    }

    // Test 5: list_declarations returns correct DeclKind values
    #[test]
    fn test_list_declarations_kinds() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let summaries = plugin.list_declarations(&blob).unwrap();

        let meta = summaries.iter().find(|s| s.name == "__metadata__").unwrap();
        assert_eq!(meta.kind, DeclKind::DocumentMetadata);

        let path = summaries.iter().find(|s| s.name == "path:/users").unwrap();
        assert_eq!(path.kind, DeclKind::PathItem);

        let schema = summaries.iter().find(|s| s.name == "schema:User").unwrap();
        assert_eq!(schema.kind, DeclKind::ComponentSchema);

        let param = summaries.iter().find(|s| s.name == "param:PageSize").unwrap();
        assert_eq!(param.kind, DeclKind::ComponentParameter);

        let response = summaries.iter().find(|s| s.name == "response:NotFound").unwrap();
        assert_eq!(response.kind, DeclKind::ComponentResponse);

        let req_body = summaries.iter().find(|s| s.name == "requestBody:CreateUser").unwrap();
        assert_eq!(req_body.kind, DeclKind::ComponentRequestBody);
    }

    // Test 6: get_declaration retrieves a PathItem by tree key
    #[test]
    fn test_get_declaration_path_item() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let detail = plugin.get_declaration(&blob, "path:/users").unwrap();
        let text = std::str::from_utf8(detail.as_bytes()).unwrap();
        assert!(text.contains("/users"), "expected /users in: {text}");
    }

    // Test 7: get_declaration retrieves a schema
    #[test]
    fn test_get_declaration_schema() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let detail = plugin.get_declaration(&blob, "schema:User").unwrap();
        let text = std::str::from_utf8(detail.as_bytes()).unwrap();
        assert!(text.contains("User"), "expected User in: {text}");
    }

    // Test 8: diff detects added declarations
    #[test]
    fn test_diff_detects_added_declaration() {
        let plugin = OpenApiPlugin;
        let old_blob = plugin.parse(MINIMAL_DOC).unwrap();
        let new_blob = plugin.parse(FULL_DOC).unwrap();
        let changes = plugin.diff(&old_blob, &new_blob).unwrap();
        let names: Vec<&str> = changes.iter().map(|c| c.declaration_name()).collect();
        // FULL_DOC has more paths and components, so some should be added
        let added: Vec<_> = changes.iter().filter(|c| matches!(c, SchemaChange::DeclarationAdded { .. })).collect();
        assert!(!added.is_empty(), "expected some added declarations: {names:?}");
    }

    // Test 9: diff detects removed declarations
    #[test]
    fn test_diff_detects_removed_declaration() {
        let plugin = OpenApiPlugin;
        let old_blob = plugin.parse(FULL_DOC).unwrap();
        let new_blob = plugin.parse(MINIMAL_DOC).unwrap();
        let changes = plugin.diff(&old_blob, &new_blob).unwrap();
        let removed: Vec<_> = changes
            .iter()
            .filter(|c| matches!(c, SchemaChange::DeclarationRemoved { .. }))
            .collect();
        assert!(!removed.is_empty(), "expected some removed declarations");
    }

    // Test 10: apply_mutation with PushDocument replaces the envelope
    #[test]
    fn test_apply_mutation_push_document() {
        use crate::operations::encode_push_document;
        use crate::operations::PushDocumentOp;
        use schemahub_types::Mutation;
        use bytes::Bytes;

        let plugin = OpenApiPlugin;
        let old_blob = plugin.parse(MINIMAL_DOC).unwrap();

        let op = PushDocumentOp {
            source: FULL_DOC.as_bytes().to_vec(),
        };
        let op_bytes = encode_push_document(&op);

        let mutation = Mutation {
            schema_path: SchemaPath::new("test", "repo", "api.yaml"),
            format_id: "openapi".into(),
            declaration_name: "__root__".into(),
            operation: Bytes::from(op_bytes),
        };

        let new_blob = plugin.apply_mutation(&old_blob, &mutation).unwrap();
        // The new blob should have more declarations than the minimal
        let result = decode_parse_result(new_blob.as_bytes()).unwrap();
        assert!(result.declarations.len() > 2, "expected more than 2 declarations after push");
    }

    // Test 11: Compatibility check: adding new path is BACKWARD compat OK
    #[test]
    fn test_compat_add_path_backward_ok() {
        let plugin = OpenApiPlugin;
        let old_blob = plugin.parse(MINIMAL_DOC).unwrap();
        let new_blob = plugin.parse(FULL_DOC).unwrap();

        let rules = CompatibilityRules { direction: CompatibilityDirection::Backward };
        // Adding new paths is backward compatible
        let result = plugin.check_compatibility(&old_blob, &new_blob, &rules);
        // May or may not have violations depending on whether content changed
        // The key assertion: adding path:/users/{id} should not be a backward violation
        if let Err(violations) = &result {
            let path_violations: Vec<_> = violations
                .iter()
                .filter(|v| v.declaration_name == "path:/users/{id}")
                .collect();
            assert!(path_violations.is_empty(),
                "adding path:/users/{{id}} should not violate BACKWARD compat: {:?}", path_violations);
        }
    }

    // Test 12: Compatibility check: adding new path violates FORWARD
    #[test]
    fn test_compat_add_path_violates_forward() {
        let plugin = OpenApiPlugin;
        let old_blob = plugin.parse(MINIMAL_DOC).unwrap();

        let doc_with_new_path = r#"
openapi: "3.1.0"
info:
  title: "Test API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      summary: List all users
      responses:
        '200':
          description: Success
  /orders:
    get:
      operationId: listOrders
      responses:
        '200':
          description: Orders
"#;

        let new_blob = plugin.parse(doc_with_new_path).unwrap();
        let rules = CompatibilityRules { direction: CompatibilityDirection::Forward };
        let result = plugin.check_compatibility(&old_blob, &new_blob, &rules);
        assert!(result.is_err(), "adding new path should violate FORWARD compat");
        if let Err(violations) = result {
            let names: Vec<&str> = violations.iter().map(|v| v.declaration_name.as_str()).collect();
            assert!(names.contains(&"path:/orders"), "expected path:/orders in violations: {names:?}");
        }
    }

    // Test 13: Compatibility check: removing a path violates BACKWARD
    #[test]
    fn test_compat_remove_path_violates_backward() {
        let plugin = OpenApiPlugin;
        let old_blob = plugin.parse(FULL_DOC).unwrap();
        let new_blob = plugin.parse(MINIMAL_DOC).unwrap();

        let rules = CompatibilityRules { direction: CompatibilityDirection::Backward };
        let result = plugin.check_compatibility(&old_blob, &new_blob, &rules);
        assert!(result.is_err(), "removing paths should violate BACKWARD compat");
        if let Err(violations) = result {
            let names: Vec<&str> = violations.iter().map(|v| v.declaration_name.as_str()).collect();
            assert!(names.iter().any(|n| n.starts_with("path:")),
                "expected path removal in violations: {names:?}");
        }
    }

    // Test 14: imports() returns empty for a document with no external $refs
    #[test]
    fn test_imports_empty_for_local_refs_only() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let imports = plugin.imports(&blob).unwrap();
        // FULL_DOC uses only local $refs (#/components/...), so no external imports
        assert!(imports.is_empty(), "expected no external imports, got: {imports:?}");
    }

    // Test 15: Round-trip: parse → list_declarations → tree keys match
    #[test]
    fn test_roundtrip_parse_list_keys() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let summaries = plugin.list_declarations(&blob).unwrap();

        let result = decode_parse_result(blob.as_bytes()).unwrap();
        let raw_keys: std::collections::HashSet<&str> =
            result.declarations.iter().map(|d| d.tree_key.as_str()).collect();
        let summary_keys: std::collections::HashSet<&str> =
            summaries.iter().map(|s| s.name.as_str()).collect();

        assert_eq!(raw_keys, summary_keys, "raw parse keys don't match list_declarations keys");
    }

    // Test 16: Parse rejects non-OpenAPI 3.x documents
    #[test]
    fn test_parse_rejects_swagger_2() {
        let plugin = OpenApiPlugin;
        let swagger_doc = r#"
swagger: "2.0"
info:
  title: "Old API"
  version: "1.0"
paths: {}
"#;
        // This should either error (missing 'openapi' field) or not, depending on whether
        // serde_yaml parses it. Either way it should fail or produce a weird result.
        // With our parser, it fails because 'openapi' field is missing.
        let result = plugin.parse(swagger_doc);
        assert!(result.is_err(), "should reject swagger 2.0 document");
    }

    // Test 17: Parse preserves operation metadata
    #[test]
    fn test_parse_preserves_operation_metadata() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let result = decode_parse_result(blob.as_bytes()).unwrap();

        let path_decl = result.declarations.iter().find(|d| d.tree_key == "path:/users").unwrap();
        let path_blob = decode_path_item(&path_decl.blob_bytes).unwrap();

        assert!(!path_blob.operations.is_empty(), "path item should have operations");
        let get_op = path_blob.operations.iter().find(|o| o.method == 0 /* Get */).unwrap();
        assert_eq!(get_op.operation_id.as_deref(), Some("listUsers"));
        assert_eq!(get_op.summary.as_deref(), Some("List users"));
    }

    // Test 18: get_declaration returns NotFound for missing key
    #[test]
    fn test_get_declaration_not_found() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(MINIMAL_DOC).unwrap();
        let result = plugin.get_declaration(&blob, "schema:NonExistent");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ReadError::NotFound(_)));
    }

    // Test 19: Parse preserves schema component fields
    #[test]
    fn test_parse_preserves_schema_fields() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(FULL_DOC).unwrap();
        let result = decode_parse_result(blob.as_bytes()).unwrap();

        let schema_decl = result.declarations.iter().find(|d| d.tree_key == "schema:User").unwrap();
        let schema_blob = decode_component_schema(&schema_decl.blob_bytes).unwrap();
        assert_eq!(schema_blob.name, "User");
        let schema = schema_blob.schema.unwrap();
        assert!(!schema.properties.is_empty(), "User schema should have properties");
        let prop_names: Vec<&str> = schema.properties.iter().map(|p| p.name.as_str()).collect();
        assert!(prop_names.contains(&"id"), "missing 'id' property");
        assert!(prop_names.contains(&"name"), "missing 'name' property");
        assert!(prop_names.contains(&"email"), "missing 'email' property");
    }

    // Test 20: Diff of identical documents returns no changes
    #[test]
    fn test_diff_identical_documents() {
        let plugin = OpenApiPlugin;
        let blob = plugin.parse(MINIMAL_DOC).unwrap();
        let changes = plugin.diff(&blob, &blob).unwrap();
        assert!(changes.is_empty(), "identical blobs should produce no changes: {changes:?}");
    }
}
