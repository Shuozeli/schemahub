//! Behavior tests for diff, compatibility, mutations, conflicts, and codegen.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use schemahub_types::{
    CompatibilityDirection, CompatibilityRules, Compiler, ConflictSides, DeclBlob, DeclChange,
    Language, Mutation, SchemaClosure, SchemaObjects, SchemaPath,
};

use schemahub_compiler_protobuf::{
    OpAddEnumValue, OpAddField, OpAddRpc, OpAddService, OpChangeCardinality, OpChangeFieldType,
    OpChangeRpcType, OpCreateEnum, OpCreateMessage, OpDeleteEnum, OpDeleteMessage,
    OpRemoveEnumValue, OpRemoveField, OpRemoveRpc, OpRenameEnumValue, OpRenameField,
    OpRenameMessage, OpRenameRpc, OpReorderFields, OpUpdateImport, ProtoOp, ProtobufCompiler,
};

fn parse_to_objects(source: &str) -> SchemaObjects {
    let compiler = ProtobufCompiler::new();
    let parsed = compiler.parse(source).expect("parse ok");
    let mut decls = BTreeMap::new();
    for (name, blob) in parsed.decls {
        decls.insert(name, blob);
    }
    SchemaObjects {
        meta: parsed.meta,
        decls,
    }
}

fn decl(objects: &SchemaObjects, name: &str) -> DeclBlob {
    objects.decls.get(name).cloned().expect("decl present")
}

fn mutation(path_schema: &str, op: ProtoOp) -> Mutation {
    Mutation {
        schema_path: SchemaPath::new("p", "r", path_schema),
        format_id: "protobuf".to_string(),
        operation: Bytes::from(op.encode()),
    }
}

// ── Mutations ──────────────────────────────────────────────────────────────

#[test]
fn add_field_appends_to_message() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::AddField(OpAddField {
            message_name: "M".into(),
            field_name: "b".into(),
            field_type: "string".into(),
            field_number: 2,
            cardinality: String::new(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    assert_eq!(effect.upserts.len(), 1);
    assert_eq!(effect.upserts[0].0, "M");
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("string b = 2"), "got:\n{printed}");
}

#[test]
fn add_field_with_duplicate_number_is_rejected() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::AddField(OpAddField {
            message_name: "M".into(),
            field_name: "b".into(),
            field_type: "string".into(),
            field_number: 1,
            cardinality: String::new(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(result.is_err(), "duplicate field number must be rejected");
}

#[test]
fn remove_field_auto_reserves_number_and_name() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::RemoveField(OpRemoveField {
            message_name: "M".into(),
            field_name: "a".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(
        printed.contains("reserved 1;"),
        "number reserved; got:\n{printed}"
    );
    assert!(
        printed.contains("reserved \"a\""),
        "name reserved; got:\n{printed}"
    );
}

#[test]
fn change_field_number_is_rejected() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeFieldNumber(schemahub_compiler_protobuf::OpChangeFieldNumber {
            message_name: "M".into(),
            field_name: "a".into(),
            new_number: 5,
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(result.is_err(), "ChangeFieldNumber must always be rejected");
}

#[test]
fn rename_field_preserves_number() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::RenameField(OpRenameField {
            message_name: "M".into(),
            old_field_name: "a".into(),
            new_field_name: "renamed".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("int32 renamed = 1"), "got:\n{printed}");
}

#[test]
fn add_enum_value_appends() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nenum E { UNKNOWN = 0; }\n");
    let op = mutation(
        "e.proto",
        ProtoOp::AddEnumValue(OpAddEnumValue {
            enum_name: "E".into(),
            value_name: "ACTIVE".into(),
            number: 1,
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("ACTIVE = 1"), "got:\n{printed}");
}

#[test]
fn apply_mutations_batch_applies_in_order() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let ops = vec![
        mutation(
            "m.proto",
            ProtoOp::AddField(OpAddField {
                message_name: "M".into(),
                field_name: "b".into(),
                field_type: "int32".into(),
                field_number: 2,
                cardinality: String::new(),
            }),
        ),
        mutation(
            "m.proto",
            ProtoOp::AddField(OpAddField {
                message_name: "M".into(),
                field_name: "c".into(),
                field_type: "int32".into(),
                field_number: 3,
                cardinality: String::new(),
            }),
        ),
    ];

    // Act
    let effect = compiler.apply_mutations(&objects, &ops).expect("batch ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("int32 b = 2"), "got:\n{printed}");
    assert!(printed.contains("int32 c = 3"), "got:\n{printed}");
}

/// Helper: apply an effect to objects and print the result.
fn print_with(
    compiler: &ProtobufCompiler,
    objects: &SchemaObjects,
    effect: &schemahub_types::MutationEffect,
) -> String {
    let mut updated = objects.clone();
    if let Some(meta) = &effect.meta {
        updated.meta = meta.clone();
    }
    for (name, blob) in &effect.upserts {
        updated.decls.insert(name.clone(), blob.clone());
    }
    for name in &effect.removes {
        updated.decls.remove(name);
    }
    compiler.print(&updated).expect("print ok")
}

// ── Compatibility ─────────────────────────────────────────────────────────

fn full() -> CompatibilityRules {
    CompatibilityRules::full()
}

fn rules(direction: CompatibilityDirection) -> CompatibilityRules {
    CompatibilityRules {
        direction,
        disabled: false,
    }
}

#[test]
fn add_optional_field_is_full_compatible() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; string b = 2; }\n"),
        "M",
    );

    // Act
    let result = compiler.check_compatibility(&old, &new, &full());

    // Assert
    assert!(
        result.is_ok(),
        "adding a field is FULL-compatible: {result:?}"
    );
}

#[test]
fn remove_field_is_backward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; }\n"),
        "M",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );

    // Act
    let backward =
        compiler.check_compatibility(&old, &new, &rules(CompatibilityDirection::Backward));
    let forward = compiler.check_compatibility(&old, &new, &rules(CompatibilityDirection::Forward));
    let fulls = compiler.check_compatibility(&old, &new, &full());

    // Assert
    assert!(backward.is_ok(), "remove is BACKWARD-ok: {backward:?}");
    assert!(forward.is_err(), "remove breaks FORWARD");
    assert!(fulls.is_err(), "remove breaks FULL");
}

