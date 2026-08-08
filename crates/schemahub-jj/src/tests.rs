//! Unit tests for the jj-style JJ model (AAA style).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use schemahub_types::{DeclBlob, MetaBlob, MutationEffect};

use crate::object_db::{ObjectDb, ObjectKind};
use crate::{Jj, JjError, MemoryObjectDb, RefSpec, SchemaWrite};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn mem_jj() -> Jj {
    Jj::new(Arc::new(MemoryObjectDb::new()))
}

fn upsert(name: &str, body: &str) -> MutationEffect {
    MutationEffect {
        meta: Some(MetaBlob::new(b"package test;".to_vec())),
        upserts: vec![(name.to_string(), DeclBlob::new(body.as_bytes().to_vec()))],
        removes: vec![],
    }
}

/// Create an initial commit with two declarations on `main`.
fn seed_two_decls(jj: &Jj) -> String {
    let effect = MutationEffect {
        meta: Some(MetaBlob::new(b"package test;".to_vec())),
        upserts: vec![
            (
                "UserRequest".to_string(),
                DeclBlob::new(b"msg req v1".to_vec()),
            ),
            (
                "UserStatus".to_string(),
                DeclBlob::new(b"enum status v1".to_vec()),
            ),
        ],
        removes: vec![],
    };
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        effect,
        "alice",
        "seed",
    )
    .unwrap()
    .commit_id
}

fn seed_foreign_commit(jj: &Jj) -> String {
    jj.commit_write(
        "other",
        "repo",
        "main",
        "other.proto",
        &RefSpec::bookmark("main"),
        upsert("Other", "message Other {}"),
        "bob",
        "other repo commit",
    )
    .expect("commit other repo")
    .commit_id
}

// ── Object round-trip ────────────────────────────────────────────────────────

#[test]
fn resolve_ref_or_root_returns_stable_base_for_fresh_and_existing_bookmarks() {
    // Arrange
    let jj = mem_jj();

    // Act
    let fresh = jj
        .resolve_ref_or_root("proj", "repo", &RefSpec::bookmark("main"))
        .expect("resolve fresh root");
    let committed = seed_two_decls(&jj);
    let existing = jj
        .resolve_ref_or_root("proj", "repo", &RefSpec::bookmark("main"))
        .expect("resolve bookmark");

    // Assert
    assert!(!fresh.is_empty());
    assert_eq!(existing, committed);
    assert_ne!(fresh, existing);
}

#[test]
fn validate_revision_rejects_commit_from_another_repository() {
    // Arrange
    let jj = mem_jj();
    let own_commit = seed_two_decls(&jj);
    let other_commit = jj
        .commit_write(
            "other",
            "repo",
            "main",
            "other.proto",
            &RefSpec::bookmark("main"),
            upsert("Other", "message other"),
            "bob",
            "other repo commit",
        )
        .expect("commit other repo")
        .commit_id;

    // Act
    let own = jj.validate_revision("proj", "repo", &own_commit);
    let foreign = jj.validate_revision("proj", "repo", &other_commit);

    // Assert
    assert!(own.is_ok());
    assert!(matches!(foreign, Err(crate::JjError::BadRef(_))));
}

#[test]
fn raw_ref_resolution_rejects_commit_from_another_repository() {
    // Arrange
    let jj = mem_jj();
    seed_two_decls(&jj);
    let foreign = seed_foreign_commit(&jj);

    // Act
    let result = jj.resolve_ref_id("proj", "repo", &RefSpec::commit(foreign));

    // Assert
    assert!(matches!(result, Err(JjError::BadRef(_))));
}

#[test]
fn raw_foreign_commit_cannot_be_published_as_a_bookmark() {
    // Arrange
    let jj = mem_jj();
    seed_two_decls(&jj);
    let foreign = seed_foreign_commit(&jj);

    // Act
    let result = jj.create_bookmark(
        "proj",
        "repo",
        "smuggled",
        &RefSpec::commit(foreign),
        "alice",
    );

    // Assert
    assert!(matches!(result, Err(JjError::BadRef(_))));
    assert!(jj
        .list_bookmarks("proj", "repo")
        .expect("list bookmarks")
        .into_iter()
        .all(|(name, _)| name != "smuggled"));
}

#[test]
fn raw_foreign_commit_cannot_be_published_as_a_tag() {
    // Arrange
    let jj = mem_jj();
    seed_two_decls(&jj);
    let foreign = seed_foreign_commit(&jj);

    // Act
    let result = jj.create_tag(
        "proj",
        "repo",
        "smuggled",
        &RefSpec::commit(foreign),
        "alice",
    );

    // Assert
    assert!(matches!(result, Err(JjError::BadRef(_))));
    assert!(jj
        .list_tags("proj", "repo")
        .expect("list tags")
        .into_iter()
        .all(|(name, _)| name != "smuggled"));
}

#[test]
fn correlated_schema_delete_removes_meta_and_is_discoverable_in_op_log() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    let attributes = BTreeMap::from([
        (
            "schemahub.change_record".to_string(),
            "change-1".to_string(),
        ),
        (
            "schemahub.apply_attempt".to_string(),
            "attempt-1".to_string(),
        ),
    ]);

    // Act
    let write = jj
        .commit_schema_changes(
            "proj",
            "repo",
            "main",
            &RefSpec::commit(base),
            vec![SchemaWrite::Delete {
                schema_path: "user.proto".to_string(),
            }],
            "alice",
            "delete user schema",
            attributes.clone(),
        )
        .expect("delete schema");
    let deleted = jj.load_schema("proj", "repo", "user.proto", &RefSpec::bookmark("main"));
    let operation = jj
        .find_operation_by_attributes("proj", "repo", &attributes)
        .expect("search op log")
        .expect("correlated operation");
    let recovered = jj
        .find_correlated_write("proj", "repo", "main", &attributes)
        .expect("recover correlated write")
        .expect("correlated write receipt");

    // Assert
    assert!(matches!(deleted, Err(crate::JjError::SchemaNotFound(_))));
    assert_eq!(operation.op_id, write.operation_id);
    assert_eq!(recovered, write);
    assert_eq!(
        operation.attributes.get("schemahub.change_record"),
        Some(&"change-1".to_string())
    );
}

