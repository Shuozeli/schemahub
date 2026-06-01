//! Tests for the OpenAPI compiler. Arrange-Act-Assert throughout.

use std::collections::BTreeMap;

use bytes::Bytes;
use schemahub_types::{
    CompatibilityDirection, CompatibilityRules, Compiler, ConflictSides, DeclBlob, DeclChange,
    DeclKind, Language, Mutation, SchemaClosure, SchemaObjects, SchemaPath,
};

use super::*;
use crate::operations::OpenApiOp;

const MINIMAL: &str = r#"
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

const FULL: &str = r#"
openapi: "3.1.0"
info:
  title: "Full API"
  version: "2.0.0"
  description: "A full API example"
servers:
  - url: "https://api.example.com"
    description: Production
paths:
  /users:
    get:
      operationId: listUsers
      summary: List users
      tags:
        - users
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

/// Reassemble a [`ParsedSchema`] into a [`SchemaObjects`] (mirrors what the VCS
/// layer does when loading a schema from storage).
fn to_objects(parsed: ParsedSchema) -> SchemaObjects {
    let mut decls = BTreeMap::new();
    for (k, v) in parsed.decls {
        decls.insert(k, v);
    }
    SchemaObjects {
        meta: parsed.meta,
        decls,
    }
}

fn make_mutation(op: &OpenApiOp) -> Mutation {
    Mutation {
        schema_path: SchemaPath::new("proj", "repo", "api.yaml"),
        format_id: "openapi".into(),
        operation: Bytes::from(op.encode()),
    }
}

// ── P0: parse + split ─────────────────────────────────────────────────────────

#[test]
fn parse_minimal_produces_metadata_and_path_decl() {
    // Arrange
    let compiler = OpenApiCompiler::new();

    // Act
    let parsed = compiler.parse(MINIMAL).unwrap();

    // Assert
    assert!(!parsed.meta.is_empty(), "metadata blob must be populated");
    let keys: Vec<&str> = parsed.decls.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        keys.contains(&"path:/users"),
        "missing path:/users: {keys:?}"
    );
    assert_eq!(
        keys.len(),
        1,
        "minimal doc should have exactly one decl: {keys:?}"
    );
}

#[test]
fn parse_full_splits_into_per_declaration_keys() {
    // Arrange
    let compiler = OpenApiCompiler::new();

    // Act
    let parsed = compiler.parse(FULL).unwrap();

    // Assert
    let keys: Vec<&str> = parsed.decls.iter().map(|(k, _)| k.as_str()).collect();
    for expected in [
        "path:/users",
        "path:/users/{id}",
        "schema:User",
        "schema:Error",
        "param:PageSize",
        "response:NotFound",
        "requestBody:CreateUser",
    ] {
        assert!(keys.contains(&expected), "missing {expected}: {keys:?}");
    }
}

#[test]
fn parse_rejects_swagger_2() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let swagger = "swagger: \"2.0\"\ninfo:\n  title: Old\n  version: \"1.0\"\npaths: {}\n";

    // Act
    let result = compiler.parse(swagger);

    // Assert
    assert!(result.is_err(), "must reject non-3.x documents");
}

// ── P0: round-trip ──────────────────────────────────────────────────────────

#[test]
fn round_trip_full_document_is_equivalent() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let parsed_a = compiler.parse(FULL).unwrap();
    let objects_a = to_objects(parsed_a);

    // Act: parse -> split -> reassemble -> print -> parse
    let printed = compiler.print(&objects_a).unwrap();
    let parsed_b = compiler.parse(&printed).unwrap();
    let objects_b = to_objects(parsed_b);

    // Assert: the two decl sets are byte-identical (stable serialization).
    assert_eq!(
        objects_a.meta.as_bytes(),
        objects_b.meta.as_bytes(),
        "metadata must round-trip identically"
    );
    let keys_a: Vec<&String> = objects_a.decls.keys().collect();
    let keys_b: Vec<&String> = objects_b.decls.keys().collect();
    assert_eq!(keys_a, keys_b, "decl keys must round-trip");
    for (k, blob_a) in &objects_a.decls {
        let blob_b = objects_b
            .decls
            .get(k)
            .expect("decl present after round-trip");
        assert_eq!(
            blob_a.as_bytes(),
            blob_b.as_bytes(),
            "decl '{k}' must round-trip byte-identically"
        );
    }
}