#[test]
fn add_enum_value_is_backward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; }\n"),
        "E",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; B = 1; }\n"),
        "E",
    );

    // Act
    let backward =
        compiler.check_compatibility(&old, &new, &rules(CompatibilityDirection::Backward));
    let forward = compiler.check_compatibility(&old, &new, &rules(CompatibilityDirection::Forward));

    // Assert
    assert!(
        backward.is_ok(),
        "adding enum value is BACKWARD-ok: {backward:?}"
    );
    assert!(forward.is_err(), "adding enum value breaks FORWARD");
}

#[test]
fn int32_to_int64_is_full_compatible() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int64 a = 1; }\n"),
        "M",
    );

    // Act
    let result = compiler.check_compatibility(&old, &new, &full());

    // Assert
    assert!(
        result.is_ok(),
        "int32->int64 is FULL-compatible: {result:?}"
    );
}

#[test]
fn int_to_string_crosses_wire_type_and_breaks() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { string a = 1; }\n"),
        "M",
    );

    // Act
    let result = compiler.check_compatibility(&old, &new, &full());

    // Assert
    assert!(
        result.is_err(),
        "int32->string crosses wire type, must break"
    );
}

#[test]
fn add_rpc_is_backward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); }\n",
        ),
        "S",
    );
    let new = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); rpc B(R) returns (R); }\n",
        ),
        "S",
    );

    // Act
    let backward =
        compiler.check_compatibility(&old, &new, &rules(CompatibilityDirection::Backward));
    let forward = compiler.check_compatibility(&old, &new, &rules(CompatibilityDirection::Forward));

    // Assert
    assert!(backward.is_ok(), "adding RPC is BACKWARD-ok: {backward:?}");
    assert!(forward.is_err(), "adding RPC breaks FORWARD");
}

#[test]
fn disabled_rules_skip_all_checks() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; }\n"),
        "M",
    );
    let new = decl(&parse_to_objects("syntax=\"proto3\";\nmessage M {}\n"), "M");

    // Act
    let result = compiler.check_compatibility(&old, &new, &rules(CompatibilityDirection::Disabled));

    // Assert
    assert!(result.is_ok(), "disabled rules never report violations");
}

// ── Diff ─────────────────────────────────────────────────────────────────

#[test]
fn diff_reports_modified_with_detail() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; }\n"),
        "M",
    );

    // Act
    let change = compiler.diff_decl(&old, &new).expect("diff ok");

    // Assert
    match change {
        DeclChange::DeclarationModified { name, detail } => {
            assert_eq!(name, "M");
            let text = String::from_utf8_lossy(&detail);
            assert!(text.contains("+ field b = 2"), "got: {text}");
        }
        other => panic!("expected Modified, got {other:?}"),
    }
}

// ── Conflict ───────────────────────────────────────────────────────────────

#[test]
fn render_conflict_shows_all_sides() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let base = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );
    let ours = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; }\n"),
        "M",
    );
    let theirs = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 c = 3; }\n"),
        "M",
    );
    let sides = ConflictSides::new(Some(base), vec![ours, theirs]);

    // Act
    let rendered = compiler.render_conflict(&sides).expect("render ok");

    // Assert
    assert!(rendered.contains("base"), "shows base");
    assert!(rendered.contains("side 1"), "shows side 1");
    assert!(rendered.contains("side 2"), "shows side 2");
    assert!(rendered.contains("int32 b = 2"));
    assert!(rendered.contains("int32 c = 3"));
}

#[test]
fn validate_resolution_accepts_valid_decl() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let resolved = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );

    // Act
    let result = compiler.validate_resolution(&resolved);

    // Assert
    assert!(result.is_ok());
}

#[test]
fn validate_resolution_rejects_garbage() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let garbage = DeclBlob::new(vec![0xFF, 0x00, 0x12]);

    // Act
    let result = compiler.validate_resolution(&garbage);

    // Assert
    assert!(result.is_err());
}

// ── Read / exploration ───────────────────────────────────────────────────

#[test]
fn type_refs_lists_referenced_messages() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage A {}\nmessage B { A a = 1; }\n");
    let b = decl(&objects, "B");

    // Act
    let refs = compiler.type_refs(&b).expect("type_refs ok");

    // Assert
    assert!(
        refs.iter().any(|r| r.name == "A"),
        "B references A: {refs:?}"
    );
}

#[test]
fn field_type_ref_selects_the_requested_message_field() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\"; message Address {} message Order { string id = 1; Address shipping = 2; }",
    );
    let order = decl(&objects, "Order");

    // Act
    let reference = compiler
        .field_type_ref(&order, "shipping")
        .expect("read field type")
        .expect("shipping has a named type");

    // Assert
    assert_eq!(reference.name, "Address");
    assert!(reference.import.is_none());
}