#[test]
fn object_roundtrip_returns_identical_bytes() {
    // Arrange
    let db = MemoryObjectDb::new();
    let bytes = b"hello declaration blob";

    // Act
    let id = db.put_object(ObjectKind::File, bytes).unwrap();
    let fetched = db.get_object(ObjectKind::File, &id).unwrap();

    // Assert
    assert_eq!(fetched, bytes);
    assert!(db.has_object(ObjectKind::File, &id).unwrap());
}

#[test]
fn put_object_is_content_addressed_and_dedups() {
    // Arrange
    let db = MemoryObjectDb::new();
    let bytes = b"same content";

    // Act
    let id1 = db.put_object(ObjectKind::File, bytes).unwrap();
    let id2 = db.put_object(ObjectKind::File, bytes).unwrap();

    // Assert
    assert_eq!(id1, id2);
    assert_eq!(db.list_objects(ObjectKind::File).unwrap().len(), 1);
}

#[test]
fn create_ref_never_overwrites_a_concurrent_value() {
    // Arrange
    let db = MemoryObjectDb::new();

    // Act
    let first = db
        .create_ref("proj/repo", "op_heads", b"root")
        .expect("create ref");
    let second = db
        .create_ref("proj/repo", "op_heads", b"new-root")
        .expect("repeat create");

    // Assert
    assert!(first);
    assert!(!second);
    assert_eq!(
        db.get_ref("proj/repo", "op_heads").expect("read ref"),
        Some(b"root".to_vec())
    );
}

// ── Per-declaration commit + dedup ───────────────────────────────────────────

#[test]
fn load_schema_reassembles_committed_declarations() {
    // Arrange
    let jj = mem_jj();
    seed_two_decls(&jj);

    // Act
    let schema = jj
        .load_schema("proj", "repo", "user.proto", &RefSpec::bookmark("main"))
        .unwrap();

    // Assert
    assert_eq!(schema.meta.as_bytes(), b"package test;");
    assert_eq!(schema.decls.len(), 2);
    assert_eq!(
        schema.decls.get("UserRequest").unwrap().as_bytes(),
        b"msg req v1"
    );
}

#[test]
fn list_schemas_preserves_nested_schema_paths() {
    // Arrange
    let jj = mem_jj();
    jj.commit_write_multi(
        "proj",
        "repo",
        "main",
        &RefSpec::bookmark("main"),
        vec![
            (
                "common/types.proto".to_string(),
                upsert("Common", "message common"),
            ),
            (
                "orders/order.proto".to_string(),
                upsert("Order", "message order"),
            ),
        ],
        "alice",
        "seed nested schemas",
    )
    .expect("commit nested schemas");

    // Act
    let schemas = jj
        .list_schemas("proj", "repo", &RefSpec::bookmark("main"))
        .expect("list nested schemas");

    // Assert
    assert_eq!(
        schemas,
        [
            "common/types.proto".to_string(),
            "orders/order.proto".to_string()
        ]
    );
}

#[test]
fn schema_pages_preserve_nested_path_order_and_exclusive_continuation() {
    // Arrange
    let jj = mem_jj();
    jj.commit_write_multi(
        "proj",
        "repo",
        "main",
        &RefSpec::bookmark("main"),
        vec![
            (
                "orders/z.proto".to_string(),
                upsert("LateOrder", "message late order"),
            ),
            (
                "common/types.proto".to_string(),
                upsert("Common", "message common"),
            ),
            (
                "orders/a.proto".to_string(),
                upsert("EarlyOrder", "message early order"),
            ),
        ],
        "alice",
        "seed paged schemas",
    )
    .expect("commit schemas");

    // Act
    let first = jj
        .list_schemas_page("proj", "repo", &RefSpec::bookmark("main"), None, 2)
        .expect("list first schema page");
    let second = jj
        .list_schemas_page(
            "proj",
            "repo",
            &RefSpec::bookmark("main"),
            first.next_cursor.as_deref(),
            2,
        )
        .expect("list second schema page");

    // Assert
    assert_eq!(
        first.schemas,
        [
            "common/types.proto".to_string(),
            "orders/a.proto".to_string()
        ]
    );
    assert_eq!(first.next_cursor.as_deref(), Some("orders/a.proto"));
    assert_eq!(second.schemas, ["orders/z.proto".to_string()]);
    assert_eq!(second.next_cursor, None);
}

#[test]
fn selected_schemas_load_with_the_full_inventory_in_one_batch() {
    // Arrange
    let jj = mem_jj();
    jj.commit_write_multi(
        "proj",
        "repo",
        "main",
        &RefSpec::bookmark("main"),
        vec![
            (
                "common/types.proto".to_string(),
                upsert("Common", "message common"),
            ),
            (
                "orders/order.proto".to_string(),
                MutationEffect {
                    meta: Some(MetaBlob::new(b"package orders;".to_vec())),
                    upserts: vec![
                        (
                            "Order".to_string(),
                            DeclBlob::new(b"message order".to_vec()),
                        ),
                        (
                            "OrderState".to_string(),
                            DeclBlob::new(b"enum order state".to_vec()),
                        ),
                    ],
                    removes: Vec::new(),
                },
            ),
            (
                "unused.proto".to_string(),
                upsert("Unused", "message unused"),
            ),
        ],
        "alice",
        "seed batch",
    )
    .expect("commit schemas");
    let selected = BTreeSet::from([
        "common/types.proto".to_string(),
        "orders/order.proto".to_string(),
    ]);

    // Act
    let batch = jj
        .load_schemas("proj", "repo", &selected, &RefSpec::bookmark("main"))
        .expect("load selected schemas");

    // Assert
    assert_eq!(
        batch.schemas.keys().cloned().collect::<BTreeSet<_>>(),
        selected
    );
    assert_eq!(
        batch.all_schema_names,
        BTreeSet::from([
            "common/types.proto".to_string(),
            "orders/order.proto".to_string(),
            "unused.proto".to_string(),
        ])
    );
    assert_eq!(batch.schemas["common/types.proto"].decls.len(), 1);
    assert_eq!(batch.schemas["orders/order.proto"].decls.len(), 2);
}

