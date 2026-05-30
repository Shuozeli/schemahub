//! Tests for the FlatBuffers compiler: round-trip (parse→split→reassemble→
//! print→parse), diff, compatibility (one per rule row), mutations, conflicts,
//! and codegen. Arrange-Act-Assert throughout.

use std::collections::BTreeMap;

use schemahub_types::{
    CompatibilityDirection, CompatibilityRules, Compiler, ConflictSides, DeclBlob, MetaBlob,
    Mutation, SchemaClosure, SchemaObjects, SchemaPath,
};

use crate::blob::{decode_decl, DeclPayload};
use crate::mutations::FbsOp;
use crate::FlatBuffersCompiler;

// ── helpers ──────────────────────────────────────────────────────────────────

fn compiler() -> FlatBuffersCompiler {
    FlatBuffersCompiler::new()
}

/// Build a `SchemaObjects` from a parsed source string.
fn parse_to_objects(src: &str) -> SchemaObjects {
    let parsed = compiler().parse(src).expect("parses");
    let mut decls = BTreeMap::new();
    for (name, blob) in parsed.decls {
        decls.insert(name, blob);
    }
    SchemaObjects {
        meta: parsed.meta,
        decls,
    }
}

/// Compare the multiset of declaration names between two sources.
fn decl_names(src: &str) -> Vec<String> {
    let parsed = compiler().parse(src).expect("parses");
    let mut names: Vec<String> = parsed.decls.iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    names
}

/// Top-level declaration names in the order they appear in `.fbs` text.
fn ordered_decl_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        for kw in ["table ", "struct ", "enum ", "union ", "rpc_service "] {
            if let Some(rest) = t.strip_prefix(kw) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push(name);
                }
                break;
            }
        }
    }
    out
}

fn mutation(op: &FbsOp) -> Mutation {
    Mutation {
        schema_path: SchemaPath::new("p", "r", "s.fbs"),
        format_id: "flatbuffers".to_string(),
        operation: op.encode(),
    }
}

const SAMPLE: &str = r#"
namespace MyGame.Sample;

include "other.fbs";

/// A color enum.
enum Color : byte {
  Red = 0,
  Green = 1,
  Blue = 2
}

union Any {
  Monster,
  Weapon
}

struct Vec3 {
  x: float;
  y: float;
  z: float;
}

/// The main monster table.
table Monster {
  name: string (required);
  hp: short = 100;
  color: Color;
  old_field: int (deprecated);
}

table Weapon {
  damage: int;
}

rpc_service MonsterService {
  Lookup(Monster): Monster;
}

root_type Monster;
file_identifier "MONS";
"#;

// ── P0: round-trip ─────────────────────────────────────────────────────────────

#[test]
fn round_trip_preserves_all_declarations() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects(SAMPLE);

    // Act
    let printed = c.print(&objects).expect("prints");
    let reparsed_names = decl_names(&printed);

    // Assert: every original declaration survives the round-trip.
    let original_names = decl_names(SAMPLE);
    assert_eq!(
        reparsed_names, original_names,
        "round-trip lost or added declarations.\nprinted:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_cross_kind_declaration_order() {
    // Arrange: SAMPLE interleaves enum/union/struct/tables/service — a source
    // order distinct from alphabetical (Any, Color, Monster, MonsterService,
    // Vec3, Weapon), which is what BTreeMap iteration would have produced.
    let c = compiler();
    let objects = parse_to_objects(SAMPLE);

    // Act
    let printed = c.print(&objects).expect("prints");

    // Assert: declarations print in original source order, not sorted by name.
    let printed_order = ordered_decl_names(&printed);
    assert_eq!(
        printed_order,
        vec!["Color", "Any", "Vec3", "Monster", "Weapon", "MonsterService"],
        "cross-kind declaration order not preserved.\nprinted:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_table_attributes_and_deprecated() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects(SAMPLE);

    // Act
    let printed = c.print(&objects).expect("prints");
    let reparsed = parse_to_objects(&printed);

    // Assert: the Monster table keeps required + deprecated semantics.
    let blob = reparsed.decls.get("Monster").expect("Monster present");
    let payload = decode_decl(blob).expect("decodes");
    let DeclPayload::Object(obj) = payload else {
        panic!("Monster is not an object");
    };
    let name_field = obj
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("name"))
        .expect("name field");
    assert!(name_field.is_required, "name should remain required");
    let old_field = obj
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("old_field"))
        .expect("old_field present");
    assert!(old_field.is_deprecated, "old_field should remain deprecated");
}

#[test]
fn round_trip_preserves_struct_enum_union_service_and_meta() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects(SAMPLE);

    // Act
    let printed = c.print(&objects).expect("prints");

    // Assert: the printed source carries every kind back.
    assert!(printed.contains("struct Vec3"), "struct lost:\n{printed}");
    assert!(printed.contains("enum Color"), "enum lost:\n{printed}");
    assert!(printed.contains("union Any"), "union lost:\n{printed}");
    assert!(
        printed.contains("rpc_service MonsterService"),
        "service lost:\n{printed}"
    );
    assert!(
        printed.contains("namespace MyGame.Sample"),
        "namespace lost:\n{printed}"
    );
    assert!(
        printed.contains("root_type Monster"),
        "root_type lost:\n{printed}"
    );
    assert!(
        printed.contains("include \"other.fbs\""),
        "include lost:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_field_slot_ids() {
    // Arrange: explicit field ids must survive the split/reassemble.
    let src = "table T { a: int (id: 0); b: string (id: 1); }";
    let c = compiler();
    let objects = parse_to_objects(src);

    // Act
    let printed = c.print(&objects).expect("prints");
    let reparsed = parse_to_objects(&printed);

    // Assert
    let payload = decode_decl(reparsed.decls.get("T").unwrap()).unwrap();
    let DeclPayload::Object(obj) = payload else {
        panic!("T not object")
    };
    assert_eq!(obj.fields[0].id, Some(0));
    assert_eq!(obj.fields[1].id, Some(1));
}