#[test]
fn imports_lists_dependencies() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nimport \"dep.proto\";\nmessage M { int32 a = 1; }\n");

    // Act
    let imports = compiler.imports(&objects).expect("imports ok");

    // Assert
    assert!(
        imports.iter().any(|i| i.path == "dep.proto"),
        "got: {imports:?}"
    );
}

#[test]
fn update_import_retains_immutable_commit_pin() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M {}\n");
    let op = mutation(
        "m.proto",
        ProtoOp::UpdateImport(OpUpdateImport {
            import_path: "shared/models/common.proto".into(),
            resolved_commit: "abc123".into(),
            remove: false,
        }),
    );

    // Act
    let effect = compiler
        .apply_mutation(&objects, &op)
        .expect("update import");

    // Assert
    let mut updated = objects.clone();
    updated.meta = effect.meta.expect("updated meta");
    let imports = compiler.imports(&updated).expect("read imports");
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].path, "shared/models/common.proto");
    assert_eq!(imports[0].resolved_commit, "abc123");
}

#[test]
fn import_add_then_remove_has_no_dependency() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M {}\n");
    let ops = [
        mutation(
            "m.proto",
            ProtoOp::UpdateImport(OpUpdateImport {
                import_path: "shared/models/common.proto".into(),
                resolved_commit: "abc123".into(),
                remove: false,
            }),
        ),
        mutation(
            "m.proto",
            ProtoOp::UpdateImport(OpUpdateImport {
                import_path: "shared/models/common.proto".into(),
                resolved_commit: String::new(),
                remove: true,
            }),
        ),
    ];

    // Act
    let effect = compiler
        .apply_mutations(&objects, &ops)
        .expect("apply import transaction");

    // Assert
    let mut updated = objects.clone();
    updated.meta = effect.meta.expect("updated meta");
    let imports = compiler.imports(&updated).expect("read imports");
    assert!(imports.is_empty());
}

// ── Codegen ────────────────────────────────────────────────────────────────

#[test]
fn generate_code_rust_produces_struct() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let mut closure = SchemaClosure::new();
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "m.proto"), objects);

    // Act
    let code = compiler
        .generate_code(
            &closure,
            Language::Rust,
            &schemahub_types::CodegenOptions::default(),
        )
        .expect("codegen ok");

    // Assert
    assert!(
        code.contains("struct M"),
        "expected a struct M; got:\n{code}"
    );
}

#[test]
fn generate_code_resolves_imported_and_nested_types_across_closure() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let common = parse_to_objects(
        r#"syntax = "proto3";
package commerce.v1;
message Money { int64 units = 1; }
enum Region { REGION_UNSPECIFIED = 0; REGION_EMEA = 1; }
"#,
    );
    let order = parse_to_objects(
        r#"syntax = "proto3";
package commerce.v1;
import "p/r/common.proto";
message Order {
  message LineItem { Money price = 1; }
  enum State { STATE_UNSPECIFIED = 0; STATE_READY = 1; }
  Money total = 1;
  LineItem item = 2;
  Region region = 3;
  State state = 4;
}
service Orders { rpc Get(Order) returns (Order); }
"#,
    );
    let root = SchemaPath::new("p", "r", "order.proto");
    let mut closure = SchemaClosure::with_root(root.clone());
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "common.proto"), common);
    closure.entries.insert(root, order);

    // Act
    let code = compiler
        .generate_code(
            &closure,
            Language::Rust,
            &schemahub_types::CodegenOptions::default(),
        )
        .expect("closure codegen ok");

    // Assert
    assert!(
        code.contains("pub total: ::core::option::Option<Money>"),
        "imported message should retain its Rust type:\n{code}"
    );
    assert!(
        code.contains("pub item: ::core::option::Option<order::LineItem>"),
        "nested message should use its generated module path:\n{code}"
    );
    assert!(
        code.contains("enumeration = \"Region\""),
        "imported enum should retain its prost type annotation:\n{code}"
    );
    assert!(
        code.contains("enumeration = \"order::State\""),
        "nested enum should use its generated module path:\n{code}"
    );
    assert!(
        code.contains("pub price: ::core::option::Option<super::Money>"),
        "nested message should resolve an imported package sibling:\n{code}"
    );
}

#[test]
fn generate_code_renders_cross_package_modules_and_reexports_the_requested_root() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let common = parse_to_objects(
        r#"syntax = "proto3";
package shared.money.v1;
message Money { int64 units = 1; }
"#,
    );
    let order = parse_to_objects(
        r#"syntax = "proto3";
package commerce.order.v1;
import "p/r/common.proto";
message Order { shared.money.v1.Money total = 1; }
"#,
    );
    let root = SchemaPath::new("p", "r", "order.proto");
    let mut closure = SchemaClosure::with_root(root.clone());
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "common.proto"), common);
    closure.entries.insert(root, order);

    // Act
    let code = compiler
        .generate_code(
            &closure,
            Language::Rust,
            &schemahub_types::CodegenOptions::default(),
        )
        .expect("cross-package closure codegen");

    // Assert
    assert!(code.contains("pub mod shared {"), "{code}");
    assert!(code.contains("use super::super::super::shared;"), "{code}");
    assert!(
        code.contains("pub total: ::core::option::Option<shared::money::v1::Money>"),
        "{code}"
    );
    assert!(code.contains("pub use commerce::order::v1::*;"), "{code}");
}