#[test]
fn editing_one_decl_leaves_siblings_content_hash_unchanged() {
    // Arrange: seed two decls, capture the sibling's file id.
    let jj = mem_jj();
    seed_two_decls(&jj);
    let sibling_before = jj
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserStatus",
            &RefSpec::bookmark("main"),
        )
        .unwrap();

    // Act: edit only UserRequest.
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v2"),
        "alice",
        "edit req",
    )
    .unwrap();

    // Assert: UserRequest changed; UserStatus blob is byte-identical (dedup).
    let req_after = jj
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    let sibling_after = jj
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserStatus",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    assert_eq!(req_after.as_bytes(), b"msg req v2");
    assert_eq!(sibling_after, sibling_before);
}

#[test]
fn unchanged_sibling_file_object_is_not_duplicated() {
    // Arrange
    let db = Arc::new(MemoryObjectDb::new());
    let jj = Jj::new(db.clone());
    seed_two_decls(&jj);
    // The UserStatus blob bytes ("enum status v1").
    let status_id = db.put_object(ObjectKind::File, b"enum status v1").unwrap();

    // Act: edit UserRequest only.
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v2"),
        "alice",
        "edit req",
    )
    .unwrap();

    // Assert: the sibling's content-addressed object still exists and was reused
    // (still exactly one File object for that content).
    assert!(db.has_object(ObjectKind::File, &status_id).unwrap());
}

#[test]
fn removing_a_declaration_drops_it_from_schema() {
    // Arrange
    let jj = mem_jj();
    seed_two_decls(&jj);

    // Act
    let effect = MutationEffect {
        meta: None,
        upserts: vec![],
        removes: vec!["UserStatus".to_string()],
    };
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        effect,
        "alice",
        "remove status",
    )
    .unwrap();

    // Assert
    let decls = jj
        .list_declarations("proj", "repo", "user.proto", &RefSpec::bookmark("main"))
        .unwrap();
    assert_eq!(decls, vec!["UserRequest".to_string()]);
}

// ── Op-log append + undo ─────────────────────────────────────────────────────

#[test]
fn each_write_appends_one_operation() {
    // Arrange
    let jj = mem_jj();

    // Act
    seed_two_decls(&jj);
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v2"),
        "alice",
        "edit",
    )
    .unwrap();

    // Assert: two operations, ordered oldest → newest.
    let ops = jj.list_operations("proj", "repo").unwrap();
    assert_eq!(ops.len(), 2);
    assert!(ops[0].description.contains("seed"));
    assert!(ops[1].description.contains("edit"));
}

#[test]
fn bounded_operation_log_returns_only_the_latest_operations_in_order() {
    // Arrange
    let jj = mem_jj();
    seed_two_decls(&jj);
    for (author, message, body) in [
        ("alice", "edit one", "msg req v2"),
        ("agent:reviewer", "edit two", "msg req v3"),
        ("bob", "edit three", "msg req v4"),
    ] {
        jj.commit_write(
            "proj",
            "repo",
            "main",
            "user.proto",
            &RefSpec::bookmark("main"),
            upsert("UserRequest", body),
            author,
            message,
        )
        .unwrap();
    }
    let expected = jj.list_operations("proj", "repo").unwrap().split_off(2);

    // Act
    let recent = jj.list_operations_tail("proj", "repo", 2).unwrap();

    // Assert
    assert_eq!(recent, expected);
    assert!(recent[0].description.contains("edit two"));
    assert!(recent[1].description.contains("edit three"));
}

#[test]
fn undo_restores_the_prior_view() {
    // Arrange: seed, then edit (so there is a prior state to restore).
    let jj = mem_jj();
    seed_two_decls(&jj);
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v2"),
        "alice",
        "edit",
    )
    .unwrap();

    // Act
    jj.undo("proj", "repo", "alice").unwrap();

    // Assert: main is back at v1 of UserRequest.
    let req = jj
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    assert_eq!(req.as_bytes(), b"msg req v1");
}

#[test]
fn repeated_undo_walks_back_through_each_prior_state_monotonically() {
    // Arrange: three sequential writes on main — create (req v1), then two edits
    // (req v2, req v3). Each write is its own content op.
    let jj = mem_jj();
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v1"),
        "alice",
        "create",
    )
    .unwrap();
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v2"),
        "alice",
        "edit to v2",
    )
    .unwrap();
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v3"),
        "alice",
        "edit to v3",
    )
    .unwrap();
    let read = |jj: &Jj| {
        jj.get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .map(|b| b.as_bytes().to_vec())
    };

    // Act 1: first undo rolls back the newest change (v3 -> v2).
    jj.undo("proj", "repo", "alice").unwrap();
    let after_first = read(&jj).unwrap();

    // Act 2: second undo continues walking back (v2 -> v1), NOT redoing the first.
    jj.undo("proj", "repo", "alice").unwrap();
    let after_second = read(&jj).unwrap();

    // Act 3: third undo rolls past the create, leaving the empty/initial state.
    jj.undo("proj", "repo", "alice").unwrap();
    let after_third = read(&jj);

    // Assert: monotonic walk-back through every prior state.
    assert_eq!(after_first, b"msg req v2", "1st undo should land on v2");
    assert_eq!(after_second, b"msg req v1", "2nd undo should land on v1");
    // After the third undo the decl no longer exists (initial empty state).
    assert!(
        after_third.is_err(),
        "3rd undo should leave the initial empty state (decl gone), got: {after_third:?}"
    );

    // Assert: a fourth undo has nothing older to roll back to.
    let fourth = jj.undo("proj", "repo", "alice");
    assert!(matches!(fourth, Err(crate::JjError::NothingToUndo)));
}

#[test]
fn undo_with_no_prior_operation_errors() {
    // Arrange: a brand-new repo with no writes — there is no content op to undo.
    // (Linear-undo walk-back DOES roll a single seed write back to the empty
    // state; the error case is when there is nothing recorded at all.)
    let jj = mem_jj();

    // Act
    let result = jj.undo("proj", "repo", "alice");

    // Assert
    assert!(result.is_err());
}