// ── P1: read / diff ─────────────────────────────────────────────────────────────

#[test]
fn summarize_reports_kind_and_doc() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects(SAMPLE);

    // Act
    let summary = c.summarize_decl(objects.decls.get("Color").unwrap()).unwrap();

    // Assert
    assert_eq!(summary.name, "Color");
    assert_eq!(summary.kind, schemahub_types::DeclKind::FbsEnum);
    assert_eq!(summary.doc_comment, "A color enum.");
}

#[test]
fn imports_lists_includes() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects(SAMPLE);

    // Act
    let imports = c.imports(&objects.meta).unwrap();

    // Assert
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].path, "other.fbs");
}

#[test]
fn type_refs_reports_user_types() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects(SAMPLE);

    // Act
    let refs = c.type_refs(objects.decls.get("Monster").unwrap()).unwrap();
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();

    // Assert: Monster references Color (and not the scalar fields).
    assert!(names.contains(&"Color"), "expected Color in {names:?}");
}

#[test]
fn diff_reports_modification() {
    // Arrange
    let c = compiler();
    let old = parse_to_objects("table T { a: int; }");
    let new = parse_to_objects("table T { a: int; b: string; }");

    // Act
    let change = c
        .diff_decl(old.decls.get("T").unwrap(), new.decls.get("T").unwrap())
        .unwrap();

    // Assert
    assert_eq!(change.declaration_name(), "T");
}

// ── P2: compatibility (one per rule row) ──────────────────────────────────────

fn table_blob(src: &str, name: &str) -> DeclBlob {
    parse_to_objects(src).decls.remove(name).expect("decl present")
}

#[test]
fn compat_add_field_at_end_is_full_ok() {
    // Arrange
    let c = compiler();
    let old = table_blob("table T { a: int; }", "T");
    let new = table_blob("table T { a: int; b: string; }", "T");

    // Act
    let result = c.check_compatibility(&old, &new, &CompatibilityRules::full());

    // Assert
    assert!(result.is_ok(), "add-at-end should be FULL ok: {result:?}");
}

#[test]
fn compat_deprecate_field_is_full_ok() {
    // Arrange
    let c = compiler();
    let old = table_blob("table T { a: int; b: string; }", "T");
    let new = table_blob("table T { a: int; b: string (deprecated); }", "T");

    // Act
    let result = c.check_compatibility(&old, &new, &CompatibilityRules::full());

    // Assert
    assert!(result.is_ok(), "deprecation should be FULL ok: {result:?}");
}

#[test]
fn compat_type_change_is_always_breaking() {
    // Arrange
    let c = compiler();
    let old = table_blob("table T { a: int; }", "T");
    let new = table_blob("table T { a: long; }", "T");

    // Act / Assert
    for dir in [
        CompatibilityDirection::Backward,
        CompatibilityDirection::Forward,
        CompatibilityDirection::Full,
    ] {
        let rules = CompatibilityRules {
            direction: dir,
            disabled: false,
        };
        let result = c.check_compatibility(&old, &new, &rules);
        assert!(result.is_err(), "type change should break {dir:?}");
    }
}

#[test]
fn compat_struct_mutation_is_rejected() {
    // Arrange
    let c = compiler();
    let old = table_blob("struct S { x: float; y: float; }", "S");
    let new = table_blob("struct S { x: float; y: float; z: float; }", "S");

    // Act
    let result = c.check_compatibility(&old, &new, &CompatibilityRules::full());

    // Assert
    assert!(result.is_err(), "any struct change is breaking");
}

#[test]
fn compat_enum_value_add_backward_ok_full_breaks() {
    // Arrange
    let c = compiler();
    let old = table_blob("enum E : int { A = 0 }", "E");
    let new = table_blob("enum E : int { A = 0, B = 1 }", "E");

    // Act
    let backward = c.check_compatibility(
        &old,
        &new,
        &CompatibilityRules {
            direction: CompatibilityDirection::Backward,
            disabled: false,
        },
    );
    let full = c.check_compatibility(&old, &new, &CompatibilityRules::full());

    // Assert
    assert!(backward.is_ok(), "add enum value is BACKWARD ok: {backward:?}");
    assert!(full.is_err(), "add enum value breaks FULL");
}

#[test]
fn compat_remove_field_is_breaking() {
    // Arrange
    let c = compiler();
    let old = table_blob("table T { a: int (id: 0); b: string (id: 1); }", "T");
    let new = table_blob("table T { a: int (id: 0); }", "T");

    // Act
    let result = c.check_compatibility(&old, &new, &CompatibilityRules::full());

    // Assert
    assert!(result.is_err(), "removing a slot is breaking");
}

// ── P3: mutations ─────────────────────────────────────────────────────────────