#[test]
fn round_trip_preserves_refs_request_body_and_responses() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());

    // Act
    let printed = compiler.print(&objects).unwrap();

    // Assert: canonical $ref forms and key structures survive.
    assert!(
        printed.contains("$ref: '#/components/schemas/User'"),
        "schema $ref\n{printed}"
    );
    assert!(
        printed.contains("$ref: '#/components/responses/NotFound'"),
        "response $ref\n{printed}"
    );
    assert!(
        printed.contains("requestBody:"),
        "requestBody in operation\n{printed}"
    );
    assert!(
        printed.contains("\"404\":"),
        "status code quoted\n{printed}"
    );
    assert!(
        printed.contains("- name: id"),
        "path param inline form\n{printed}"
    );
}

#[test]
fn print_minimal_omits_absent_sections() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());

    // Act
    let printed = compiler.print(&objects).unwrap();

    // Assert
    assert!(printed.contains("paths:"), "paths present\n{printed}");
    assert!(
        !printed.contains("components:"),
        "components must be absent\n{printed}"
    );
}

// ── P1: read / explore ────────────────────────────────────────────────────────

#[test]
fn summarize_decl_reports_kind_and_doc() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let schema_blob = objects.decls.get("schema:User").unwrap();

    // Act
    let summary = compiler.summarize_decl(schema_blob).unwrap();

    // Assert
    assert_eq!(summary.name, "schema:User");
    assert_eq!(summary.kind, DeclKind::ComponentSchema);
    assert_eq!(summary.doc_comment, "A user object");
}

#[test]
fn summarize_decl_classifies_path_item() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let path_blob = objects.decls.get("path:/users").unwrap();

    // Act
    let summary = compiler.summarize_decl(path_blob).unwrap();

    // Assert
    assert_eq!(summary.name, "path:/users");
    assert_eq!(summary.kind, DeclKind::PathItem);
}

#[test]
fn decl_detail_renders_named_declaration() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let path_blob = objects.decls.get("path:/users").unwrap();

    // Act
    let detail = compiler.decl_detail(path_blob).unwrap();

    // Assert
    let text = std::str::from_utf8(detail.as_bytes()).unwrap();
    assert!(
        text.contains("/users"),
        "detail should mention path\n{text}"
    );
    assert!(
        text.contains("operationId: listUsers"),
        "detail should mention op\n{text}"
    );
}

#[test]
fn imports_on_metadata_is_empty() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let parsed = compiler.parse(FULL).unwrap();

    // Act
    let imports = compiler.imports(&parsed.meta).unwrap();

    // Assert: external $ref imports are v2-modeled; none at document level.
    assert!(
        imports.is_empty(),
        "expected no document-level imports: {imports:?}"
    );
}

#[test]
fn type_refs_lists_local_schema_references() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let path_blob = objects.decls.get("path:/users").unwrap();

    // Act
    let refs = compiler.type_refs(path_blob).unwrap();

    // Assert: /users references schema:User (response + requestBody).
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"schema:User"),
        "expected schema:User ref: {names:?}"
    );
}

#[test]
fn type_refs_resolves_response_ref() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let path_blob = objects.decls.get("path:/users/{id}").unwrap();

    // Act
    let refs = compiler.type_refs(path_blob).unwrap();

    // Assert: /users/{id} GET 404 references response:NotFound.
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"response:NotFound"),
        "expected response:NotFound: {names:?}"
    );
}

// ── P1: diff ──────────────────────────────────────────────────────────────────

#[test]
fn diff_decl_detects_modified_path() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let one_op = to_objects(compiler.parse(MINIMAL).unwrap());
    // Add a POST to /users to produce a modified path-item blob.
    let effect = compiler
        .apply_mutation(
            &one_op,
            &make_mutation(&OpenApiOp::AddOperation {
                path_pattern: "/users".into(),
                method: "post".into(),
                operation_id: "createUser".into(),
                summary: String::new(),
                description: String::new(),
            }),
        )
        .unwrap();
    let new_blob = &effect.upserts[0].1;
    let old_blob = one_op.decls.get("path:/users").unwrap();

    // Act
    let change = compiler.diff_decl(old_blob, new_blob).unwrap();

    // Assert
    match change {
        DeclChange::DeclarationModified { name, .. } => assert_eq!(name, "path:/users"),
        other => panic!("expected DeclarationModified, got {other:?}"),
    }
}