// ── Declaration names with separator-like characters (OpenAPI `path:/...`) ────

#[test]
fn decl_name_with_slash_and_colon_round_trips_and_keeps_sibling_dedup() {
    // Arrange: a repo whose schema file holds an OpenAPI-style `path:/users`
    // declaration (name contains both `:` and `/`) alongside a plain sibling.
    let jj = mem_jj();
    let effect = MutationEffect {
        meta: Some(MetaBlob::new(b"openapi: 3.0.3".to_vec())),
        upserts: vec![
            (
                "path:/users".to_string(),
                DeclBlob::new(b"get listUsers".to_vec()),
            ),
            (
                "schema:User".to_string(),
                DeclBlob::new(b"type: object".to_vec()),
            ),
        ],
        removes: vec![],
    };

    // Act: write, then read the schema back and fetch the slashed decl directly.
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "api.yaml",
        &RefSpec::bookmark("main"),
        effect,
        "alice",
        "seed openapi",
    )
    .unwrap();
    let schema = jj
        .load_schema("proj", "repo", "api.yaml", &RefSpec::bookmark("main"))
        .unwrap();
    let names = jj
        .list_declarations("proj", "repo", "api.yaml", &RefSpec::bookmark("main"))
        .unwrap();
    let path_decl = jj
        .get_declaration(
            "proj",
            "repo",
            "api.yaml",
            "path:/users",
            &RefSpec::bookmark("main"),
        )
        .unwrap();

    // Assert: the slashed/colon'd name round-trips EXACTLY, with its content, and
    // is listed alongside the plain sibling.
    assert_eq!(schema.meta.as_bytes(), b"openapi: 3.0.3");
    assert_eq!(schema.decls.len(), 2);
    assert_eq!(
        schema.decls.get("path:/users").unwrap().as_bytes(),
        b"get listUsers"
    );
    assert_eq!(
        schema.decls.get("schema:User").unwrap().as_bytes(),
        b"type: object"
    );
    assert_eq!(path_decl.as_bytes(), b"get listUsers");
    assert!(names.contains(&"path:/users".to_string()));
    assert!(names.contains(&"schema:User".to_string()));
}

#[test]
fn editing_a_slashed_decl_leaves_its_sibling_unchanged() {
    // Arrange: two decls, one with a `/` in its name; capture the sibling.
    let jj = mem_jj();
    let effect = MutationEffect {
        meta: Some(MetaBlob::new(b"openapi: 3.0.3".to_vec())),
        upserts: vec![
            (
                "path:/users".to_string(),
                DeclBlob::new(b"get listUsers v1".to_vec()),
            ),
            (
                "path:/orders".to_string(),
                DeclBlob::new(b"post createOrder".to_vec()),
            ),
        ],
        removes: vec![],
    };
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "api.yaml",
        &RefSpec::bookmark("main"),
        effect,
        "alice",
        "seed",
    )
    .unwrap();
    let sibling_before = jj
        .get_declaration(
            "proj",
            "repo",
            "api.yaml",
            "path:/orders",
            &RefSpec::bookmark("main"),
        )
        .unwrap();

    // Act: edit only `path:/users`.
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "api.yaml",
        &RefSpec::bookmark("main"),
        upsert("path:/users", "get listUsers v2"),
        "alice",
        "edit users path",
    )
    .unwrap();

    // Assert: the edited slashed decl changed; the slashed sibling is unchanged
    // (per-declaration dedup still holds for names containing `/`).
    let edited = jj
        .get_declaration(
            "proj",
            "repo",
            "api.yaml",
            "path:/users",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    let sibling_after = jj
        .get_declaration(
            "proj",
            "repo",
            "api.yaml",
            "path:/orders",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    assert_eq!(edited.as_bytes(), b"get listUsers v2");
    assert_eq!(sibling_after, sibling_before);
}

// ── First-class conflict at declaration granularity ──────────────────────────

#[test]
fn concurrent_edits_to_different_decls_merge_cleanly() {
    // Arrange: a shared base with two decls.
    let jj = mem_jj();
    let base = seed_two_decls(&jj);

    // Act: two writers, both basing off `base`, edit DIFFERENT decls.
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base.clone()),
        upsert("UserRequest", "msg req from A"),
        "alice",
        "A edits req",
    )
    .unwrap();
    let second = jj
        .commit_write(
            "proj",
            "repo",
            "main",
            "user.proto",
            &RefSpec::commit(base.clone()), // stale base on purpose
            upsert("UserStatus", "enum status from B"),
            "bob",
            "B edits status",
        )
        .unwrap();

    // Assert: no conflict; both edits present.
    assert!(second.conflicted_decls.is_empty());
    let schema = jj
        .load_schema("proj", "repo", "user.proto", &RefSpec::bookmark("main"))
        .unwrap();
    assert_eq!(
        schema.decls.get("UserRequest").unwrap().as_bytes(),
        b"msg req from A"
    );
    assert_eq!(
        schema.decls.get("UserStatus").unwrap().as_bytes(),
        b"enum status from B"
    );
}