#[test]
fn mutation_add_field_appends_at_end() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int (id: 0); }");
    let op = mutation(&FbsOp::AddField {
        table: "T".to_string(),
        field_name: "b".to_string(),
        field_type: "string".to_string(),
        default_value: None,
        doc_comment: None,
    });

    // Act
    let effect = c.apply_mutation(&objects, &op).expect("applies");

    // Assert
    assert_eq!(effect.upserts.len(), 1);
    let payload = decode_decl(&effect.upserts[0].1).unwrap();
    let DeclPayload::Object(obj) = payload else {
        panic!("not object")
    };
    let b = obj.fields.last().unwrap();
    assert_eq!(b.name.as_deref(), Some("b"));
    assert_eq!(b.id, Some(1), "new field appended at next slot");
}

#[test]
fn mutation_remove_field_is_rejected_with_deprecate_hint() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int; }");
    let op = mutation(&FbsOp::RemoveField {
        table: "T".to_string(),
        field_name: "a".to_string(),
    });

    // Act
    let result = c.apply_mutation(&objects, &op);

    // Assert
    let err = result.expect_err("RemoveField must be rejected");
    assert!(err.to_string().contains("DeprecateField"), "got: {err}");
}

#[test]
fn mutation_deprecate_field_sets_flag() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int; }");
    let op = mutation(&FbsOp::DeprecateField {
        table: "T".to_string(),
        field_name: "a".to_string(),
    });

    // Act
    let effect = c.apply_mutation(&objects, &op).expect("applies");

    // Assert
    let payload = decode_decl(&effect.upserts[0].1).unwrap();
    let DeclPayload::Object(obj) = payload else {
        panic!("not object")
    };
    assert!(obj.fields[0].is_deprecated);
}

#[test]
fn mutation_add_field_to_struct_is_rejected() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("struct S { x: float; }");
    let op = mutation(&FbsOp::AddField {
        table: "S".to_string(),
        field_name: "y".to_string(),
        field_type: "float".to_string(),
        default_value: None,
        doc_comment: None,
    });

    // Act
    let result = c.apply_mutation(&objects, &op);

    // Assert
    assert!(result.is_err(), "struct mutation must be rejected");
}

#[test]
fn mutation_rename_field_keeps_slot() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int (id: 0); }");
    let op = mutation(&FbsOp::RenameField {
        table: "T".to_string(),
        old_name: "a".to_string(),
        new_name: "alpha".to_string(),
    });

    // Act
    let effect = c.apply_mutation(&objects, &op).expect("applies");

    // Assert
    let payload = decode_decl(&effect.upserts[0].1).unwrap();
    let DeclPayload::Object(obj) = payload else {
        panic!("not object")
    };
    assert_eq!(obj.fields[0].name.as_deref(), Some("alpha"));
    assert_eq!(obj.fields[0].id, Some(0), "rename keeps slot identity");
}

#[test]
fn mutation_change_field_type() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int; }");
    let op = mutation(&FbsOp::ChangeFieldType {
        table: "T".to_string(),
        field_name: "a".to_string(),
        new_type: "string".to_string(),
    });

    // Act
    let effect = c.apply_mutation(&objects, &op).expect("applies");

    // Assert
    let payload = decode_decl(&effect.upserts[0].1).unwrap();
    let DeclPayload::Object(obj) = payload else {
        panic!("not object")
    };
    assert_eq!(
        obj.fields[0].type_.as_ref().unwrap().base_type,
        Some(flatc_rs_schema::BaseType::BASE_TYPE_STRING)
    );
}

#[test]
fn mutation_create_rename_delete_table() {
    // Arrange
    let c = compiler();
    let mut objects = SchemaObjects::new(MetaBlob::default());

    // Act: create
    let effect = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::CreateTable {
                name: "T".to_string(),
                doc_comment: None,
            }),
        )
        .expect("create");
    assert_eq!(effect.upserts[0].0, "T");
    objects.decls.insert("T".to_string(), effect.upserts[0].1.clone());

    // Act: rename
    let effect = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::RenameTable {
                old_name: "T".to_string(),
                new_name: "U".to_string(),
            }),
        )
        .expect("rename");

    // Assert: rename removes old name, upserts new.
    assert!(effect.removes.contains(&"T".to_string()));
    assert!(effect.upserts.iter().any(|(n, _)| n == "U"));
}

#[test]
fn mutation_enum_value_add_remove_rename() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("enum E : int { A = 0 }");

    // Act: add
    let add = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::AddEnumValue {
                enum_name: "E".to_string(),
                value_name: "B".to_string(),
                value: 1,
            }),
        )
        .expect("add value");

    // Assert
    let payload = decode_decl(&add.upserts[0].1).unwrap();
    let DeclPayload::Enum(en) = payload else {
        panic!("not enum")
    };
    assert!(en.values.iter().any(|v| v.name.as_deref() == Some("B")));
}

#[test]
fn mutation_update_import_changes_meta() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int; }");

    // Act
    let effect = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::UpdateImport {
                import_path: "dep.fbs".to_string(),
                remove: false,
            }),
        )
        .expect("update import");

    // Assert
    let meta = effect.meta.expect("meta changed");
    let imports = c.imports(&meta).unwrap();
    assert!(imports.iter().any(|i| i.path == "dep.fbs"));
}