#[test]
fn generate_code_rejects_a_multi_file_closure_without_an_explicit_root() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let mut closure = SchemaClosure::new();
    closure.entries.insert(
        SchemaPath::new("p", "r", "common.proto"),
        parse_to_objects("syntax = \"proto3\"; package shared.v1; message Shared {}"),
    );
    closure.entries.insert(
        SchemaPath::new("p", "r", "root.proto"),
        parse_to_objects("syntax = \"proto3\"; package app.v1; message Root {}"),
    );

    // Act
    let result = compiler.generate_code(
        &closure,
        Language::Rust,
        &schemahub_types::CodegenOptions::default(),
    );

    // Assert
    let error = result.expect_err("multi-file codegen must require a root");
    assert!(
        error
            .to_string()
            .contains("multi-file Protobuf codegen requires an explicit root schema"),
        "unexpected error: {error}"
    );
}

#[test]
fn generate_code_rejects_unresolved_named_type() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\"; package test; message M { Missing value = 1; }");
    let mut closure = SchemaClosure::new();
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "m.proto"), objects);

    // Act
    let result = compiler.generate_code(
        &closure,
        Language::Rust,
        &schemahub_types::CodegenOptions::default(),
    );

    // Assert
    let error = result.expect_err("unresolved type must fail codegen");
    assert!(
        error
            .to_string()
            .contains("unresolved protobuf type `Missing`"),
        "unexpected error: {error}"
    );
}

#[test]
fn generate_code_unsupported_language_errors() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let closure = SchemaClosure::new();

    // Act
    let result = compiler.generate_code(
        &closure,
        Language::Go,
        &schemahub_types::CodegenOptions::default(),
    );

    // Assert
    assert!(matches!(
        result,
        Err(schemahub_types::CodegenError::UnsupportedLanguage(
            Language::Go
        ))
    ));
}

#[test]
fn generate_descriptors_is_nonempty_and_versioned() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let mut closure = SchemaClosure::new();
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "m.proto"), objects);

    // Act
    let bytes = compiler
        .generate_descriptors(&closure)
        .expect("descriptors ok");

    // Assert
    assert!(!bytes.is_empty(), "descriptor bytes must be produced");
    assert_eq!(bytes[0], schemahub_compiler_protobuf::BLOB_VERSION);
}

// ════════════════════════════════════════════════════════════════════════════
// EXTENDED COVERAGE: full granular-mutation op set + compatibility rule matrix
// ════════════════════════════════════════════════════════════════════════════

// ── Cardinality mutations ───────────────────────────────────────────────────

#[test]
fn change_cardinality_singular_to_optional_sets_proto3_optional() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "M".into(),
            field_name: "a".into(),
            new_cardinality: "optional".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(
        printed.contains("optional int32 a = 1"),
        "proto3 optional; got:\n{printed}"
    );
}

#[test]
fn change_cardinality_singular_to_repeated_sets_repeated_label() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "M".into(),
            field_name: "a".into(),
            new_cardinality: "repeated".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("repeated int32 a = 1"), "got:\n{printed}");
}

#[test]
fn change_cardinality_repeated_to_singular_drops_label() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { repeated int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "M".into(),
            field_name: "a".into(),
            new_cardinality: "singular".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(
        printed.contains("int32 a = 1") && !printed.contains("repeated"),
        "label dropped; got:\n{printed}"
    );
}

#[test]
fn change_cardinality_proto2_to_required_sets_required_label() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto2\";\nmessage M { optional int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "M".into(),
            field_name: "a".into(),
            new_cardinality: "required".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("required int32 a = 1"), "got:\n{printed}");
}

#[test]
fn change_cardinality_unknown_keyword_is_rejected() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "M".into(),
            field_name: "a".into(),
            new_cardinality: "bogus".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(
        result.is_err(),
        "unknown cardinality keyword must be rejected"
    );
}

// ── Reorder fields ──────────────────────────────────────────────────────────

#[test]
fn reorder_fields_changes_order_but_keeps_numbers() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; int32 c = 3; }\n",
    );
    let op = mutation(
        "m.proto",
        ProtoOp::ReorderFields(OpReorderFields {
            message_name: "M".into(),
            field_order: vec!["c".into(), "a".into(), "b".into()],
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert — printed order is c, a, b; field numbers unchanged (identity).
    let printed = print_with(&compiler, &objects, &effect);
    let pos_c = printed.find("int32 c = 3").expect("c present");
    let pos_a = printed.find("int32 a = 1").expect("a present");
    let pos_b = printed.find("int32 b = 2").expect("b present");
    assert!(
        pos_c < pos_a && pos_a < pos_b,
        "order should be c,a,b; got:\n{printed}"
    );
}

#[test]
fn reorder_fields_with_mismatched_set_is_rejected() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ReorderFields(OpReorderFields {
            message_name: "M".into(),
            field_order: vec!["a".into()], // omits b
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(
        result.is_err(),
        "reorder must list exactly the existing fields"
    );
}

// ── Message create / rename / delete ────────────────────────────────────────

#[test]
fn create_message_adds_empty_message() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "N".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("message N {"), "got:\n{printed}");
}

#[test]
fn create_message_duplicate_is_rejected() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "M".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(
        result.is_err(),
        "creating an existing message must be rejected"
    );
}

#[test]
fn rename_message_renames_decl_and_removes_old() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::RenameMessage(OpRenameMessage {
            old_name: "M".into(),
            new_name: "Renamed".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    assert!(
        effect.upserts.iter().any(|(n, _)| n == "Renamed"),
        "new decl upserted"
    );
    assert!(effect.removes.iter().any(|n| n == "M"), "old decl removed");
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("message Renamed {"), "got:\n{printed}");
    assert!(
        !printed.contains("message M {"),
        "old name gone; got:\n{printed}"
    );
}

