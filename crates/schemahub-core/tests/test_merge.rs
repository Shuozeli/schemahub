mod helpers;
use helpers::*;
use schemahub_core::CoreError;

const PROTO_A: &str = r#"syntax = "proto3"; message A { uint64 id = 1; }"#;
const PROTO_B: &str = r#"syntax = "proto3"; message B { string name = 1; }"#;

// ── Fast-forward merge ────────────────────────────────────────────────────────

#[test]
fn merge_fast_forward_advances_target() {
    let core = make_core_with_repo("acme", "schemas");
    let base = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Create feature branch and add a schema on it.
    core.create_branch("acme", "schemas", "feature/x", &base).unwrap();
    core.create_schema(
        "acme", "schemas", "feature/x",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let feature_head = core.get_branch_head("acme", "schemas", "feature/x").unwrap();

    // main is still at base — this is a clean fast-forward.
    let main_head_before = core.get_branch_head("acme", "schemas", "main").unwrap();
    assert_eq!(main_head_before.to_hex(), base);

    let new_main = core.merge_branches(
        "acme", "schemas",
        "feature/x", "main",
        &base, &idem(), "test", None,
    ).unwrap();

    let main_head_after = core.get_branch_head("acme", "schemas", "main").unwrap();
    assert_eq!(main_head_after, feature_head);
    assert_eq!(new_main, feature_head.to_hex());
}

#[test]
fn merge_when_already_equal_is_noop() {
    let core = make_core_with_repo("acme", "schemas");
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Merge main into itself.
    let result = core.merge_branches(
        "acme", "schemas",
        "main", "main",
        &head, &idem(), "test", None,
    ).unwrap();

    assert_eq!(result, head);
    let still_head = core.get_branch_head("acme", "schemas", "main").unwrap();
    assert_eq!(still_head.to_hex(), head);
}

#[test]
fn merge_idempotent_with_same_key() {
    let core = make_core_with_repo("acme", "schemas");
    let base = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    core.create_branch("acme", "schemas", "feature/x", &base).unwrap();
    core.create_schema(
        "acme", "schemas", "feature/x",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let key = idem();
    let r1 = core.merge_branches(
        "acme", "schemas",
        "feature/x", "main",
        &base, &key, "test", None,
    ).unwrap();
    let r2 = core.merge_branches(
        "acme", "schemas",
        "feature/x", "main",
        &base, &key, "test", None,
    ).unwrap();
    assert_eq!(r1, r2);
}

#[test]
fn merge_non_fast_forward_returns_conflict() {
    let core = make_core_with_repo("acme", "schemas");
    let base = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Create two branches from base, each advancing independently.
    core.create_branch("acme", "schemas", "feature/x", &base).unwrap();
    core.create_branch("acme", "schemas", "feature/y", &base).unwrap();

    core.create_schema(
        "acme", "schemas", "feature/x",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    core.create_schema(
        "acme", "schemas", "feature/y",
        "b.proto", PROTO_B.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // Fast-forward main to feature/x.
    core.merge_branches(
        "acme", "schemas",
        "feature/x", "main",
        &base, &idem(), "test", None,
    ).unwrap();

    let main_after_x = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Now attempt to merge feature/y — feature/y diverged from base,
    // not from the current main, so it's NOT a fast-forward.
    let err = core.merge_branches(
        "acme", "schemas",
        "feature/y", "main",
        &main_after_x, &idem(), "test", None,
    ).unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }), "expected Conflict, got {err:?}");
}

#[test]
fn merge_stale_base_revision_returns_conflict() {
    let core = make_core_with_repo("acme", "schemas");
    let stale_base = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Advance main so `stale_base` is now outdated.
    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // Create a feature branch from the NEW main and try to merge using stale base.
    let new_main = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();
    core.create_branch("acme", "schemas", "feature/z", &new_main).unwrap();

    let err = core.merge_branches(
        "acme", "schemas",
        "feature/z", "main",
        &stale_base, // stale — actual main has moved past this
        &idem(), "test", None,
    ).unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }));
}

#[test]
fn merge_preserves_schema_content() {
    let core = make_core_with_repo("acme", "schemas");
    let base = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    core.create_branch("acme", "schemas", "feat", &base).unwrap();
    core.create_schema(
        "acme", "schemas", "feat",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let merged_hex = core.merge_branches(
        "acme", "schemas",
        "feat", "main",
        &base, &idem(), "test", None,
    ).unwrap();

    // After merge, main should have the schema.
    let schemas = core.list_schemas("acme", "schemas", &merged_hex).unwrap();
    let names: Vec<&str> = schemas.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"a.proto"), "a.proto missing after merge: {names:?}");
}