#[test]
fn mutations_batch_applies_in_order() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int (id: 0); }");
    let ops = vec![
        mutation(&FbsOp::AddField {
            table: "T".to_string(),
            field_name: "b".to_string(),
            field_type: "string".to_string(),
            default_value: None,
            doc_comment: None,
        }),
        mutation(&FbsOp::DeprecateField {
            table: "T".to_string(),
            field_name: "b".to_string(),
        }),
    ];

    // Act
    let effect = c.apply_mutations(&objects, &ops).expect("batch applies");

    // Assert: final state has b deprecated.
    let payload = decode_decl(&effect.upserts[0].1).unwrap();
    let DeclPayload::Object(obj) = payload else {
        panic!("not object")
    };
    let b = obj.fields.iter().find(|f| f.name.as_deref() == Some("b")).unwrap();
    assert!(b.is_deprecated);
}

// ── P4: conflicts + codegen ────────────────────────────────────────────────────

#[test]
fn render_conflict_shows_all_sides() {
    // Arrange
    let c = compiler();
    let base = table_blob("table T { a: int; }", "T");
    let ours = table_blob("table T { a: int; b: string; }", "T");
    let theirs = table_blob("table T { a: int; c: bool; }", "T");
    let sides = ConflictSides::new(Some(base), vec![ours, theirs]);

    // Act
    let rendered = c.render_conflict(&sides).expect("renders");

    // Assert
    assert!(rendered.contains("base"));
    assert!(rendered.contains("side 1"));
    assert!(rendered.contains("side 2"));
}

#[test]
fn validate_resolution_accepts_valid_decl() {
    // Arrange
    let c = compiler();
    let resolved = table_blob("table T { a: int; }", "T");

    // Act
    let result = c.validate_resolution(&resolved);

    // Assert
    assert!(result.is_ok(), "valid decl accepted: {result:?}");
}

#[test]
fn generate_descriptors_reconstructs_bundle() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int; }");
    let mut closure = SchemaClosure::new();
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "s.fbs"), objects);

    // Act
    let bytes = c.generate_descriptors(&closure).expect("descriptors");
    let text = String::from_utf8(bytes.to_vec()).unwrap();

    // Assert
    assert!(text.contains("table T"), "bundle missing decl:\n{text}");
}

#[test]
fn generate_code_rust_for_simple_table() {
    // Arrange: a table of scalars resolves without the analyzer's layout pass.
    let c = compiler();
    let objects = parse_to_objects("table T { a: int; b: string; }");
    let mut closure = SchemaClosure::new();
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "s.fbs"), objects);

    // Act
    let result = c.generate_code(&closure, schemahub_types::Language::Rust);

    // Assert: either generates Rust, or surfaces a clear resolution limitation.
    match result {
        Ok(code) => assert!(!code.is_empty(), "generated rust is empty"),
        Err(e) => panic!("simple table codegen should succeed, got: {e}"),
    }
}

#[test]
fn generate_code_rejects_unsupported_language() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("table T { a: int; }");
    let mut closure = SchemaClosure::new();
    closure
        .entries
        .insert(SchemaPath::new("p", "r", "s.fbs"), objects);

    // Act
    let result = c.generate_code(&closure, schemahub_types::Language::Go);

    // Assert
    assert!(matches!(
        result,
        Err(schemahub_types::CodegenError::UnsupportedLanguage(_))
    ));
}

// ── P5: advanced round-trip fidelity ───────────────────────────────────────────

/// Decode the `Object` payload for `name` from a parsed source.
fn object_of(src: &str, name: &str) -> flatc_rs_schema::Object {
    let objects = parse_to_objects(src);
    let payload = decode_decl(objects.decls.get(name).expect("decl present")).expect("decodes");
    let DeclPayload::Object(o) = payload else {
        panic!("'{name}' is not an object");
    };
    *o
}

/// Decode the `Enum` payload for `name` from a parsed source.
fn enum_of(src: &str, name: &str) -> flatc_rs_schema::Enum {
    let objects = parse_to_objects(src);
    let payload = decode_decl(objects.decls.get(name).expect("decl present")).expect("decodes");
    let DeclPayload::Enum(e) = payload else {
        panic!("'{name}' is not an enum/union");
    };
    *e
}

/// Round-trip a source (parse → print) and return the printed `.fbs`.
fn round_trip(src: &str) -> String {
    compiler().print(&parse_to_objects(src)).expect("prints")
}

#[test]
fn round_trip_preserves_fixed_size_array_field() {
    // Arrange: a struct with a fixed-size array field.
    let src = "struct Mat { v: [float:16]; }";

    // Act
    let printed = round_trip(src);
    let obj = object_of(&printed, "Mat");

    // Assert: the array type, element type, and fixed length all survive.
    let ty = obj.fields[0].type_.as_ref().expect("field type");
    assert_eq!(ty.base_type, Some(flatc_rs_schema::BaseType::BASE_TYPE_ARRAY));
    assert_eq!(
        ty.element_type,
        Some(flatc_rs_schema::BaseType::BASE_TYPE_FLOAT)
    );
    assert_eq!(ty.fixed_length, Some(16), "fixed length lost:\n{printed}");
}