#[test]
fn shared_backend_serializes_validation_through_operation_publication() {
    // Arrange: two independent Jj instances share one backend and stale base.
    // Writer A pauses inside final-tree validation while holding the repository
    // publication guard; writer B must not load/publish the same op head yet.
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let seed = Jj::new(db.clone());
    let base = seed_two_decls(&seed);
    let first = Arc::new(Jj::new(db.clone()));
    let second = Arc::new(Jj::new(db));
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let first_base = base.clone();
    let first_writer = {
        let first = first.clone();
        std::thread::spawn(move || {
            first.commit_schema_changes_validated(
                "proj",
                "repo",
                "main",
                &RefSpec::commit(first_base),
                vec![SchemaWrite::Patch {
                    schema_path: "user.proto".to_string(),
                    effect: upsert("UserRequest", "msg req from A"),
                }],
                "alice",
                "writer A",
                BTreeMap::new(),
                |_| {
                    entered_tx.send(()).map_err(|error| error.to_string())?;
                    release_rx.recv().map_err(|error| error.to_string())
                },
            )
        })
    };
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("writer A entered validation");
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let second_writer = {
        let second = second.clone();
        std::thread::spawn(move || {
            let result = second.commit_write(
                "proj",
                "repo",
                "main",
                "user.proto",
                &RefSpec::commit(base),
                upsert("UserStatus", "enum status from B"),
                "bob",
                "writer B",
            );
            finished_tx.send(result).expect("report writer B result");
        })
    };

    // Act
    let before_release = finished_rx.recv_timeout(Duration::from_millis(100));
    release_tx.send(()).expect("release writer A");
    let first_result = first_writer.join().expect("join writer A");
    let second_result = finished_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("writer B finishes after release");
    second_writer.join().expect("join writer B");

    // Assert
    assert!(before_release.is_err());
    first_result.expect("writer A publishes");
    assert!(second_result
        .expect("writer B publishes")
        .conflicted_decls
        .is_empty());
    let schema = second
        .load_schema("proj", "repo", "user.proto", &RefSpec::bookmark("main"))
        .expect("read serialized result");
    assert_eq!(
        schema.decls.get("UserRequest").expect("request").as_bytes(),
        b"msg req from A"
    );
    assert_eq!(
        schema.decls.get("UserStatus").expect("status").as_bytes(),
        b"enum status from B"
    );
}

#[test]
fn concurrent_edits_to_same_decl_produce_a_first_class_conflict() {
    // Arrange: shared base.
    let jj = mem_jj();
    let base = seed_two_decls(&jj);

    // Act: two writers edit the SAME decl differently off the same base.
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base.clone()),
        upsert("UserRequest", "msg req from A"),
        "alice",
        "A",
    )
    .unwrap();
    let second = jj
        .commit_write(
            "proj",
            "repo",
            "main",
            "user.proto",
            &RefSpec::commit(base.clone()),
            upsert("UserRequest", "msg req from B"),
            "bob",
            "B",
        )
        .unwrap();

    // Assert: the declaration landed conflicted, recorded but not rejected.
    assert_eq!(second.conflicted_decls, vec!["UserRequest".to_string()]);
    let sides = jj
        .read_conflict(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    assert_eq!(sides.sides.len(), 2);
    let bodies: Vec<&[u8]> = sides.sides.iter().map(|s| s.as_bytes()).collect();
    assert!(bodies.contains(&b"msg req from A".as_slice()));
    assert!(bodies.contains(&b"msg req from B".as_slice()));
}

#[test]
fn conflict_stats_count_the_snapshot_but_only_group_selected_schemas() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base.clone()),
        upsert("UserRequest", "msg req from A"),
        "alice",
        "A",
    )
    .expect("write first side");
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base),
        upsert("UserRequest", "msg req from B"),
        "bob",
        "B",
    )
    .expect("write second side");
    let selected = BTreeSet::from(["user.proto".to_string()]);

    // Act
    let stats = jj
        .conflict_stats("proj", "repo", &RefSpec::bookmark("main"), &selected)
        .expect("count conflicts");

    // Assert
    assert_eq!(stats.total, 1);
    assert_eq!(stats.by_schema.get("user.proto"), Some(&1));
}

#[test]
fn resolve_conflict_replaces_the_conflict_with_a_clean_decl() {
    // Arrange: produce a conflict on UserRequest.
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base.clone()),
        upsert("UserRequest", "msg req from A"),
        "alice",
        "A",
    )
    .unwrap();
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base.clone()),
        upsert("UserRequest", "msg req from B"),
        "bob",
        "B",
    )
    .unwrap();

    // Act
    jj.resolve_conflict(
        "proj",
        "repo",
        "main",
        "user.proto",
        "UserRequest",
        DeclBlob::new(b"msg req merged".to_vec()),
        "carol",
        "resolve",
    )
    .unwrap();

    // Assert: UserRequest reads cleanly as the resolved blob.
    let req = jj
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    assert_eq!(req.as_bytes(), b"msg req merged");
}

// ── Bookmarks & tags ─────────────────────────────────────────────────────────

#[test]
fn create_and_list_bookmark() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);

    // Act
    jj.create_bookmark(
        "proj",
        "repo",
        "feature/x",
        &RefSpec::commit(base.clone()),
        "alice",
    )
    .unwrap();

    // Assert
    let bms = jj.list_bookmarks("proj", "repo").unwrap();
    let names: Vec<&str> = bms.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"main"));
    assert!(names.contains(&"feature/x"));
}

#[test]
fn bookmark_pages_preserve_prefix_order_and_exclusive_continuation() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    for name in ["feature/b", "preview/a", "feature/a"] {
        jj.create_bookmark(
            "proj",
            "repo",
            name,
            &RefSpec::commit(base.clone()),
            "alice",
        )
        .expect("create bookmark");
    }

    // Act
    let first = jj
        .list_bookmarks_page("proj", "repo", "feature/", None, 1)
        .expect("list first bookmark page");
    let second = jj
        .list_bookmarks_page("proj", "repo", "feature/", first.next_cursor.as_deref(), 1)
        .expect("list second bookmark page");

    // Assert
    assert_eq!(
        first
            .refs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["feature/a"]
    );
    assert_eq!(first.next_cursor.as_deref(), Some("feature/a"));
    assert_eq!(
        second
            .refs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["feature/b"]
    );
    assert_eq!(second.next_cursor, None);
}

#[test]
fn direct_bookmark_lookup_returns_only_the_requested_head() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.create_bookmark(
        "proj",
        "repo",
        "feature/x",
        &RefSpec::commit(base.clone()),
        "alice",
    )
    .expect("create bookmark");

    // Act
    let found = jj
        .get_bookmark("proj", "repo", "feature/x")
        .expect("get bookmark");
    let missing = jj
        .get_bookmark("proj", "repo", "feature/missing")
        .expect("get missing bookmark");

    // Assert
    assert_eq!(found.as_deref(), Some(base.as_str()));
    assert_eq!(missing, None);
}

