mod helpers;
use helpers::*;
use schemahub_core::CoreError;

const PROTO_A: &str = r#"
syntax = "proto3";
message A { uint64 id = 1; }
"#;

const PROTO_B: &str = r#"
syntax = "proto3";
message B { string name = 1; }
"#;

// ── Branch CRUD ───────────────────────────────────────────────────────────────

#[test]
fn create_branch_from_main() {
    let core = make_core_with_repo("acme", "schemas");
    let main_head = core.get_branch_head("acme", "schemas", "main").unwrap();

    core.create_branch("acme", "schemas", "feature/new", &main_head.to_hex()).unwrap();

    let feature_head = core.get_branch_head("acme", "schemas", "feature/new").unwrap();
    assert_eq!(feature_head, main_head, "new branch should point to same commit as main");
}

#[test]
fn create_branch_invalid_hash_returns_error() {
    let core = make_core_with_repo("acme", "schemas");
    let err = core.create_branch("acme", "schemas", "bad", "not-a-hash").unwrap_err();
    assert!(matches!(err, CoreError::InvalidArgument(_)));
}

#[test]
fn create_branch_nonexistent_commit_returns_not_found() {
    let core = make_core_with_repo("acme", "schemas");
    // Valid hex but doesn't exist in storage.
    let fake_hex = "a".repeat(64);
    let err = core.create_branch("acme", "schemas", "ghost", &fake_hex).unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_) | CoreError::AlreadyExists(_) | CoreError::InvalidArgument(_)));
}

#[test]
fn delete_branch() {
    let core = make_core_with_repo("acme", "schemas");
    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    core.create_branch("acme", "schemas", "temp", &head.to_hex()).unwrap();

    core.delete_branch("acme", "schemas", "temp").unwrap();

    let err = core.get_branch_head("acme", "schemas", "temp").unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)));
}

#[test]
fn list_branches_with_and_without_prefix() {
    let core = make_core_with_repo("acme", "schemas");
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    core.create_branch("acme", "schemas", "feature/x", &head).unwrap();
    core.create_branch("acme", "schemas", "feature/y", &head).unwrap();
    core.create_branch("acme", "schemas", "hotfix/z", &head).unwrap();

    let all = core.list_branches("acme", "schemas", "").unwrap();
    assert!(all.len() >= 4, "expected at least 4 branches, got {}", all.len());

    let feature = core.list_branches("acme", "schemas", "feature/").unwrap();
    assert_eq!(feature.len(), 2);

    let hotfix = core.list_branches("acme", "schemas", "hotfix/").unwrap();
    assert_eq!(hotfix.len(), 1);
    assert_eq!(hotfix[0].0, "hotfix/z");
}

