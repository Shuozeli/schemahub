/// Integration tests: compatibility enforcement on protected branches.
mod helpers;
use helpers::*;
use schemahub_core::{CoreError, RepoConfig};
use schemahub_types::CompatibilityDirection;

/// Helper: create a repo with Full compatibility direction.
fn make_core_with_full_compat(project: &str, repo: &str) -> schemahub_core::Core {
    let core = make_core();
    let mut config = RepoConfig::default();
    config.compatibility_direction = CompatibilityDirection::Full;
    core.initialize_repo(project, repo, &config).unwrap();
    core
}

// ── Protobuf compat via update_schema ─────────────────────────────────────────

const PROTO_V1: &str = r#"syntax = "proto3"; message Order { uint64 id = 1; string name = 2; }"#;

#[test]
fn proto_update_removes_field_blocked_under_full_compat() {
    // Removing a field breaks FORWARD and FULL compat.
    let core = make_core_with_full_compat("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();
    let v2 = r#"syntax = "proto3"; message Order { uint64 id = 1; }"#;
    let err = core.update_schema(
        "acme", "schemas", "main",
        "order.proto", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    ).unwrap_err();

    assert!(
        matches!(err, CoreError::CompatibilityViolation(_)),
        "removing a field should be rejected under Full compat: {err:?}"
    );
}

#[test]
fn proto_update_removes_field_allowed_under_backward_compat() {
    // Removing a field is BACKWARD compatible (new reader ignores unknown fields from old data).
    let core = make_core_with_repo("acme", "schemas"); // default = Backward
    core.create_schema(
        "acme", "schemas", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    let v2 = r#"syntax = "proto3"; message Order { uint64 id = 1; }"#;
    let result = core.update_schema(
        "acme", "schemas", "main",
        "order.proto", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    );
    assert!(result.is_ok(), "removing a field is ok under Backward compat: {result:?}");
}

#[test]
fn proto_update_removes_field_allowed_with_force() {
    let core = make_core_with_full_compat("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    let v2 = r#"syntax = "proto3"; message Order { uint64 id = 1; }"#;
    let result = core.update_schema(
        "acme", "schemas", "main",
        "order.proto", v2.as_bytes(),
        &head, &idem(), true, // force=true bypasses compat
        "test", None,
    );
    assert!(result.is_ok(), "force=true should bypass compat check: {result:?}");
}

#[test]
fn proto_update_add_field_ok_under_full_compat() {
    let core = make_core_with_full_compat("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Adding a field is fully compatible (Backward ✓, Forward ✓, Full ✓).
    let v2 = r#"syntax = "proto3"; message Order { uint64 id = 1; string name = 2; bool active = 3; }"#;
    let result = core.update_schema(
        "acme", "schemas", "main",
        "order.proto", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    );
    assert!(result.is_ok(), "adding field should be allowed under Full compat: {result:?}");
}

// ── FlatBuffers compat via update_schema ──────────────────────────────────────

const FBS_V1: &str = r#"
namespace Acme;
table Item { id:ulong; price:float; }
enum Status : byte { UNKNOWN = 0, ACTIVE = 1 }
root_type Item;
"#;

#[test]
fn fbs_type_change_blocked_under_full_compat() {
    let core = make_core_with_full_compat("acme", "game");
    core.create_schema(
        "acme", "game", "main",
        "item.fbs", FBS_V1.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "game", "main").unwrap().to_hex();

    // Changing a field's type is always breaking for FlatBuffers.
    let v2 = "namespace Acme; table Item { id:ulong; price:double; } enum Status : byte { UNKNOWN = 0, ACTIVE = 1 } root_type Item;";
    let err = core.update_schema(
        "acme", "game", "main",
        "item.fbs", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    ).unwrap_err();

    assert!(
        matches!(err, CoreError::CompatibilityViolation(_)),
        "type change should be rejected on protected branch: {err:?}"
    );
}

#[test]
fn fbs_type_change_blocked_under_backward_compat() {
    // Type changes are always breaking in FlatBuffers regardless of direction.
    let core = make_core_with_repo("acme", "game"); // default = Backward
    core.create_schema(
        "acme", "game", "main",
        "item.fbs", FBS_V1.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "game", "main").unwrap().to_hex();

    let v2 = "namespace Acme; table Item { id:ulong; price:double; } enum Status : byte { UNKNOWN = 0, ACTIVE = 1 } root_type Item;";
    let err = core.update_schema(
        "acme", "game", "main",
        "item.fbs", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    ).unwrap_err();

    assert!(
        matches!(err, CoreError::CompatibilityViolation(_)),
        "type change always breaks compat: {err:?}"
    );
}

#[test]
fn fbs_add_enum_value_blocked_under_full_compat() {
    // Adding an enum value breaks FORWARD and FULL compat.
    let core = make_core_with_full_compat("acme", "game");
    core.create_schema(
        "acme", "game", "main",
        "item.fbs", FBS_V1.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "game", "main").unwrap().to_hex();

    let v2 = "namespace Acme; table Item { id:ulong; price:float; } enum Status : byte { UNKNOWN = 0, ACTIVE = 1, DELETED = 2 } root_type Item;";
    let err = core.update_schema(
        "acme", "game", "main",
        "item.fbs", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    ).unwrap_err();

    assert!(
        matches!(err, CoreError::CompatibilityViolation(_)),
        "adding enum value should be rejected under Full compat: {err:?}"
    );
}

#[test]
fn fbs_add_enum_value_allowed_under_backward_compat() {
    // Adding an enum value is BACKWARD compatible.
    let core = make_core_with_repo("acme", "game"); // default = Backward
    core.create_schema(
        "acme", "game", "main",
        "item.fbs", FBS_V1.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "game", "main").unwrap().to_hex();

    let v2 = "namespace Acme; table Item { id:ulong; price:float; } enum Status : byte { UNKNOWN = 0, ACTIVE = 1, DELETED = 2 } root_type Item;";
    let result = core.update_schema(
        "acme", "game", "main",
        "item.fbs", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    );
    assert!(result.is_ok(), "adding enum value is ok under Backward compat: {result:?}");
}

#[test]
fn fbs_add_field_to_table_allowed_under_full_compat() {
    let core = make_core_with_full_compat("acme", "game");
    core.create_schema(
        "acme", "game", "main",
        "item.fbs", FBS_V1.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "game", "main").unwrap().to_hex();

    // Adding a field at end of table is always compatible in FlatBuffers.
    let v2 = "namespace Acme; table Item { id:ulong; price:float; name:string; } enum Status : byte { UNKNOWN = 0, ACTIVE = 1 } root_type Item;";
    let result = core.update_schema(
        "acme", "game", "main",
        "item.fbs", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    );
    assert!(result.is_ok(), "adding field at end should be allowed: {result:?}");
}

#[test]
fn compat_not_enforced_on_unprotected_branch() {
    let core = make_core_with_full_compat("acme", "game");
    let base = core.get_branch_head("acme", "game", "main").unwrap().to_hex();

    // Create an unprotected feature branch.
    core.create_branch("acme", "game", "feature/x", &base).unwrap();
    core.create_schema(
        "acme", "game", "feature/x",
        "item.fbs", FBS_V1.as_bytes(), "flatbuffers",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "game", "feature/x").unwrap().to_hex();

    // Type change — breaking, but feature branches are unprotected.
    let v2 = "namespace Acme; table Item { id:ulong; price:double; } enum Status : byte { UNKNOWN = 0, ACTIVE = 1 } root_type Item;";
    let result = core.update_schema(
        "acme", "game", "feature/x",
        "item.fbs", v2.as_bytes(),
        &head, &idem(), false, "test", None,
    );
    assert!(result.is_ok(), "compat not enforced on unprotected branch: {result:?}");
}