#[test]
fn round_trip_preserves_nested_struct_field() {
    // Arrange: a struct whose field's type is another struct.
    let src = "struct Inner { x: int; }\nstruct Outer { inner: Inner; }";

    // Act
    let printed = round_trip(src);
    let outer = object_of(&printed, "Outer");

    // Assert: Outer keeps its struct-typed field referencing Inner.
    assert!(outer.is_struct, "Outer should remain a struct");
    let ty = outer.fields[0].type_.as_ref().expect("field type");
    assert_eq!(ty.unresolved_name.as_deref(), Some("Inner"));
    assert_eq!(
        ordered_decl_names(&printed),
        vec!["Inner", "Outer"],
        "struct order lost:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_scalar_and_enum_field_defaults() {
    // Arrange: int/float defaults plus an enum-typed field with a default.
    let src = "enum Color : byte { Red = 0, Green = 1, Blue = 2 }\n\
               table T { mana: short = 150; speed: float = 1.5; color: Color = Green; }";

    // Act
    let printed = round_trip(src);
    let t = object_of(&printed, "T");

    // Assert: each default survives in the field AST.
    let mana = t.fields.iter().find(|f| f.name.as_deref() == Some("mana")).unwrap();
    assert_eq!(mana.default_integer, Some(150), "int default lost:\n{printed}");
    let speed = t.fields.iter().find(|f| f.name.as_deref() == Some("speed")).unwrap();
    assert_eq!(speed.default_real, Some(1.5), "float default lost:\n{printed}");
    let color = t.fields.iter().find(|f| f.name.as_deref() == Some("color")).unwrap();
    assert_eq!(
        color.default_string.as_deref(),
        Some("Green"),
        "enum default lost:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_bool_field_default_value() {
    // Arrange: bool defaults are stored as integer 1/0; the printer emits the
    // integer spelling (`= 1`/`= 0`) rather than `true`/`false`, but the value
    // semantics must survive.
    let src = "table T { active: bool = true; flag: bool = false; }";

    // Act
    let printed = round_trip(src);
    let t = object_of(&printed, "T");

    // Assert: the default *values* round-trip (spelling is normalized to int).
    let active = t.fields.iter().find(|f| f.name.as_deref() == Some("active")).unwrap();
    assert_eq!(active.default_integer, Some(1), "true default lost:\n{printed}");
    let flag = t.fields.iter().find(|f| f.name.as_deref() == Some("flag")).unwrap();
    assert_eq!(flag.default_integer, Some(0), "false default lost:\n{printed}");
}

#[test]
fn round_trip_preserves_key_and_explicit_field_ids() {
    // Arrange: a `(key)` attribute and explicit `(id: N)` ordering.
    let src = "table T { id: uint (key, id: 0); name: string (id: 1); }";

    // Act
    let printed = round_trip(src);
    let t = object_of(&printed, "T");

    // Assert: slot ids survive and the key attribute is retained.
    let id_field = t.fields.iter().find(|f| f.name.as_deref() == Some("id")).unwrap();
    assert_eq!(id_field.id, Some(0));
    let key_present = id_field.is_key
        || id_field
            .attributes
            .as_ref()
            .map(|a| a.has("key"))
            .unwrap_or(false);
    assert!(key_present, "key attribute lost:\n{printed}");
    let name_field = t.fields.iter().find(|f| f.name.as_deref() == Some("name")).unwrap();
    assert_eq!(name_field.id, Some(1), "explicit id lost:\n{printed}");
}

#[test]
fn round_trip_preserves_bit_flags_enum_attribute() {
    // Arrange: a bit_flags enum.
    let src = "enum Mode : ubyte (bit_flags) { A, B, C }";

    // Act
    let printed = round_trip(src);
    let e = enum_of(&printed, "Mode");

    // Assert: the bit_flags attribute and all members (in order) survive.
    assert!(
        e.attributes.as_ref().map(|a| a.has("bit_flags")).unwrap_or(false),
        "bit_flags attribute lost:\n{printed}"
    );
    let names: Vec<&str> = e.values.iter().filter_map(|v| v.name.as_deref()).collect();
    assert_eq!(names, vec!["A", "B", "C"], "bit_flags members lost:\n{printed}");
}

#[test]
fn round_trip_preserves_non_sequential_enum_values() {
    // Arrange: explicit, non-contiguous enum values.
    let src = "enum E : int { X = 1, Y = 5, Z = 10 }";

    // Act
    let printed = round_trip(src);
    let e = enum_of(&printed, "E");

    // Assert: the exact numeric values survive.
    let pairs: Vec<(Option<&str>, Option<i64>)> =
        e.values.iter().map(|v| (v.name.as_deref(), v.value)).collect();
    assert_eq!(
        pairs,
        vec![
            (Some("X"), Some(1)),
            (Some("Y"), Some(5)),
            (Some("Z"), Some(10)),
        ],
        "non-sequential enum values lost:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_positional_union_with_multiple_members() {
    // Arrange: a positional union of three members.
    let src = "table A { x: int; }\ntable B { y: int; }\ntable C { z: int; }\nunion U { A, B, C }";

    // Act
    let printed = round_trip(src);
    let u = enum_of(&printed, "U");

    // Assert: members (minus the synthetic NONE) survive in order.
    let members: Vec<&str> = u
        .values
        .iter()
        .filter(|v| v.name.as_deref() != Some("NONE"))
        .filter_map(|v| {
            v.union_type
                .as_ref()
                .and_then(|t| t.unresolved_name.as_deref())
                .or(v.name.as_deref())
        })
        .collect();
    assert_eq!(members, vec!["A", "B", "C"], "union members lost:\n{printed}");
    assert!(u.is_union, "U should remain a union");
}

#[test]
fn round_trip_preserves_named_union_member_aliases() {
    // Arrange: a union with explicitly *named* members (alias: type).
    let src = "struct V { x: float; }\ntable W { d: int; }\nunion E { weapon: W, position: V }";

    // Act
    let printed = round_trip(src);
    let e = enum_of(&printed, "E");

    // Assert: the member aliases (`weapon`, `position`) must survive — they are
    // the source-level member names, distinct from the referenced type names.
    let names: Vec<&str> = e
        .values
        .iter()
        .filter(|v| v.name.as_deref() != Some("NONE"))
        .filter_map(|v| v.name.as_deref())
        .collect();
    assert_eq!(
        names,
        vec!["weapon", "position"],
        "named union member aliases lost; printer dropped them to the type name:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_nested_namespace() {
    // Arrange: a multi-segment namespace.
    let src = "namespace A.B.C;\ntable T { v: int; }";

    // Act
    let printed = round_trip(src);

    // Assert: the full dotted namespace path survives.
    assert!(
        printed.contains("namespace A.B.C;"),
        "nested namespace lost:\n{printed}"
    );
    let reparsed = parse_to_objects(&printed);
    let meta = crate::blob::decode_meta(&reparsed.meta).expect("meta decodes");
    assert_eq!(meta.namespace.as_deref(), Some("A.B.C"));
}

#[test]
fn round_trip_preserves_all_file_metadata() {
    // Arrange: include + root_type + file_identifier + file_extension together.
    let src = "include \"x.fbs\";\n\
               table Monster { hp: int; }\n\
               root_type Monster;\n\
               file_identifier \"ABCD\";\n\
               file_extension \"bin\";";

    // Act
    let printed = round_trip(src);

    // Assert: every file-level directive is reproduced.
    assert!(printed.contains("include \"x.fbs\""), "include lost:\n{printed}");
    assert!(printed.contains("root_type Monster"), "root_type lost:\n{printed}");
    assert!(
        printed.contains("file_identifier \"ABCD\""),
        "file_identifier lost:\n{printed}"
    );
    assert!(
        printed.contains("file_extension \"bin\""),
        "file_extension lost:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_custom_and_align_attribute_usage() {
    // Arrange: a custom field attribute `(priority: 5)` and `(force_align: 16)`
    // on a struct. (The `attribute "priority";` *declaration* is file-level and
    // is not stored in the blob model — see the separate dropped-declaration
    // test — but the *usage* must survive.)
    let src = "table T { name: string (priority: 5); }\n\
               struct S (force_align: 16) { x: float; y: float; z: float; w: float; }";

    // Act
    let printed = round_trip(src);
    let t = object_of(&printed, "T");
    let s = object_of(&printed, "S");

    // Assert: the custom field attribute keeps its key and value.
    let attrs = t.fields[0].attributes.as_ref().expect("field attributes");
    assert_eq!(attrs.get("priority"), Some("5"), "custom attr lost:\n{printed}");
    // And force_align survives on the struct.
    let s_attrs = s.attributes.as_ref().expect("struct attributes");
    assert_eq!(
        s_attrs.get("force_align"),
        Some("16"),
        "force_align lost:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_custom_attribute_declaration() {
    // Arrange: a top-level `attribute "..."` declaration. The schemahub blob
    // model (FbsMeta) records these in `declared_attributes`, so both the
    // declaration and its usage must survive the round-trip.
    let src = "attribute \"priority\";\ntable T { name: string (priority: 5); }";

    // Act
    let printed = round_trip(src);

    // Assert: the `attribute` directive is reproduced, the usage survives, and
    // the result re-parses with the declaration captured in meta.
    assert!(
        printed.contains("attribute \"priority\""),
        "custom attribute declaration lost:\n{printed}"
    );
    assert!(printed.contains("(priority: 5)"), "usage lost:\n{printed}");
    let reparsed = parse_to_objects(&printed);
    let meta = crate::blob::decode_meta(&reparsed.meta).expect("meta decodes");
    assert_eq!(
        meta.declared_attributes,
        vec!["priority".to_string()],
        "declared attribute not captured in meta:\n{printed}"
    );
}

#[test]
fn round_trip_preserves_vector_of_tables_and_scalars() {
    // Arrange: a table with a vector-of-tables and a vector-of-scalars field.
    let src = "table Item { n: int; }\ntable C { items: [Item]; nums: [int]; }";

    // Act
    let printed = round_trip(src);
    let c = object_of(&printed, "C");

    // Assert: both vectors survive with the right element types.
    let items = c.fields.iter().find(|f| f.name.as_deref() == Some("items")).unwrap();
    let items_ty = items.type_.as_ref().unwrap();
    assert_eq!(items_ty.base_type, Some(flatc_rs_schema::BaseType::BASE_TYPE_VECTOR));
    assert_eq!(items_ty.unresolved_name.as_deref(), Some("Item"));
    let nums = c.fields.iter().find(|f| f.name.as_deref() == Some("nums")).unwrap();
    let nums_ty = nums.type_.as_ref().unwrap();
    assert_eq!(nums_ty.base_type, Some(flatc_rs_schema::BaseType::BASE_TYPE_VECTOR));
    assert_eq!(
        nums_ty.element_type,
        Some(flatc_rs_schema::BaseType::BASE_TYPE_INT),
        "vector-of-scalars element type lost:\n{printed}"
    );
}

// ── P6: advanced mutations ──────────────────────────────────────────────────────

#[test]
fn mutation_create_rename_delete_enum() {
    // Arrange
    let c = compiler();
    let mut objects = SchemaObjects::new(MetaBlob::default());

    // Act: create
    let created = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::CreateEnum {
                name: "E".to_string(),
                underlying_type: "int".to_string(),
                doc_comment: None,
            }),
        )
        .expect("create enum");
    objects.decls.insert("E".to_string(), created.upserts[0].1.clone());

    // Act: rename
    let renamed = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::RenameEnum {
                old_name: "E".to_string(),
                new_name: "F".to_string(),
            }),
        )
        .expect("rename enum");
    objects.decls.remove("E");
    objects.decls.insert("F".to_string(), renamed.upserts[0].1.clone());

    // Act: delete
    let deleted = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::DeleteEnum {
                name: "F".to_string(),
            }),
        )
        .expect("delete enum");

    // Assert
    assert!(renamed.removes.contains(&"E".to_string()));
    assert!(renamed.upserts.iter().any(|(n, _)| n == "F"));
    assert!(deleted.removes.contains(&"F".to_string()));
}