#[test]
fn branch_diverges_independently() {
    let core = make_core_with_repo("acme", "schemas");
    let base = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    core.create_branch("acme", "schemas", "feature/a", &base).unwrap();
    core.create_branch("acme", "schemas", "feature/b", &base).unwrap();

    // Advance feature/a.
    core.create_schema(
        "acme", "schemas", "feature/a",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // Advance feature/b differently.
    core.create_schema(
        "acme", "schemas", "feature/b",
        "b.proto", PROTO_B.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head_a = core.get_branch_head("acme", "schemas", "feature/a").unwrap();
    let head_b = core.get_branch_head("acme", "schemas", "feature/b").unwrap();
    assert_ne!(head_a, head_b, "branches should have diverged");

    let main_head = core.get_branch_head("acme", "schemas", "main").unwrap();
    assert_ne!(head_a, main_head, "feature/a should not have touched main");
    assert_ne!(head_b, main_head, "feature/b should not have touched main");
}

// ── Tag CRUD ──────────────────────────────────────────────────────────────────

#[test]
fn create_and_read_tag() {
    let core = make_core_with_repo("acme", "schemas");
    let head = core.get_branch_head("acme", "schemas", "main").unwrap();

    core.create_tag("acme", "schemas", "v1.0", &head.to_hex(), None).unwrap();

    let key = schemahub_storage::keys::tag_ref_key("acme", "schemas", "v1.0");
    let stored = core.storage.get_ref(&key).unwrap().unwrap();
    assert_eq!(stored, head);
}

#[test]
fn create_duplicate_tag_returns_already_exists() {
    let core = make_core_with_repo("acme", "schemas");
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    core.create_tag("acme", "schemas", "v1.0", &head, None).unwrap();
    let err = core.create_tag("acme", "schemas", "v1.0", &head, None).unwrap_err();
    assert!(matches!(err, CoreError::AlreadyExists(_)));
}

#[test]
fn delete_tag() {
    let core = make_core_with_repo("acme", "schemas");
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    core.create_tag("acme", "schemas", "v1.0", &head, None).unwrap();
    core.delete_tag("acme", "schemas", "v1.0").unwrap();

    let key = schemahub_storage::keys::tag_ref_key("acme", "schemas", "v1.0");
    let stored = core.storage.get_ref(&key).unwrap();
    assert!(stored.is_none());
}

#[test]
fn list_tags_with_prefix() {
    let core = make_core_with_repo("acme", "schemas");
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    core.create_tag("acme", "schemas", "v1.0", &head, None).unwrap();
    core.create_tag("acme", "schemas", "v1.1", &head, None).unwrap();
    core.create_tag("acme", "schemas", "v2.0", &head, None).unwrap();

    let all = core.list_tags("acme", "schemas", "").unwrap();
    assert_eq!(all.len(), 3);

    let v1 = core.list_tags("acme", "schemas", "v1").unwrap();
    assert_eq!(v1.len(), 2);

    let v2 = core.list_tags("acme", "schemas", "v2").unwrap();
    assert_eq!(v2.len(), 1);
    assert_eq!(v2[0].0, "v2.0");
}

#[test]
fn tag_points_to_commit_not_branch_tip() {
    let core = make_core_with_repo("acme", "schemas");

    // Tag the initial commit.
    let initial = core.get_branch_head("acme", "schemas", "main").unwrap();
    core.create_tag("acme", "schemas", "v0", &initial.to_hex(), None).unwrap();

    // Advance main.
    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // Tag should still point to the original commit.
    let key = schemahub_storage::keys::tag_ref_key("acme", "schemas", "v0");
    let tag_target = core.storage.get_ref(&key).unwrap().unwrap();
    assert_eq!(tag_target, initial, "tag should be immutable after branch advances");
}

// ── Commit history ────────────────────────────────────────────────────────────

#[test]
fn list_commits_walks_history_newest_first() {
    let core = make_core_with_repo("acme", "schemas");

    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    core.create_schema(
        "acme", "schemas", "main",
        "b.proto", PROTO_B.as_bytes(), "protobuf",
        &head.to_hex(), &idem(), "test", None,
    ).unwrap();

    let commits = core.list_commits("acme", "schemas", Some("main"), None, 10).unwrap();
    // initial + create a + create b = 3
    assert_eq!(commits.len(), 3);
    // Newest first: b, a, initial.
    assert!(commits[0].1.message.contains("b.proto"));
    assert!(commits[1].1.message.contains("a.proto"));
}

#[test]
fn list_commits_limit_is_respected() {
    let core = make_core_with_repo("acme", "schemas");

    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    core.create_schema(
        "acme", "schemas", "main",
        "b.proto", PROTO_B.as_bytes(), "protobuf",
        &head.to_hex(), &idem(), "test", None,
    ).unwrap();

    let limited = core.list_commits("acme", "schemas", Some("main"), None, 2).unwrap();
    assert_eq!(limited.len(), 2);
}

#[test]
fn get_commit_by_hex() {
    let core = make_core_with_repo("acme", "schemas");
    let commit_hex = core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let commit = core.get_commit("acme", "schemas", &commit_hex).unwrap();
    assert_eq!(commit.message, "create schema: a.proto");
    assert_eq!(commit.author, "test");
}

#[test]
fn get_commit_invalid_hex_returns_error() {
    let core = make_core_with_repo("acme", "schemas");
    let err = core.get_commit("acme", "schemas", "bad-hex").unwrap_err();
    assert!(matches!(err, CoreError::InvalidArgument(_)));
}