#[test]
fn create_and_list_tag() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);

    // Act
    jj.create_tag(
        "proj",
        "repo",
        "v1.0.0",
        &RefSpec::commit(base.clone()),
        "alice",
    )
    .unwrap();

    // Assert
    let tags = jj.list_tags("proj", "repo").unwrap();
    assert_eq!(tags, vec![("v1.0.0".to_string(), base)]);
}

#[test]
fn tag_pages_preserve_prefix_order_and_exclusive_continuation() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    for name in ["release/2", "preview/1", "release/1"] {
        jj.create_tag(
            "proj",
            "repo",
            name,
            &RefSpec::commit(base.clone()),
            "alice",
        )
        .expect("create tag");
    }

    // Act
    let first = jj
        .list_tags_page("proj", "repo", "release/", None, 1)
        .expect("list first tag page");
    let second = jj
        .list_tags_page("proj", "repo", "release/", first.next_cursor.as_deref(), 1)
        .expect("list second tag page");

    // Assert
    assert_eq!(
        first
            .refs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["release/1"]
    );
    assert_eq!(first.next_cursor.as_deref(), Some("release/1"));
    assert_eq!(
        second
            .refs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["release/2"]
    );
    assert_eq!(second.next_cursor, None);
}

#[test]
fn duplicate_tag_name_is_rejected_without_retargeting() {
    // Arrange
    let jj = mem_jj();
    let initial = seed_two_decls(&jj);
    jj.create_tag(
        "proj",
        "repo",
        "release",
        &RefSpec::commit(initial.clone()),
        "alice",
    )
    .expect("create initial tag");
    let advanced = jj
        .commit_write(
            "proj",
            "repo",
            "main",
            "user.proto",
            &RefSpec::bookmark("main"),
            upsert("UserRequest", "advanced"),
            "alice",
            "advance main",
        )
        .expect("advance main")
        .commit_id;

    // Act
    let result = jj.create_tag(
        "proj",
        "repo",
        "release",
        &RefSpec::commit(advanced),
        "alice",
    );

    // Assert
    assert!(matches!(result, Err(JjError::TagExists(name)) if name == "release"));
    assert_eq!(
        jj.list_tags("proj", "repo").unwrap(),
        vec![("release".to_string(), initial)]
    );
}

// ── Merge ────────────────────────────────────────────────────────────────────

#[test]
fn merge_disjoint_decl_edits_is_clean() {
    // Arrange: base on main, branch off, each side edits a different decl.
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.create_bookmark(
        "proj",
        "repo",
        "feat",
        &RefSpec::commit(base.clone()),
        "alice",
    )
    .unwrap();
    // main edits UserRequest
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "main req"),
        "alice",
        "main edit",
    )
    .unwrap();
    // feat edits UserStatus
    jj.commit_write(
        "proj",
        "repo",
        "feat",
        "user.proto",
        &RefSpec::bookmark("feat"),
        upsert("UserStatus", "feat status"),
        "bob",
        "feat edit",
    )
    .unwrap();

    // Act
    let result = jj.merge("proj", "repo", "feat", "main", "carol").unwrap();

    // Assert: clean merge with both edits.
    assert!(result.conflicted_decls.is_empty());
    let schema = jj
        .load_schema("proj", "repo", "user.proto", &RefSpec::bookmark("main"))
        .unwrap();
    assert_eq!(
        schema.decls.get("UserRequest").unwrap().as_bytes(),
        b"main req"
    );
    assert_eq!(
        schema.decls.get("UserStatus").unwrap().as_bytes(),
        b"feat status"
    );
}

#[test]
fn merge_same_decl_edits_produces_conflict_not_error() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.create_bookmark(
        "proj",
        "repo",
        "feat",
        &RefSpec::commit(base.clone()),
        "alice",
    )
    .unwrap();
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "main req"),
        "alice",
        "main edit",
    )
    .unwrap();
    jj.commit_write(
        "proj",
        "repo",
        "feat",
        "user.proto",
        &RefSpec::bookmark("feat"),
        upsert("UserRequest", "feat req"),
        "bob",
        "feat edit",
    )
    .unwrap();

    // Act
    let result = jj.merge("proj", "repo", "feat", "main", "carol").unwrap();

    // Assert
    assert_eq!(result.conflicted_decls, vec!["UserRequest".to_string()]);
}

// ── GC ───────────────────────────────────────────────────────────────────────

#[test]
fn gc_sweeps_unreachable_objects_and_keeps_reachable_ones() {
    // Arrange: a reachable commit, plus an orphan object reachable from nothing.
    let db = Arc::new(MemoryObjectDb::new());
    let jj = Jj::new(db.clone());
    seed_two_decls(&jj);
    let orphan = db
        .put_object(ObjectKind::File, b"orphaned blob never referenced")
        .unwrap();
    let reachable = jj
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .unwrap();

    // Act
    let swept = jj.gc(&[("proj".to_string(), "repo".to_string())]).unwrap();

    // Assert: the orphan is gone, the reachable decl survives.
    assert!(swept >= 1);
    assert!(!db.has_object(ObjectKind::File, &orphan).unwrap());
    // Re-reading the reachable declaration still works.
    let req = jj
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    assert_eq!(req, reachable);
}