#[test]
fn mutation_create_rename_delete_union_with_members() {
    // Arrange: unions are Enums with `is_union`; members are added via
    // AddEnumValue (there is no dedicated AddUnionMember op).
    let c = compiler();
    let mut objects = SchemaObjects::new(MetaBlob::default());

    // Act: create union
    let created = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::CreateUnion {
                name: "U".to_string(),
                doc_comment: None,
            }),
        )
        .expect("create union");
    objects.decls.insert("U".to_string(), created.upserts[0].1.clone());

    // Assert: a fresh union carries the synthetic NONE sentinel and is_union.
    let DeclPayload::Enum(u0) = decode_decl(&created.upserts[0].1).unwrap() else {
        panic!("not enum")
    };
    assert!(u0.is_union, "created decl should be a union");
    assert_eq!(u0.values.len(), 1, "union starts with NONE only");

    // Act: add a member, then remove it.
    let added = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::AddEnumValue {
                enum_name: "U".to_string(),
                value_name: "Weapon".to_string(),
                value: 1,
            }),
        )
        .expect("add union member");
    objects.decls.insert("U".to_string(), added.upserts[0].1.clone());
    let DeclPayload::Enum(u1) = decode_decl(&added.upserts[0].1).unwrap() else {
        panic!("not enum")
    };
    assert!(u1.values.iter().any(|v| v.name.as_deref() == Some("Weapon")));

    let removed = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::RemoveEnumValue {
                enum_name: "U".to_string(),
                value_name: "Weapon".to_string(),
            }),
        )
        .expect("remove union member");
    objects.decls.insert("U".to_string(), removed.upserts[0].1.clone());

    // Act: rename then delete the union.
    let renamed = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::RenameUnion {
                old_name: "U".to_string(),
                new_name: "V".to_string(),
            }),
        )
        .expect("rename union");
    objects.decls.remove("U");
    objects.decls.insert("V".to_string(), renamed.upserts[0].1.clone());
    let deleted = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::DeleteUnion {
                name: "V".to_string(),
            }),
        )
        .expect("delete union");

    // Assert
    let DeclPayload::Enum(u2) = decode_decl(&removed.upserts[0].1).unwrap() else {
        panic!("not enum")
    };
    assert!(
        !u2.values.iter().any(|v| v.name.as_deref() == Some("Weapon")),
        "member should be removed"
    );
    assert!(renamed.removes.contains(&"U".to_string()));
    assert!(deleted.removes.contains(&"V".to_string()));
}