#[test]
fn delete_message_removes_decl() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\";\nmessage M { int32 a = 1; }\nmessage N { int32 b = 1; }\n",
    );
    let op = mutation(
        "m.proto",
        ProtoOp::DeleteMessage(OpDeleteMessage {
            message_name: "N".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    assert!(effect.removes.iter().any(|n| n == "N"), "N removed");
    let printed = print_with(&compiler, &objects, &effect);
    assert!(!printed.contains("message N {"), "N gone; got:\n{printed}");
}

#[test]
fn delete_message_rejects_same_file_field_and_rpc_references() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\";\nmessage M {}\nmessage N { M value = 1; }\nservice S { rpc Get(M) returns (N); }\n",
    );
    let op = mutation(
        "m.proto",
        ProtoOp::DeleteMessage(OpDeleteMessage {
            message_name: "M".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    let error = result.expect_err("referenced message deletion must fail");
    assert!(error.to_string().contains("referenced by N, S"), "{error}");
}

#[test]
fn transaction_checks_message_references_on_final_state() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nmessage M {}\nmessage N { M value = 1; }\n");
    let ops = [
        mutation(
            "m.proto",
            ProtoOp::DeleteMessage(OpDeleteMessage {
                message_name: "M".into(),
            }),
        ),
        mutation(
            "m.proto",
            ProtoOp::DeleteMessage(OpDeleteMessage {
                message_name: "N".into(),
            }),
        ),
    ];

    // Act
    let effect = compiler
        .apply_mutations(&objects, &ops)
        .expect("final state has no dangling reference");

    // Assert
    assert_eq!(
        effect.removes.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from(["M".to_string(), "N".to_string()])
    );
}

#[test]
fn rename_message_rejects_existing_cross_kind_target() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M {}\nenum E { ZERO = 0; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::RenameMessage(OpRenameMessage {
            old_name: "M".into(),
            new_name: "E".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(result
        .expect_err("cross-kind target collision must fail")
        .to_string()
        .contains("target already exists"));
}

// ── Service & RPC ops ───────────────────────────────────────────────────────

#[test]
fn add_service_creates_empty_service() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage R {}\n");
    let op = mutation(
        "m.proto",
        ProtoOp::AddService(OpAddService {
            service_name: "Svc".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("service Svc {"), "got:\n{printed}");
}

#[test]
fn add_rpc_appends_method() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::AddRpc(OpAddRpc {
            service_name: "S".into(),
            rpc_name: "B".into(),
            request_type: "R".into(),
            response_type: "R".into(),
            client_streaming: false,
            server_streaming: false,
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("rpc B(R) returns (R)"), "got:\n{printed}");
}

#[test]
fn add_rpc_duplicate_is_rejected() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::AddRpc(OpAddRpc {
            service_name: "S".into(),
            rpc_name: "A".into(),
            request_type: "R".into(),
            response_type: "R".into(),
            client_streaming: false,
            server_streaming: false,
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(result.is_err(), "duplicate rpc name must be rejected");
}

#[test]
fn rename_rpc_changes_method_name() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::RenameRpc(OpRenameRpc {
            service_name: "S".into(),
            old_rpc_name: "A".into(),
            new_rpc_name: "Renamed".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(
        printed.contains("rpc Renamed(R) returns (R)"),
        "got:\n{printed}"
    );
    assert!(
        !printed.contains("rpc A("),
        "old name gone; got:\n{printed}"
    );
}

#[test]
fn remove_rpc_drops_method() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); rpc B(R) returns (R); }\n",
    );
    let op = mutation(
        "m.proto",
        ProtoOp::RemoveRpc(OpRemoveRpc {
            service_name: "S".into(),
            rpc_name: "A".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(!printed.contains("rpc A("), "A gone; got:\n{printed}");
    assert!(
        printed.contains("rpc B(R) returns (R)"),
        "B kept; got:\n{printed}"
    );
}

#[test]
fn change_rpc_type_updates_request_and_response() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\";\nmessage R {}\nmessage Q {}\nservice S { rpc A(R) returns (R); }\n",
    );
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeRpcType(OpChangeRpcType {
            service_name: "S".into(),
            rpc_name: "A".into(),
            new_request_type: "Q".into(),
            new_response_type: "Q".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("rpc A(Q) returns (Q)"), "got:\n{printed}");
}

// ── Enum ops ────────────────────────────────────────────────────────────────

#[test]
fn remove_enum_value_auto_reserves() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; B = 1; }\n");
    let op = mutation(
        "e.proto",
        ProtoOp::RemoveEnumValue(OpRemoveEnumValue {
            enum_name: "E".into(),
            value_name: "B".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(!printed.contains("B = 1"), "value removed; got:\n{printed}");
    assert!(
        printed.contains("reserved 1"),
        "number reserved; got:\n{printed}"
    );
    assert!(
        printed.contains("reserved \"B\""),
        "name reserved; got:\n{printed}"
    );
}

#[test]
fn rename_enum_value_changes_name_keeps_number() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; B = 1; }\n");
    let op = mutation(
        "e.proto",
        ProtoOp::RenameEnumValue(OpRenameEnumValue {
            enum_name: "E".into(),
            old_value_name: "B".into(),
            new_value_name: "RENAMED".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("RENAMED = 1"), "got:\n{printed}");
}

#[test]
fn remove_enum_value_rejects_a_proto2_default_reference() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax = \"proto2\";\n\
         enum State { UNKNOWN = 0; READY = 1; }\n\
         message Item { optional State state = 1 [default = READY]; }\n",
    );
    let op = mutation(
        "state.proto",
        ProtoOp::RemoveEnumValue(OpRemoveEnumValue {
            enum_name: "State".into(),
            value_name: "READY".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    let error = result.expect_err("a referenced enum default cannot be removed");
    assert!(
        error.to_string().contains("referenced as a default"),
        "{error}"
    );
}

#[test]
fn rename_enum_value_propagates_a_proto2_default_reference() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax = \"proto2\";\n\
         enum State { UNKNOWN = 0; READY = 1; }\n\
         message Item { optional State state = 1 [default = READY]; }\n",
    );
    let op = mutation(
        "state.proto",
        ProtoOp::RenameEnumValue(OpRenameEnumValue {
            enum_name: "State".into(),
            old_value_name: "READY".into(),
            new_value_name: "ACTIVE".into(),
        }),
    );

    // Act
    let effect = compiler
        .apply_mutation(&objects, &op)
        .expect("default follows the enum-value rename");

    // Assert
    assert!(effect.upserts.iter().any(|(name, _)| name == "Item"));
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("default = ACTIVE"), "{printed}");
    assert!(!printed.contains("default = READY"), "{printed}");
}

#[test]
fn transaction_can_remove_enum_value_after_removing_its_consumer() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax = \"proto2\";\n\
         enum State { UNKNOWN = 0; READY = 1; }\n\
         message Item { optional State state = 1 [default = READY]; }\n",
    );
    let ops = [
        mutation(
            "state.proto",
            ProtoOp::RemoveEnumValue(OpRemoveEnumValue {
                enum_name: "State".into(),
                value_name: "READY".into(),
            }),
        ),
        mutation(
            "state.proto",
            ProtoOp::DeleteMessage(OpDeleteMessage {
                message_name: "Item".into(),
            }),
        ),
    ];

    // Act
    let effect = compiler
        .apply_mutations(&objects, &ops)
        .expect("final state contains no default reference");

    // Assert
    assert!(effect.removes.iter().any(|name| name == "Item"));
    assert!(effect.upserts.iter().any(|(name, _)| name == "State"));
}

#[test]
fn create_enum_adds_empty_enum() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::CreateEnum(OpCreateEnum {
            enum_name: "E".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("enum E {"), "got:\n{printed}");
}

#[test]
fn delete_enum_removes_decl() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; }\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::DeleteEnum(OpDeleteEnum {
            enum_name: "E".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    assert!(effect.removes.iter().any(|n| n == "E"), "E removed");
    let printed = print_with(&compiler, &objects, &effect);
    assert!(!printed.contains("enum E {"), "E gone; got:\n{printed}");
}

#[test]
fn delete_enum_rejects_same_file_field_reference() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nenum E { ZERO = 0; }\nmessage M { E value = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::DeleteEnum(OpDeleteEnum {
            enum_name: "E".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    assert!(result
        .expect_err("referenced enum deletion must fail")
        .to_string()
        .contains("referenced by M"));
}

#[test]
fn first_proto3_enum_value_must_use_zero() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M {}\n");
    let ops = [
        mutation(
            "m.proto",
            ProtoOp::CreateEnum(OpCreateEnum {
                enum_name: "E".into(),
            }),
        ),
        mutation(
            "m.proto",
            ProtoOp::AddEnumValue(OpAddEnumValue {
                enum_name: "E".into(),
                value_name: "ONE".into(),
                number: 1,
            }),
        ),
    ];

    // Act
    let result = compiler.apply_mutations(&objects, &ops);

    // Assert
    assert!(result
        .expect_err("non-zero first proto3 enum value must fail")
        .to_string()
        .contains("must use number 0"));
}

// ── oneof ops ───────────────────────────────────────────────────────────────

#[test]
fn remove_oneof_member_field_keeps_oneof_and_auto_reserves() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\";\nmessage M { oneof choice { int32 a = 1; string b = 2; } }\n",
    );
    let op = mutation(
        "m.proto",
        ProtoOp::RemoveField(OpRemoveField {
            message_name: "M".into(),
            field_name: "a".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert — the oneof block keeps `b`; removed `a` is auto-reserved.
    let printed = print_with(&compiler, &objects, &effect);
    assert!(
        printed.contains("oneof choice {"),
        "oneof kept; got:\n{printed}"
    );
    assert!(printed.contains("string b = 2"), "b kept; got:\n{printed}");
    assert!(
        printed.contains("reserved 1"),
        "a's number reserved; got:\n{printed}"
    );
}

/// The op set has no oneof-aware AddField: a plain AddField always lands OUTSIDE
/// an existing oneof, never inside it. Documents the current capability gap.
#[test]
fn add_field_lands_outside_existing_oneof() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects =
        parse_to_objects("syntax=\"proto3\";\nmessage M { oneof choice { int32 a = 1; } }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::AddField(OpAddField {
            message_name: "M".into(),
            field_name: "c".into(),
            field_type: "string".into(),
            field_number: 3,
            cardinality: String::new(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert — `c` appears after the closing brace of `oneof choice`, i.e. outside.
    let printed = print_with(&compiler, &objects, &effect);
    let oneof_close = printed.find("  }").expect("oneof closing brace");
    let c_pos = printed.find("string c = 3").expect("c present");
    assert!(
        c_pos > oneof_close,
        "new field is outside the oneof; got:\n{printed}"
    );
}

// ── Reference integrity on rename (design §7) ───────────────────────────────

/// `RenameMessage` renames the target decl AND propagates the rename within the
/// schema (design.md §7): fields and RPCs that referenced the old name are
/// rewritten to the new name, and the updated decls are included in the effect's
/// upserts. The referencing field `B.a` and `rpc Do(A) returns (A)` follow `A`
/// to its new name `Renamed`, leaving no dangling reference.
#[test]
fn rename_message_propagates_references() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax=\"proto3\";\nmessage A {}\nmessage B { A a = 1; }\nservice S { rpc Do(A) returns (A); }\n",
    );
    let op = mutation(
        "m.proto",
        ProtoOp::RenameMessage(OpRenameMessage {
            old_name: "A".into(),
            new_name: "Renamed".into(),
        }),
    );

    // Act
    let effect = compiler
        .apply_mutation(&objects, &op)
        .expect("rename succeeds");

    // Assert — `A` renamed to `Renamed`, and the referencing `B` and `S` are
    // upserted with their references updated.
    assert!(effect.upserts.iter().any(|(n, _)| n == "Renamed"));
    assert!(effect.removes.iter().any(|n| n == "A"));
    assert!(
        effect.upserts.iter().any(|(n, _)| n == "B"),
        "B's referencing field is updated in the same effect"
    );
    assert!(
        effect.upserts.iter().any(|(n, _)| n == "S"),
        "S's referencing rpc is updated in the same effect"
    );
    let printed = print_with(&compiler, &objects, &effect);
    assert!(
        printed.contains("Renamed a = 1") && printed.contains("rpc Do(Renamed) returns (Renamed)"),
        "references now point at new name 'Renamed'; got:\n{printed}"
    );
    assert!(
        !printed.contains(" A a = 1") && !printed.contains("rpc Do(A)"),
        "no dangling reference to old name 'A'; got:\n{printed}"
    );
}

#[test]
fn rename_message_propagates_top_level_extension_extendee() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax = \"proto2\";\n\
         message Host { extensions 100 to max; }\n\
         extend Host { optional string note = 100; }\n",
    );
    let op = mutation(
        "extensions.proto",
        ProtoOp::RenameMessage(OpRenameMessage {
            old_name: "Host".into(),
            new_name: "RenamedHost".into(),
        }),
    );

    // Act
    let effect = compiler
        .apply_mutation(&objects, &op)
        .expect("extension extendee follows the rename");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("extend RenamedHost"), "{printed}");
    assert!(!printed.contains("extend Host"), "{printed}");
}

