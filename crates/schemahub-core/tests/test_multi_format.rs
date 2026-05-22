/// Integration tests for FlatBuffers and OpenAPI schema lifecycle via Core.
mod helpers;
use helpers::*;
use schemahub_types::DeclKind;

// ── FlatBuffers ───────────────────────────────────────────────────────────────

const FBS_MONSTER: &str = r#"
namespace MyGame.Sample;

table Monster {
  pos:Vec3;
  mana:short = 150;
  hp:short = 100;
  name:string;
  friendly:bool = false (deprecated);
  inventory:[ubyte];
  color:Color = Blue;
  weapons:[Weapon];
  equipped:Equipment;
  path:[Vec3];
}

table Vec3 {
  x:float;
  y:float;
  z:float;
}

table Weapon {
  name:string;
  damage:short;
}

enum Color:byte { Red = 0, Green, Blue = 2 }

union Equipment { Weapon }

root_type Monster;
"#;

#[test]
fn fbs_create_and_list_declarations() {
    let core = make_core_with_repo("acme", "game");

    let commit_hex = core.create_schema(
        "acme", "game", "main",
        "monster.fbs", FBS_MONSTER.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();

    let decls = core.list_declarations("acme", "game", "monster.fbs", &commit_hex).unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"Monster"), "missing Monster: {names:?}");
    assert!(names.contains(&"Vec3"), "missing Vec3: {names:?}");
    assert!(names.contains(&"Weapon"), "missing Weapon: {names:?}");
    assert!(names.contains(&"Color"), "missing Color: {names:?}");
    assert!(names.contains(&"Equipment"), "missing Equipment: {names:?}");
}