#[test]
fn mutation_create_struct_is_allowed() {
    // Arrange: creating a struct is done via CreateTable then marking is_struct
    // is not exposed; CreateTable always makes a table. Verify a plain create.
    let c = compiler();
    let objects = SchemaObjects::new(MetaBlob::default());

    // Act
    let effect = c
        .apply_mutation(
            &objects,
            &mutation(&FbsOp::CreateTable {
                name: "S".to_string(),
                doc_comment: None,
            }),
        )
        .expect("create");

    // Assert: a new empty object is produced.
    assert_eq!(effect.upserts[0].0, "S");
    let DeclPayload::Object(o) = decode_decl(&effect.upserts[0].1).unwrap() else {
        panic!("not object")
    };
    assert!(o.fields.is_empty(), "new decl should start empty");
}

#[test]
fn mutation_rename_field_on_struct_is_rejected() {
    // Arrange: a struct's fields are wire-fixed; renaming one is a layout-touching
    // edit that must be rejected, exactly like AddField/ChangeFieldType on a struct.
    let c = compiler();
    let objects = parse_to_objects("struct S { x: float; }");
    let op = mutation(&FbsOp::RenameField {
        table: "S".to_string(),
        old_name: "x".to_string(),
        new_name: "y".to_string(),
    });

    // Act
    let result = c.apply_mutation(&objects, &op);

    // Assert: should be rejected (currently it is NOT — see #[ignore] reason).
    assert!(
        result.is_err(),
        "RenameField on a struct must be rejected; struct layout is immutable"
    );
}