// ── P2: compatibility ─────────────────────────────────────────────────────────

fn backward() -> CompatibilityRules {
    CompatibilityRules {
        direction: CompatibilityDirection::Backward,
        disabled: false,
    }
}
fn forward() -> CompatibilityRules {
    CompatibilityRules {
        direction: CompatibilityDirection::Forward,
        disabled: false,
    }
}

/// Build a single path-item DeclBlob from a one-path OpenAPI document.
fn path_blob(doc: &str, key: &str) -> DeclBlob {
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(doc).unwrap());
    objects.decls.get(key).cloned().expect("path decl present")
}

#[test]
fn compat_add_optional_parameter_is_compatible() {
    // Arrange
    let old = path_blob(MINIMAL, "path:/users");
    let with_optional = r#"
openapi: "3.1.0"
info:
  title: "Test API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
      responses:
        '200':
          description: Success
"#;
    let new = path_blob(with_optional, "path:/users");
    let compiler = OpenApiCompiler::new();

    // Act
    let result = compiler.check_compatibility(&old, &new, &forward());

    // Assert
    assert!(
        result.is_ok(),
        "adding optional parameter must be compatible: {result:?}"
    );
}

#[test]
fn compat_add_required_parameter_breaks_forward() {
    // Arrange
    let old = path_blob(MINIMAL, "path:/users");
    let with_required = r#"
openapi: "3.1.0"
info:
  title: "Test API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      parameters:
        - name: tenant
          in: query
          required: true
          schema:
            type: string
      responses:
        '200':
          description: Success
"#;
    let new = path_blob(with_required, "path:/users");
    let compiler = OpenApiCompiler::new();

    // Act
    let result = compiler.check_compatibility(&old, &new, &forward());

    // Assert
    assert!(
        result.is_err(),
        "adding required parameter must break FORWARD"
    );
}

#[test]
fn compat_add_operation_breaks_forward() {
    // Arrange: old has GET only; new adds POST.
    let old = path_blob(MINIMAL, "path:/users");
    let with_post = r#"
openapi: "3.1.0"
info:
  title: "Test API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      responses:
        '200':
          description: Success
    post:
      operationId: createUser
      responses:
        '201':
          description: Created
"#;
    let new = path_blob(with_post, "path:/users");
    let compiler = OpenApiCompiler::new();

    // Act
    let result = compiler.check_compatibility(&old, &new, &forward());

    // Assert
    assert!(result.is_err(), "adding an operation must break FORWARD");
}

#[test]
fn compat_remove_operation_breaks_backward() {
    // Arrange: old has GET+POST; new drops POST.
    let with_post = r#"
openapi: "3.1.0"
info:
  title: "Test API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      responses:
        '200':
          description: Success
    post:
      operationId: createUser
      responses:
        '201':
          description: Created
"#;
    let old = path_blob(with_post, "path:/users");
    let new = path_blob(MINIMAL, "path:/users");
    let compiler = OpenApiCompiler::new();

    // Act
    let result = compiler.check_compatibility(&old, &new, &backward());

    // Assert
    assert!(result.is_err(), "removing an operation must break BACKWARD");
}

#[test]
fn compat_change_property_type_breaks_all_directions() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let v1 = r#"
openapi: "3.1.0"
info:
  title: "T"
  version: "1.0.0"
components:
  schemas:
    User:
      type: object
      properties:
        id:
          type: string
"#;
    let v2 = r#"
openapi: "3.1.0"
info:
  title: "T"
  version: "1.0.0"
components:
  schemas:
    User:
      type: object
      properties:
        id:
          type: integer
"#;
    let old = to_objects(compiler.parse(v1).unwrap())
        .decls
        .get("schema:User")
        .cloned()
        .unwrap();
    let new = to_objects(compiler.parse(v2).unwrap())
        .decls
        .get("schema:User")
        .cloned()
        .unwrap();

    // Act
    let result = compiler.check_compatibility(&old, &new, &backward());

    // Assert
    assert!(
        result.is_err(),
        "changing a property type must break compatibility"
    );
}