#[test]
fn delete_message_rejects_top_level_extension_extendee_reference() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects(
        "syntax = \"proto2\";\n\
         message Host { extensions 100 to max; }\n\
         extend Host { optional string note = 100; }\n",
    );
    let op = mutation(
        "extensions.proto",
        ProtoOp::DeleteMessage(OpDeleteMessage {
            message_name: "Host".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert
    let error = result.expect_err("an extension cannot target a deleted message");
    assert!(error.to_string().contains("file extension"), "{error}");
}

// ════════════════════════════════════════════════════════════════════════════
// COMPATIBILITY MATRIX — every direction (Backward / Forward / Full)
// ════════════════════════════════════════════════════════════════════════════

/// Helper: (backward_ok, forward_ok, full_ok) verdicts for a decl pair.
fn verdicts(compiler: &ProtobufCompiler, old: &DeclBlob, new: &DeclBlob) -> (bool, bool, bool) {
    (
        compiler
            .check_compatibility(old, new, &rules(CompatibilityDirection::Backward))
            .is_ok(),
        compiler
            .check_compatibility(old, new, &rules(CompatibilityDirection::Forward))
            .is_ok(),
        compiler.check_compatibility(old, new, &full()).is_ok(),
    )
}

#[test]
fn matrix_add_optional_field_ok_all_directions() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );
    let new = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nmessage M { int32 a = 1; optional string b = 2; }\n",
        ),
        "M",
    );

    // Act
    let (b, f, fu) = verdicts(&compiler, &old, &new);

    // Assert
    assert_eq!(
        (b, f, fu),
        (true, true, true),
        "add optional ok all directions"
    );
}