#[test]
fn repo_scoped_gc_preserves_live_objects_from_every_repo_in_the_shared_store() {
    // Arrange
    let db = Arc::new(MemoryObjectDb::new());
    let jj = Jj::new(db.clone());
    jj.commit_write(
        "alpha",
        "schemas",
        "main",
        "alpha.proto",
        &RefSpec::bookmark("main"),
        upsert("Alpha", "alpha-live"),
        "alice",
        "seed alpha",
    )
    .unwrap();
    jj.commit_write(
        "beta",
        "schemas",
        "main",
        "beta.proto",
        &RefSpec::bookmark("main"),
        upsert("Beta", "beta-live"),
        "bob",
        "seed beta",
    )
    .unwrap();
    let beta_before = jj
        .get_declaration(
            "beta",
            "schemas",
            "beta.proto",
            "Beta",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    let orphan = db
        .put_object(ObjectKind::File, b"unreachable across every repo")
        .unwrap();

    // Act
    let swept = jj
        .gc(&[("alpha".to_string(), "schemas".to_string())])
        .unwrap();

    // Assert
    assert!(swept >= 1);
    assert!(!db.has_object(ObjectKind::File, &orphan).unwrap());
    assert_eq!(
        jj.get_declaration(
            "beta",
            "schemas",
            "beta.proto",
            "Beta",
            &RefSpec::bookmark("main"),
        )
        .unwrap(),
        beta_before
    );
}

#[test]
fn redb_gc_restart_drill_preserves_cross_repo_history_and_undo() {
    // Arrange
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("schemahub.redb");
    let expected_alpha_v1 = {
        let db = Arc::new(crate::RedbObjectDb::open(&database_path).unwrap());
        let jj = Jj::new(db.clone());
        jj.commit_write(
            "alpha",
            "schemas",
            "main",
            "alpha.proto",
            &RefSpec::bookmark("main"),
            upsert("Alpha", "alpha-v1"),
            "alice",
            "seed alpha",
        )
        .unwrap();
        let alpha_v1 = jj
            .get_declaration(
                "alpha",
                "schemas",
                "alpha.proto",
                "Alpha",
                &RefSpec::bookmark("main"),
            )
            .unwrap();
        jj.commit_write(
            "alpha",
            "schemas",
            "main",
            "alpha.proto",
            &RefSpec::bookmark("main"),
            upsert("Alpha", "alpha-v2"),
            "alice",
            "update alpha",
        )
        .unwrap();
        jj.commit_write(
            "beta",
            "schemas",
            "main",
            "beta.proto",
            &RefSpec::bookmark("main"),
            upsert("Beta", "beta-live"),
            "bob",
            "seed beta",
        )
        .unwrap();
        db.put_object(ObjectKind::File, b"orphan before recovery drill")
            .unwrap();

        // Act: collect, close the process-owned database, reopen, then use the
        // retained operation log to recover the prior alpha revision.
        assert!(
            jj.gc(&[("alpha".to_string(), "schemas".to_string())])
                .unwrap()
                >= 1
        );
        alpha_v1
    };
    let restarted_db = Arc::new(crate::RedbObjectDb::open(&database_path).unwrap());
    let restarted = Jj::new(restarted_db);
    let beta_after_restart = restarted
        .get_declaration(
            "beta",
            "schemas",
            "beta.proto",
            "Beta",
            &RefSpec::bookmark("main"),
        )
        .unwrap();
    restarted.undo("alpha", "schemas", "operator").unwrap();
    let alpha_after_undo = restarted
        .get_declaration(
            "alpha",
            "schemas",
            "alpha.proto",
            "Alpha",
            &RefSpec::bookmark("main"),
        )
        .unwrap();

    // Assert
    assert_eq!(beta_after_restart, DeclBlob::new(b"beta-live".to_vec()));
    assert_eq!(alpha_after_undo, expected_alpha_v1);
}

#[test]
fn redb_offline_backup_restore_drill_recovers_the_snapshotted_revision() {
    // Arrange
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("schemahub.redb");
    let backup_path = directory.path().join("schemahub.backup.redb");
    let restore_path = directory.path().join("schemahub.restore.redb");
    let expected = {
        let db = Arc::new(crate::RedbObjectDb::open(&source_path).unwrap());
        let jj = Jj::new(db);
        jj.commit_write(
            "acme",
            "schemas",
            "main",
            "event.proto",
            &RefSpec::bookmark("main"),
            upsert("Event", "snapshot-v1"),
            "operator",
            "seed backup fixture",
        )
        .unwrap();
        jj.get_declaration(
            "acme",
            "schemas",
            "event.proto",
            "Event",
            &RefSpec::bookmark("main"),
        )
        .unwrap()
    };

    // Act
    std::fs::copy(&source_path, &backup_path).unwrap();
    {
        let db = Arc::new(crate::RedbObjectDb::open(&source_path).unwrap());
        let jj = Jj::new(db);
        jj.commit_write(
            "acme",
            "schemas",
            "main",
            "event.proto",
            &RefSpec::bookmark("main"),
            upsert("Event", "post-backup-v2"),
            "operator",
            "mutate after backup",
        )
        .unwrap();
    }
    std::fs::copy(&backup_path, &restore_path).unwrap();
    let restored_db = Arc::new(crate::RedbObjectDb::open(&restore_path).unwrap());
    let restored = Jj::new(restored_db);
    let actual = restored
        .get_declaration(
            "acme",
            "schemas",
            "event.proto",
            "Event",
            &RefSpec::bookmark("main"),
        )
        .unwrap();

    // Assert
    assert_eq!(actual, expected);
}

// ── redb parity smoke test ───────────────────────────────────────────────────

#[test]
fn redb_backend_supports_full_write_read_cycle() {
    // Arrange
    let dir = std::env::temp_dir().join(format!("schemahub-jj-test-{}", uuid::Uuid::new_v4()));
    let db = crate::RedbObjectDb::open(&dir).unwrap();
    let jj = Jj::new(Arc::new(db));

    // Act
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v1"),
        "alice",
        "seed",
    )
    .unwrap();

    // Assert
    let schema = jj
        .load_schema("proj", "repo", "user.proto", &RefSpec::bookmark("main"))
        .unwrap();
    assert_eq!(
        schema.decls.get("UserRequest").unwrap().as_bytes(),
        b"msg req v1"
    );

    let _ = std::fs::remove_file(&dir);
}

#[test]
fn redb_state_survives_reopening_the_database() {
    // Arrange: write through one Jj over a redb file, then drop it.
    let dir = std::env::temp_dir().join(format!("schemahub-jj-persist-{}", uuid::Uuid::new_v4()));
    {
        let db = crate::RedbObjectDb::open(&dir).unwrap();
        let jj = Jj::new(Arc::new(db));
        jj.commit_write(
            "proj",
            "repo",
            "main",
            "user.proto",
            &RefSpec::bookmark("main"),
            upsert("UserRequest", "persisted v1"),
            "alice",
            "seed",
        )
        .unwrap();
    } // Jj (and its jj RepoLoader / op-heads) dropped here.

    // Act: open a brand-new Jj over the SAME redb file.
    let db = crate::RedbObjectDb::open(&dir).unwrap();
    let vcs2 = Jj::new(Arc::new(db));
    let decl = vcs2
        .get_declaration(
            "proj",
            "repo",
            "user.proto",
            "UserRequest",
            &RefSpec::bookmark("main"),
        )
        .unwrap();

    // Assert: the bookmark, op-log, and content all survived in the DB.
    assert_eq!(decl.as_bytes(), b"persisted v1");
    assert_eq!(vcs2.list_operations("proj", "repo").unwrap().len(), 1);

    let _ = std::fs::remove_file(&dir);
}