#[test]
fn compat_disabled_rules_allow_anything() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let with_post = r#"
openapi: "3.1.0"
info:
  title: "Test API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      responses:
        '200':
          description: Success
    post:
      operationId: createUser
      responses:
        '201':
          description: Created
"#;
    let old = path_blob(MINIMAL, "path:/users");
    let new = path_blob(with_post, "path:/users");
    let rules = CompatibilityRules {
        direction: CompatibilityDirection::Disabled,
        disabled: true,
    };

    // Act
    let result = compiler.check_compatibility(&old, &new, &rules);

    // Assert
    assert!(result.is_ok(), "disabled rules must permit any change");
}

// ── P3: mutations ─────────────────────────────────────────────────────────────

#[test]
fn mutation_push_document_replaces_decls() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::PushDocument {
        source: FULL.to_string(),
    };

    // Act
    let effect = compiler
        .apply_mutation(&objects, &make_mutation(&op))
        .unwrap();

    // Assert: replaces meta + upserts the full decl set.
    assert!(effect.meta.is_some(), "PushDocument must produce new meta");
    let upsert_keys: Vec<&str> = effect.upserts.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        upsert_keys.contains(&"schema:User"),
        "expected schema:User upsert: {upsert_keys:?}"
    );
    assert!(
        upsert_keys.contains(&"path:/users/{id}"),
        "expected new path: {upsert_keys:?}"
    );
}

#[test]
fn mutation_add_path_upserts_new_path() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::AddPath {
        path_pattern: "/orders".into(),
        summary: "Orders".into(),
        description: String::new(),
    };

    // Act
    let effect = compiler
        .apply_mutation(&objects, &make_mutation(&op))
        .unwrap();

    // Assert
    assert_eq!(effect.upserts.len(), 1);
    assert_eq!(effect.upserts[0].0, "path:/orders");
}

#[test]
fn mutation_add_path_rejects_duplicate() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::AddPath {
        path_pattern: "/users".into(),
        summary: String::new(),
        description: String::new(),
    };

    // Act
    let result = compiler.apply_mutation(&objects, &make_mutation(&op));

    // Assert
    assert!(result.is_err(), "must reject duplicate path");
}

#[test]
fn mutation_remove_path_emits_remove() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::RemovePath {
        path_pattern: "/users".into(),
    };

    // Act
    let effect = compiler
        .apply_mutation(&objects, &make_mutation(&op))
        .unwrap();

    // Assert
    assert_eq!(effect.removes, vec!["path:/users".to_string()]);
}

#[test]
fn mutation_remove_path_not_found_errors() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::RemovePath {
        path_pattern: "/missing".into(),
    };

    // Act
    let result = compiler.apply_mutation(&objects, &make_mutation(&op));

    // Assert
    assert!(result.is_err(), "removing a missing path must error");
}

#[test]
fn mutation_add_operation_appends_method() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::AddOperation {
        path_pattern: "/users".into(),
        method: "post".into(),
        operation_id: "createUser".into(),
        summary: String::new(),
        description: String::new(),
    };

    // Act
    let effect = compiler
        .apply_mutation(&objects, &make_mutation(&op))
        .unwrap();

    // Assert: the path:/users blob is upserted with a POST operation.
    let (key, blob) = &effect.upserts[0];
    assert_eq!(key, "path:/users");
    let decl = crate::blob::decode_decl(blob).unwrap();
    if let crate::ast::DeclPayload::PathItem(p) = decl.kind {
        let methods: Vec<&str> = p.operations.iter().map(|o| o.method.to_str()).collect();
        assert!(methods.contains(&"post"), "expected post: {methods:?}");
    } else {
        panic!("expected PathItem payload");
    }
}

#[test]
fn mutation_add_operation_rejects_unknown_method() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::AddOperation {
        path_pattern: "/users".into(),
        method: "FETCH".into(),
        operation_id: String::new(),
        summary: String::new(),
        description: String::new(),
    };

    // Act
    let result = compiler.apply_mutation(&objects, &make_mutation(&op));

    // Assert
    assert!(result.is_err(), "unknown HTTP method must error");
}