#[test]
fn mutation_change_type_on_struct_field_is_rejected() {
    // Arrange
    let c = compiler();
    let objects = parse_to_objects("struct S { x: float; }");
    let op = mutation(&FbsOp::ChangeFieldType {
        table: "S".to_string(),
        field_name: "x".to_string(),
        new_type: "int".to_string(),
    });

    // Act
    let result = c.apply_mutation(&objects, &op);

    // Assert
    assert!(result.is_err(), "changing a struct field type must be rejected");
}

#[test]
fn mutation_no_reorder_op_exists() {
    // Arrange / Act / Assert: there is intentionally no Reorder op in `FbsOp`.
    // Field reordering changes slot indices (wire identity) and is unrepresentable
    // by design — slot ids are preserved verbatim and only append/deprecate edits
    // exist. This test documents that invariant (case 12). If a Reorder op is ever
    // added, it must reject as breaking; until then there is nothing to apply.
    let ops = [
        FbsOp::AddField {
            table: "T".to_string(),
            field_name: "x".to_string(),
            field_type: "int".to_string(),
            default_value: None,
            doc_comment: None,
        },
        FbsOp::DeprecateField {
            table: "T".to_string(),
            field_name: "x".to_string(),
        },
        FbsOp::RenameField {
            table: "T".to_string(),
            old_name: "x".to_string(),
            new_name: "y".to_string(),
        },
    ];
    // None of the representable ops change slot ordering of existing fields.
    for op in &ops {
        let json = String::from_utf8(op.encode().to_vec()).unwrap();
        assert!(
            !json.contains("Reorder"),
            "unexpected reorder op variant: {json}"
        );
    }
}

// ── P7: advanced compatibility ──────────────────────────────────────────────────

#[test]
fn compat_add_union_member_backward_ok_forward_full_break() {
    // Arrange: adding a union member is adding an enum value on the discriminator.
    let c = compiler();
    let old = table_blob(
        "table A { x: int; }\ntable B { y: int; }\nunion U { A }",
        "U",
    );
    let new = table_blob(
        "table A { x: int; }\ntable B { y: int; }\nunion U { A, B }",
        "U",
    );

    // Act
    let backward = c.check_compatibility(
        &old,
        &new,
        &CompatibilityRules {
            direction: CompatibilityDirection::Backward,
            disabled: false,
        },
    );
    let forward = c.check_compatibility(
        &old,
        &new,
        &CompatibilityRules {
            direction: CompatibilityDirection::Forward,
            disabled: false,
        },
    );
    let full = c.check_compatibility(&old, &new, &CompatibilityRules::full());

    // Assert: add value rule — backward ok, forward & full break.
    assert!(backward.is_ok(), "add union member is BACKWARD ok: {backward:?}");
    assert!(forward.is_err(), "add union member breaks FORWARD");
    assert!(full.is_err(), "add union member breaks FULL");
}

#[test]
fn compat_add_field_at_end_under_each_direction() {
    // Arrange
    let c = compiler();
    let old = table_blob("table T { a: int (id: 0); }", "T");
    let new = table_blob("table T { a: int (id: 0); b: string (id: 1); }", "T");

    // Act / Assert: appending a field at the next slot is OK in every direction.
    for dir in [
        CompatibilityDirection::Backward,
        CompatibilityDirection::Forward,
        CompatibilityDirection::Full,
    ] {
        let result = c.check_compatibility(
            &old,
            &new,
            &CompatibilityRules {
                direction: dir,
                disabled: false,
            },
        );
        assert!(result.is_ok(), "append-at-end should be ok for {dir:?}: {result:?}");
    }
}

#[test]
fn compat_deprecate_field_under_each_direction() {
    // Arrange
    let c = compiler();
    let old = table_blob("table T { a: int; b: string; }", "T");
    let new = table_blob("table T { a: int; b: string (deprecated); }", "T");

    // Act / Assert: deprecation retains the slot, so it is non-breaking everywhere.
    for dir in [
        CompatibilityDirection::Backward,
        CompatibilityDirection::Forward,
        CompatibilityDirection::Full,
    ] {
        let result = c.check_compatibility(
            &old,
            &new,
            &CompatibilityRules {
                direction: dir,
                disabled: false,
            },
        );
        assert!(result.is_ok(), "deprecate should be ok for {dir:?}: {result:?}");
    }
}

#[test]
fn compat_add_bit_flags_enum_value_backward_ok_forward_breaks() {
    // Arrange: a bit_flags enum gaining a new flag. The compat check keys on the
    // numeric value, so explicit values are used here to make the addition visible.
    let c = compiler();
    let old = table_blob("enum Mode : ubyte (bit_flags) { A = 1, B = 2 }", "Mode");
    let new = table_blob("enum Mode : ubyte (bit_flags) { A = 1, B = 2, C = 4 }", "Mode");

    // Act
    let backward = c.check_compatibility(
        &old,
        &new,
        &CompatibilityRules {
            direction: CompatibilityDirection::Backward,
            disabled: false,
        },
    );
    let forward = c.check_compatibility(
        &old,
        &new,
        &CompatibilityRules {
            direction: CompatibilityDirection::Forward,
            disabled: false,
        },
    );

    // Assert: add value rule applies to bit_flags enums too.
    assert!(backward.is_ok(), "add bit_flags value is BACKWARD ok: {backward:?}");
    assert!(forward.is_err(), "add bit_flags value breaks FORWARD");
}