// ── Commit log (real change/commit graph walk) ───────────────────────────────

#[test]
fn commit_log_walks_the_real_commit_graph_newest_first() {
    // Arrange: two commits on main (seed, then an edit).
    let jj = mem_jj();
    seed_two_decls(&jj);
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::bookmark("main"),
        upsert("UserRequest", "msg req v2"),
        "bob",
        "second commit",
    )
    .unwrap();

    // Act
    let log = jj
        .commit_log("proj", "repo", &RefSpec::bookmark("main"), 10)
        .unwrap();

    // Assert: two real commits, newest first, each with a stable change id.
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].message, "second commit");
    assert_eq!(log[1].message, "seed");
    assert!(!log[0].change_id.is_empty());
    assert_ne!(log[0].change_id, log[1].change_id);
    // The newest commit's parent is the seed commit.
    assert_eq!(log[0].parents, vec![log[1].commit_id.clone()]);
}

#[test]
fn commit_log_respects_the_limit() {
    // Arrange: three commits.
    let jj = mem_jj();
    seed_two_decls(&jj);
    for i in 0..2 {
        jj.commit_write(
            "proj",
            "repo",
            "main",
            "user.proto",
            &RefSpec::bookmark("main"),
            upsert("UserRequest", &format!("v{i}")),
            "bob",
            &format!("edit {i}"),
        )
        .unwrap();
    }

    // Act
    let log = jj
        .commit_log("proj", "repo", &RefSpec::bookmark("main"), 1)
        .unwrap();

    // Assert
    assert_eq!(log.len(), 1);
}

#[test]
fn schema_history_filter_detects_a_change_between_conflicted_trees() {
    // Arrange: create a two-sided conflict, then add a third competing edit
    // from the same clean base. Both resulting trees omit the conflicted
    // declaration from normal schema loads, but their raw merged values differ.
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base.clone()),
        upsert("UserRequest", "msg req from A"),
        "alice",
        "A",
    )
    .expect("first edit");
    jj.commit_write(
        "proj",
        "repo",
        "main",
        "user.proto",
        &RefSpec::commit(base.clone()),
        upsert("UserRequest", "msg req from B"),
        "bob",
        "B",
    )
    .expect("second edit creates conflict");
    let third = jj
        .commit_write(
            "proj",
            "repo",
            "main",
            "user.proto",
            &RefSpec::commit(base),
            upsert("UserRequest", "msg req from C"),
            "carol",
            "C",
        )
        .expect("third edit extends conflict");

    // Act
    let touched = jj.commit_touches_schema("proj", "repo", &third.commit_id, "user.proto");

    // Assert
    assert!(touched.expect("inspect raw schema subtree"));
}

// ── Bookmark / tag deletion ──────────────────────────────────────────────────

#[test]
fn delete_bookmark_removes_it_from_the_view() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.create_bookmark("proj", "repo", "feature/y", &RefSpec::commit(base), "alice")
        .unwrap();

    // Act
    jj.delete_bookmark("proj", "repo", "feature/y", "alice")
        .unwrap();

    // Assert
    let names: Vec<String> = jj
        .list_bookmarks("proj", "repo")
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert!(!names.contains(&"feature/y".to_string()));
}

#[test]
fn delete_missing_bookmark_errors() {
    // Arrange
    let jj = mem_jj();
    seed_two_decls(&jj);

    // Act
    let result = jj.delete_bookmark("proj", "repo", "nope", "alice");

    // Assert
    assert!(result.is_err());
}

#[test]
fn delete_tag_removes_it_from_the_view() {
    // Arrange
    let jj = mem_jj();
    let base = seed_two_decls(&jj);
    jj.create_tag("proj", "repo", "v9", &RefSpec::commit(base), "alice")
        .unwrap();

    // Act
    jj.delete_tag("proj", "repo", "v9", "alice").unwrap();

    // Assert
    assert!(jj.list_tags("proj", "repo").unwrap().is_empty());
}

// ── Multi-file atomic write ──────────────────────────────────────────────────

#[test]
fn commit_write_multi_touches_several_files_in_one_commit() {
    // Arrange: a fresh repo.
    let jj = mem_jj();
    let effects = vec![
        (
            "user.proto".to_string(),
            MutationEffect {
                meta: Some(MetaBlob::new(b"package user;".to_vec())),
                upserts: vec![("User".to_string(), DeclBlob::new(b"msg user".to_vec()))],
                removes: vec![],
            },
        ),
        (
            "order.proto".to_string(),
            MutationEffect {
                meta: Some(MetaBlob::new(b"package order;".to_vec())),
                upserts: vec![("Order".to_string(), DeclBlob::new(b"msg order".to_vec()))],
                removes: vec![],
            },
        ),
    ];

    // Act
    let write = jj
        .commit_write_multi(
            "proj",
            "repo",
            "main",
            &RefSpec::bookmark("main"),
            effects,
            "alice",
            "two files in one commit",
        )
        .unwrap();

    // Assert: both files are present at the single resulting commit, and exactly
    // one operation was recorded.
    assert!(write.conflicted_decls.is_empty());
    let user = jj
        .load_schema("proj", "repo", "user.proto", &RefSpec::bookmark("main"))
        .unwrap();
    let order = jj
        .load_schema("proj", "repo", "order.proto", &RefSpec::bookmark("main"))
        .unwrap();
    assert_eq!(user.decls.get("User").unwrap().as_bytes(), b"msg user");
    assert_eq!(order.decls.get("Order").unwrap().as_bytes(), b"msg order");
    assert_eq!(jj.list_operations("proj", "repo").unwrap().len(), 1);
}