#[test]
fn mutation_add_component_schema_upserts() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let op = OpenApiOp::AddComponentSchema {
        schema_name: "Order".into(),
        schema_type: "object".into(),
        description: "An order".into(),
    };

    // Act
    let effect = compiler
        .apply_mutation(&objects, &make_mutation(&op))
        .unwrap();

    // Assert
    assert_eq!(effect.upserts[0].0, "schema:Order");
}

#[test]
fn mutations_batch_folds_to_net_effect() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let ops = vec![
        make_mutation(&OpenApiOp::AddPath {
            path_pattern: "/orders".into(),
            summary: String::new(),
            description: String::new(),
        }),
        make_mutation(&OpenApiOp::AddComponentSchema {
            schema_name: "Order".into(),
            schema_type: "object".into(),
            description: String::new(),
        }),
    ];

    // Act
    let effect = compiler.apply_mutations(&objects, &ops).unwrap();

    // Assert
    let keys: Vec<&str> = effect.upserts.iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"path:/orders"), "{keys:?}");
    assert!(keys.contains(&"schema:Order"), "{keys:?}");
}

#[test]
fn mutation_with_garbage_bytes_errors() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(MINIMAL).unwrap());
    let mutation = Mutation {
        schema_path: SchemaPath::new("p", "r", "api.yaml"),
        format_id: "openapi".into(),
        operation: Bytes::from_static(b"not json at all"),
    };

    // Act
    let result = compiler.apply_mutation(&objects, &mutation);

    // Assert
    assert!(result.is_err(), "invalid operation bytes must error");
}

// ── P4: conflicts + descriptors + codegen ──────────────────────────────────────

#[test]
fn render_conflict_shows_each_side() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let base = objects.decls.get("schema:User").cloned();
    let ours = objects.decls.get("schema:User").cloned().unwrap();
    let theirs = objects.decls.get("schema:Error").cloned().unwrap();
    let sides = ConflictSides::new(base, vec![ours, theirs]);

    // Act
    let rendered = compiler.render_conflict(&sides).unwrap();

    // Assert
    assert!(rendered.contains("base"), "base section\n{rendered}");
    assert!(rendered.contains("side 0"), "side 0\n{rendered}");
    assert!(rendered.contains("side 1"), "side 1\n{rendered}");
}

#[test]
fn render_conflict_empty_sides_errors() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let sides = ConflictSides::new(None, vec![]);

    // Act
    let result = compiler.render_conflict(&sides);

    // Assert
    assert!(result.is_err(), "empty conflict must error");
}

#[test]
fn validate_resolution_accepts_valid_decl() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let blob = objects.decls.get("schema:User").cloned().unwrap();

    // Act
    let result = compiler.validate_resolution(&blob);

    // Assert
    assert!(
        result.is_ok(),
        "a valid decl blob must validate: {result:?}"
    );
}

#[test]
fn validate_resolution_rejects_garbage() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let blob = DeclBlob::new(b"garbage".to_vec());

    // Act
    let result = compiler.validate_resolution(&blob);

    // Assert
    assert!(result.is_err(), "garbage must fail validation");
}

#[test]
fn generate_descriptors_emits_resolved_yaml() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let objects = to_objects(compiler.parse(FULL).unwrap());
    let mut closure = SchemaClosure::new();
    closure
        .entries
        .insert(SchemaPath::new("proj", "repo", "api.yaml"), objects);

    // Act
    let bytes = compiler.generate_descriptors(&closure).unwrap();

    // Assert
    let yaml = std::str::from_utf8(&bytes).unwrap();
    assert!(
        yaml.contains("openapi:"),
        "descriptor must be a full document\n{yaml}"
    );
    assert!(yaml.contains("components:"), "components section\n{yaml}");
    // Re-parse the descriptor to confirm it is a valid OpenAPI document.
    let reparsed = compiler.parse(yaml);
    assert!(
        reparsed.is_ok(),
        "generated descriptor must re-parse: {reparsed:?}"
    );
}

#[test]
fn generate_code_is_unsupported() {
    // Arrange
    let compiler = OpenApiCompiler::new();
    let closure = SchemaClosure::new();

    // Act
    let result = compiler.generate_code(&closure, Language::Rust);

    // Assert
    assert!(
        matches!(
            result,
            Err(schemahub_types::CodegenError::UnsupportedLanguage(_))
        ),
        "OpenAPI codegen must be unsupported"
    );
}