#[test]
fn fbs_decl_kinds_are_correct() {
    let core = make_core_with_repo("acme", "game");
    let commit_hex = core.create_schema(
        "acme", "game", "main",
        "monster.fbs", FBS_MONSTER.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();

    let decls = core.list_declarations("acme", "game", "monster.fbs", &commit_hex).unwrap();
    let by_name: std::collections::HashMap<_, _> = decls.iter().map(|d| (d.name.as_str(), d.kind)).collect();

    assert_eq!(by_name["Monster"], DeclKind::Table);
    assert_eq!(by_name["Vec3"], DeclKind::Table);
    assert_eq!(by_name["Weapon"], DeclKind::Table);
    assert_eq!(by_name["Color"], DeclKind::FbsEnum);
    assert_eq!(by_name["Equipment"], DeclKind::Union);
}

#[test]
fn fbs_get_declaration_returns_blob() {
    let core = make_core_with_repo("acme", "game");
    let commit_hex = core.create_schema(
        "acme", "game", "main",
        "monster.fbs", FBS_MONSTER.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();

    let detail = core.get_declaration("acme", "game", "monster.fbs", "Monster", &commit_hex).unwrap();
    assert!(detail.is_some(), "Monster declaration should be found");

    let missing = core.get_declaration("acme", "game", "monster.fbs", "Dragon", &commit_hex).unwrap();
    assert!(missing.is_none());
}

#[test]
fn fbs_update_schema_replaces_blobs() {
    let core = make_core_with_repo("acme", "game");
    core.create_schema(
        "acme", "game", "main",
        "monster.fbs", FBS_MONSTER.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();

    // Updated version removes Equipment union, adds a new table.
    let fbs_v2 = r#"
namespace MyGame.Sample;
table Monster { name:string; hp:short = 100; }
table Dragon  { name:string; firepower:ushort; }
enum Color:byte { Red = 0, Green, Blue = 2 }
root_type Monster;
"#;

    let head = core.get_branch_head("acme", "game", "main").unwrap().to_hex();
    let v2_commit = core.update_schema(
        "acme", "game", "main",
        "monster.fbs", fbs_v2.as_bytes(),
        &head, &idem(), true, "test", None, // force=true to skip compat on protected branch
    ).unwrap();

    let decls = core.list_declarations("acme", "game", "monster.fbs", &v2_commit).unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"Monster"), "{names:?}");
    assert!(names.contains(&"Dragon"), "{names:?}");
    assert!(names.contains(&"Color"), "{names:?}");
    assert!(!names.contains(&"Equipment"), "Equipment should be gone: {names:?}");
}

// ── OpenAPI ───────────────────────────────────────────────────────────────────

const OPENAPI_PETSTORE: &str = r#"
openapi: "3.0.3"
info:
  title: Petstore
  version: "1.0"
paths:
  /pets:
    get:
      operationId: listPets
      summary: List all pets
      responses:
        "200":
          description: A list of pets
  /pets/{petId}:
    get:
      operationId: showPetById
      summary: Info for a specific pet
      responses:
        "200":
          description: Expected response to a valid request
components:
  schemas:
    Pet:
      type: object
      required: [id, name]
      properties:
        id:
          type: integer
        name:
          type: string
    Error:
      type: object
      required: [code, message]
      properties:
        code:
          type: integer
        message:
          type: string
"#;

#[test]
fn openapi_create_and_list_declarations() {
    let core = make_core_with_repo("acme", "api");
    let commit_hex = core.create_schema(
        "acme", "api", "main",
        "petstore.yaml", OPENAPI_PETSTORE.as_bytes(), "openapi",
        "", &idem(), "test", None,
    ).unwrap();

    let decls = core.list_declarations("acme", "api", "petstore.yaml", &commit_hex).unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();

    assert!(names.iter().any(|n| n.contains("/pets")), "missing /pets path: {names:?}");
    assert!(names.iter().any(|n| n.contains("schema:Pet")), "missing Pet schema: {names:?}");
    assert!(names.iter().any(|n| n.contains("schema:Error")), "missing Error schema: {names:?}");
}

#[test]
fn openapi_decl_kinds_are_correct() {
    let core = make_core_with_repo("acme", "api");
    let commit_hex = core.create_schema(
        "acme", "api", "main",
        "petstore.yaml", OPENAPI_PETSTORE.as_bytes(), "openapi",
        "", &idem(), "test", None,
    ).unwrap();

    let decls = core.list_declarations("acme", "api", "petstore.yaml", &commit_hex).unwrap();
    let kinds: std::collections::HashMap<_, _> = decls.iter().map(|d| (d.name.as_str(), d.kind)).collect();

    for (name, kind) in &kinds {
        if name.starts_with("path:") {
            assert_eq!(*kind, DeclKind::PathItem, "{name} should be PathItem");
        } else if name.starts_with("schema:") {
            assert_eq!(*kind, DeclKind::ComponentSchema, "{name} should be ComponentSchema");
        }
    }
}

#[test]
fn openapi_update_replaces_document() {
    let core = make_core_with_repo("acme", "api");
    core.create_schema(
        "acme", "api", "main",
        "petstore.yaml", OPENAPI_PETSTORE.as_bytes(), "openapi",
        "", &idem(), "test", None,
    ).unwrap();

    let v2 = r#"
openapi: "3.0.3"
info:
  title: Petstore v2
  version: "2.0"
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: ok
  /owners:
    get:
      operationId: listOwners
      responses:
        "200":
          description: ok
"#;

    let head = core.get_branch_head("acme", "api", "main").unwrap().to_hex();
    let v2_commit = core.update_schema(
        "acme", "api", "main",
        "petstore.yaml", v2.as_bytes(),
        &head, &idem(), true, "test", None,
    ).unwrap();

    let decls = core.list_declarations("acme", "api", "petstore.yaml", &v2_commit).unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();

    assert!(names.iter().any(|n| n.contains("/owners")), "new /owners path missing: {names:?}");
    assert!(!names.iter().any(|n| n.contains("schema:Pet")), "Pet schema should be gone: {names:?}");
}

// ── Mixed-format repo ─────────────────────────────────────────────────────────

#[test]
fn repo_can_hold_multiple_formats() {
    let core = make_core_with_repo("acme", "mixed");

    core.create_schema(
        "acme", "mixed", "main",
        "user.proto", b"syntax = \"proto3\"; message User { uint64 id = 1; }", "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "mixed", "main").unwrap().to_hex();
    core.create_schema(
        "acme", "mixed", "main",
        "events.fbs", b"table Event { id:ulong; name:string; }", "flatbuffers",
        &head, &idem(), "test", None,
    ).unwrap();

    let head2 = core.get_branch_head("acme", "mixed", "main").unwrap().to_hex();
    core.create_schema(
        "acme", "mixed", "main",
        "api.yaml", OPENAPI_PETSTORE.as_bytes(), "openapi",
        &head2, &idem(), "test", None,
    ).unwrap();

    let final_commit = core.get_branch_head("acme", "mixed", "main").unwrap().to_hex();
    let schemas = core.list_schemas("acme", "mixed", &final_commit).unwrap();
    let names: Vec<&str> = schemas.iter().map(|(n, _)| n.as_str()).collect();

    assert!(names.contains(&"user.proto"), "{names:?}");
    assert!(names.contains(&"events.fbs"), "{names:?}");
    assert!(names.contains(&"api.yaml"), "{names:?}");
}

#[test]
fn delete_one_schema_leaves_others_intact() {
    let core = make_core_with_repo("acme", "mixed");

    core.create_schema(
        "acme", "mixed", "main",
        "user.proto", b"syntax = \"proto3\"; message User { uint64 id = 1; }", "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "mixed", "main").unwrap().to_hex();
    core.create_schema(
        "acme", "mixed", "main",
        "events.fbs", b"table Event { id:ulong; name:string; }", "flatbuffers",
        &head, &idem(), "test", None,
    ).unwrap();

    let head2 = core.get_branch_head("acme", "mixed", "main").unwrap().to_hex();
    let after_delete = core.delete_schema(
        "acme", "mixed", "main",
        "user.proto", &head2, &idem(), false, "test", None,
    ).unwrap();

    let schemas = core.list_schemas("acme", "mixed", &after_delete).unwrap();
    let names: Vec<&str> = schemas.iter().map(|(n, _)| n.as_str()).collect();
    assert!(!names.contains(&"user.proto"), "user.proto should be deleted");
    assert!(names.contains(&"events.fbs"), "events.fbs should remain");
}
