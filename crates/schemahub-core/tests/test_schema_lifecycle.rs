mod helpers;
use helpers::*;
use schemahub_core::CoreError;

const PROTO_USER: &str = r#"
syntax = "proto3";
package payments;

message User {
  uint64 id    = 1;
  string email = 2;
}

message CreateUserRequest {
  string email = 1;
}
"#;

const PROTO_USER_V2: &str = r#"
syntax = "proto3";
package payments;

message User {
  uint64 id    = 1;
  string email = 2;
  string name  = 3;
}

message CreateUserRequest {
  string email = 1;
}
"#;

const PROTO_ORDER: &str = r#"
syntax = "proto3";
package payments;

message Order {
  uint64 id     = 1;
  uint64 user_id = 2;
}
"#;

// ── create_schema ─────────────────────────────────────────────────────────────

#[test]
fn create_schema_advances_branch_head() {
    let core = make_core_with_repo("acme", "schemas");
    let head_before = core.get_branch_head("acme", "schemas", "main").unwrap();

    let new_commit = core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head_after = core.get_branch_head("acme", "schemas", "main").unwrap();
    assert_ne!(head_before, head_after);
    assert_eq!(head_after.to_hex(), new_commit);
}

#[test]
fn create_schema_stores_declarations_in_tree() {
    let core = make_core_with_repo("acme", "schemas");
    let commit_hex = core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let schemas = core.list_schemas("acme", "schemas", &commit_hex).unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].0, "user.proto");

    let decls = core.list_declarations("acme", "schemas", "user.proto", &commit_hex).unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"User"), "missing User in {names:?}");
    assert!(names.contains(&"CreateUserRequest"), "missing CreateUserRequest in {names:?}");
}

#[test]
fn create_schema_duplicate_returns_already_exists() {
    let core = make_core_with_repo("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let err = core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap_err();
    assert!(matches!(err, CoreError::AlreadyExists(_)));
}

#[test]
fn create_schema_stale_base_revision_returns_conflict() {
    let core = make_core_with_repo("acme", "schemas");
    let initial_head = core.get_branch_head("acme", "schemas", "main").unwrap();

    // Advance the head.
    core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // Now try with the old (stale) base.
    let err = core.create_schema(
        "acme", "schemas", "main",
        "order.proto", PROTO_ORDER.as_bytes(), "protobuf",
        &initial_head.to_hex(), &idem(), "test", None,
    ).unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }));
}

#[test]
fn create_schema_idempotency_returns_same_commit() {
    let core = make_core_with_repo("acme", "schemas");
    let key = idem();

    let commit1 = core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &key, "test", None,
    ).unwrap();

    let commit2 = core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &key, "test", None,
    ).unwrap();

    assert_eq!(commit1, commit2);
    // Only one commit should have been created.
    let commits = core.list_commits("acme", "schemas", Some("main"), None, 10).unwrap();
    assert_eq!(commits.len(), 2); // initial + create
}

// ── update_schema ─────────────────────────────────────────────────────────────

#[test]
fn update_schema_adds_new_declaration() {
    let core = make_core_with_repo("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    let new_commit = core.update_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER_V2.as_bytes(),
        &head.to_hex(), &idem(), false, "test", None,
    ).unwrap();

    let decls = core.list_declarations("acme", "schemas", "user.proto", &new_commit).unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"User"), "User missing from {names:?}");
    assert!(names.contains(&"CreateUserRequest"), "CreateUserRequest missing");
}

#[test]
fn update_schema_not_found_returns_error() {
    let core = make_core_with_repo("acme", "schemas");
    let err = core.update_schema(
        "acme", "schemas", "main",
        "ghost.proto", PROTO_USER.as_bytes(),
        "", &idem(), false, "test", None,
    ).unwrap_err();
    // Should be NotFound (schema doesn't exist) or an error from the tree lookup.
    assert!(
        matches!(err, CoreError::NotFound(_)) || matches!(err, CoreError::InvalidArgument(_)),
        "unexpected error: {err:?}"
    );
}

#[test]
fn update_schema_history_shows_two_commits() {
    let core = make_core_with_repo("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    core.update_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER_V2.as_bytes(),
        &head.to_hex(), &idem(), false, "test", None,
    ).unwrap();

    // initial + create + update = 3
    let commits = core.list_commits("acme", "schemas", Some("main"), None, 10).unwrap();
    assert_eq!(commits.len(), 3);
    assert!(commits[0].1.message.contains("update"));
}

// ── delete_schema ─────────────────────────────────────────────────────────────

#[test]
fn delete_schema_removes_it_from_tree() {
    let core = make_core_with_repo("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    let after_delete = core.delete_schema(
        "acme", "schemas", "main",
        "user.proto",
        &head.to_hex(), &idem(), false, "test", None,
    ).unwrap();

    let schemas = core.list_schemas("acme", "schemas", &after_delete).unwrap();
    assert!(schemas.is_empty(), "expected empty schema list after delete");
}

#[test]
fn delete_schema_not_found_returns_error() {
    let core = make_core_with_repo("acme", "schemas");
    let err = core.delete_schema(
        "acme", "schemas", "main",
        "ghost.proto", "", &idem(), false, "test", None,
    ).unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)));
}

#[test]
fn delete_schema_idempotent_with_same_key() {
    let core = make_core_with_repo("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    let key = idem();

    let c1 = core.delete_schema(
        "acme", "schemas", "main",
        "user.proto", &head.to_hex(), &key, false, "test", None,
    ).unwrap();

    let c2 = core.delete_schema(
        "acme", "schemas", "main",
        "user.proto", &head.to_hex(), &key, false, "test", None,
    ).unwrap();

    assert_eq!(c1, c2);
}

// ── multi-schema repo ─────────────────────────────────────────────────────────

#[test]
fn multiple_schemas_coexist_in_same_repo() {
    let core = make_core_with_repo("acme", "schemas");

    core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    let final_commit = core.create_schema(
        "acme", "schemas", "main",
        "order.proto", PROTO_ORDER.as_bytes(), "protobuf",
        &head.to_hex(), &idem(), "test", None,
    ).unwrap();

    let schemas = core.list_schemas("acme", "schemas", &final_commit).unwrap();
    let names: Vec<&str> = schemas.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"user.proto"), "missing user.proto");
    assert!(names.contains(&"order.proto"), "missing order.proto");
}

// ── get_declaration ───────────────────────────────────────────────────────────

#[test]
fn get_declaration_returns_correct_blob() {
    let core = make_core_with_repo("acme", "schemas");
    let commit_hex = core.create_schema(
        "acme", "schemas", "main",
        "user.proto", PROTO_USER.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let detail = core.get_declaration(
        "acme", "schemas", "user.proto", "User", &commit_hex,
    ).unwrap();
    assert!(detail.is_some(), "expected User declaration to be found");

    let missing = core.get_declaration(
        "acme", "schemas", "user.proto", "NonExistent", &commit_hex,
    ).unwrap();
    assert!(missing.is_none());
}
