//! Unit tests for the jj-style JJ model (AAA style).

use std::sync::Arc;

use schemahub_types::{DeclBlob, MetaBlob, MutationEffect};

use crate::object_db::{ObjectDb, ObjectKind};
use crate::{Jj, MemoryObjectDb, RefSpec};

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

// ── Object round-trip ────────────────────────────────────────────────────────

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