#[test]
fn matrix_remove_field_backward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; int32 b = 2; }\n"),
        "M",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n"),
        "M",
    );

    // Act
    let (b, f, fu) = verdicts(&compiler, &old, &new);

    // Assert
    assert_eq!(
        (b, f, fu),
        (true, false, false),
        "remove field: Backward ok only"
    );
}

#[test]
fn matrix_add_enum_value_backward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; }\n"),
        "E",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; B = 1; }\n"),
        "E",
    );

    // Act
    let (b, f, fu) = verdicts(&compiler, &old, &new);

    // Assert
    assert_eq!(
        (b, f, fu),
        (true, false, false),
        "add enum value: Backward ok only"
    );
}

#[test]
fn matrix_remove_enum_value_forward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; B = 1; }\n"),
        "E",
    );
    let new = decl(
        &parse_to_objects("syntax=\"proto3\";\nenum E { A = 0; }\n"),
        "E",
    );

    // Act
    let (b, f, fu) = verdicts(&compiler, &old, &new);

    // Assert
    assert_eq!(
        (b, f, fu),
        (false, true, false),
        "remove enum value: Forward ok only"
    );
}

#[test]
fn matrix_add_rpc_backward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); }\n",
        ),
        "S",
    );
    let new = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); rpc B(R) returns (R); }\n",
        ),
        "S",
    );

    // Act
    let (b, f, fu) = verdicts(&compiler, &old, &new);

    // Assert
    assert_eq!(
        (b, f, fu),
        (true, false, false),
        "add rpc: Backward ok only"
    );
}

#[test]
fn matrix_remove_rpc_forward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); rpc B(R) returns (R); }\n",
        ),
        "S",
    );
    let new = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nmessage R {}\nservice S { rpc A(R) returns (R); }\n",
        ),
        "S",
    );

    // Act
    let (b, f, fu) = verdicts(&compiler, &old, &new);

    // Assert
    assert_eq!(
        (b, f, fu),
        (false, true, false),
        "remove rpc: Forward ok only"
    );
}

// ── Type-change allowlist rows (within wire type) ───────────────────────────

/// Helper: compat verdicts for a `M { <from> a = 1; }` → `M { <to> a = 1; }` change.
fn type_change_verdicts(compiler: &ProtobufCompiler, from: &str, to: &str) -> (bool, bool, bool) {
    let old = decl(
        &parse_to_objects(&format!(
            "syntax=\"proto3\";\nmessage M {{ {from} a = 1; }}\n"
        )),
        "M",
    );
    let new = decl(
        &parse_to_objects(&format!(
            "syntax=\"proto3\";\nmessage M {{ {to} a = 1; }}\n"
        )),
        "M",
    );
    verdicts(compiler, &old, &new)
}

#[test]
fn matrix_uint32_to_uint64_ok_all_directions() {
    // Arrange
    let compiler = ProtobufCompiler::new();

    // Act
    let v = type_change_verdicts(&compiler, "uint32", "uint64");

    // Assert
    assert_eq!(v, (true, true, true), "uint32->uint64 ok all directions");
}

#[test]
fn matrix_sint32_to_sint64_ok_all_directions() {
    // Arrange
    let compiler = ProtobufCompiler::new();

    // Act
    let v = type_change_verdicts(&compiler, "sint32", "sint64");

    // Assert
    assert_eq!(v, (true, true, true), "sint32->sint64 ok all directions");
}

#[test]
fn matrix_string_to_bytes_ok_all_directions() {
    // Arrange
    let compiler = ProtobufCompiler::new();

    // Act
    let v = type_change_verdicts(&compiler, "string", "bytes");

    // Assert
    assert_eq!(v, (true, true, true), "string->bytes ok all directions");
}

#[test]
fn matrix_int64_to_int32_backward_only() {
    // Arrange
    let compiler = ProtobufCompiler::new();

    // Act
    let v = type_change_verdicts(&compiler, "int64", "int32");

    // Assert
    assert_eq!(v, (true, false, false), "int64->int32: Backward ok only");
}

/// Same wire type (both varint) but NOT on the allowlist → breaking all dirs.
#[test]
fn matrix_int32_to_sint32_breaks_all_directions() {
    // Arrange
    let compiler = ProtobufCompiler::new();

    // Act — int32 and sint32 are both varint, but the pair is not allowlisted.
    let v = type_change_verdicts(&compiler, "int32", "sint32");

    // Assert
    assert_eq!(
        v,
        (false, false, false),
        "non-allowlisted same-wire change breaks all"
    );
}

/// BUG: `enum→int32` is wire-compatible (enum's wire type IS varint, same as
/// int32), so it should be FULLY compatible. But `typecompat::wire_of` treats
/// any non-scalar name (the enum's type name) as length-delimited, so the change
/// is misclassified as cross-wire → Breaking in ALL directions. The correct
/// expectation is (true, true, true).
///
/// Repro: compare `message M { Color a = 1; }` against `message M { int32 a = 1; }`
/// (Color being an enum). check_compatibility returns a violation
/// "field 1 type 'Color'->'int32' is breaking" even for Backward.
#[test]
fn matrix_enum_to_int32_ok_all_directions() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let old = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nenum Color { C0 = 0; }\nmessage M { Color a = 1; }\n",
        ),
        "M",
    );
    let new = decl(
        &parse_to_objects(
            "syntax=\"proto3\";\nenum Color { C0 = 0; }\nmessage M { int32 a = 1; }\n",
        ),
        "M",
    );

    // Act
    let (b, f, fu) = verdicts(&compiler, &old, &new);

    // Assert — enum and int32 share the varint wire type, so this is FULL-compatible.
    assert_eq!(
        (b, f, fu),
        (true, true, true),
        "enum->int32 ok all directions"
    );
}

/// Cross-wire type changes are enforced at the MUTATION-VALIDATION layer (the
/// `ChangeFieldType` op rejects them) rather than surfacing as a compat verdict.
#[test]
fn cross_wire_change_rejected_at_mutation_validation() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeFieldType(OpChangeFieldType {
            message_name: "M".into(),
            field_name: "a".into(),
            new_type: "string".into(),
        }),
    );

    // Act
    let result = compiler.apply_mutation(&objects, &op);

    // Assert — int32->string crosses the wire boundary; rejected as INVALID here,
    // not deferred to a compatibility check.
    assert!(
        matches!(
            result,
            Err(schemahub_types::MutationError::InvalidOperation(_))
        ),
        "cross-wire ChangeFieldType must be rejected at mutation validation; got {result:?}"
    );
}

/// The allowlisted ChangeFieldType (int32->int64) is accepted at mutation time.
#[test]
fn allowlisted_change_field_type_is_accepted() {
    // Arrange
    let compiler = ProtobufCompiler::new();
    let objects = parse_to_objects("syntax=\"proto3\";\nmessage M { int32 a = 1; }\n");
    let op = mutation(
        "m.proto",
        ProtoOp::ChangeFieldType(OpChangeFieldType {
            message_name: "M".into(),
            field_name: "a".into(),
            new_type: "int64".into(),
        }),
    );

    // Act
    let effect = compiler.apply_mutation(&objects, &op).expect("mutation ok");

    // Assert
    let printed = print_with(&compiler, &objects, &effect);
    assert!(printed.contains("int64 a = 1"), "got:\n{printed}");
}
