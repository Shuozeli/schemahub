//! End-to-end integration tests for the Jujutsu-style version-control workflows
//! of the schema registry, driven through the real in-process gRPC server:
//! bookmarks/branches, tags, merge, diff, history/undo, and idempotency.
//!
//! Shapes mirror `e2e.rs`; the shared harness lives in `common/mod.rs`.

mod common;

use common::*;
use schemahub_api::schemahub_v1 as pb;

const USER_PROTO: &str = r#"syntax = "proto3";
package user.v1;

message User {
  string id = 1;
  string name = 2;
}
"#;

/// Build an `apply_mutation` request that adds an OPTIONAL (singular) field to a
/// protobuf message. An optional add is FULL-compatible, so it is allowed even on
/// the protected `main` bookmark.
fn add_field_request(
    project: &str,
    repo: &str,
    branch: &str,
    schema_path: &str,
    message_name: &str,
    field_name: &str,
    field_number: u32,
    idempotency_key: &str,
) -> pb::ApplyMutationRequest {
    pb::ApplyMutationRequest {
        project: project.into(),
        repo: repo.into(),
        branch: branch.into(),
        base_revision: String::new(),
        idempotency_key: idempotency_key.into(),
        force: false,
        operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
            pb::ProtobufMutation {
                schema_path: schema_path.into(),
                operation: Some(pb::protobuf_mutation::Operation::AddField(
                    pb::ProtoAddField {
                        message_name: message_name.into(),
                        field_name: field_name.into(),
                        field_type: "string".into(),
                        field_number,
                        repeated: false,
                        doc_comment: String::new(),
                    },
                )),
            },
        )),
    }
}

// ── 1. Bookmark isolation ──────────────────────────────────────────────────────

#[tokio::test]
async fn branch_mutation_is_isolated_from_main() {
    // Arrange: a schema on `main`, then a `feature/x` branch off main.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "k1",
    )
    .await;
    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "feature/x".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create_branch");

    // Act: add a field only on `feature/x`.
    c.schema
        .apply_mutation(add_field_request(
            "acme",
            "core",
            "feature/x",
            "user.proto",
            "User",
            "email",
            3,
            "k2",
        ))
        .await
        .expect("apply_mutation on feature/x");

    // Assert: the new field shows at `feature/x` but NOT at `main`.
    let on_feature =
        pull_source(&mut c.explore, "acme", "core", "user.proto", vref_branch("feature/x")).await;
    let on_main =
        pull_source(&mut c.explore, "acme", "core", "user.proto", vref_branch("main")).await;
    assert!(
        on_feature.contains("email"),
        "feature/x should have the new field, got:\n{on_feature}"
    );
    assert!(
        !on_main.contains("email"),
        "main must NOT have the field added on feature/x, got:\n{on_main}"
    );

    // Assert: both bookmarks are listed.
    let branches = c
        .refs
        .list_branches(pb::ListBranchesRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: String::new(),
        })
        .await
        .expect("list_branches")
        .into_inner();
    let names: Vec<&str> = branches.branches.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"main"), "main should be listed: {names:?}");
    assert!(
        names.contains(&"feature/x"),
        "feature/x should be listed: {names:?}"
    );
}

// ── 2. Tag + pull-at-tag immutability ───────────────────────────────────────────

#[tokio::test]
async fn tag_pins_an_immutable_snapshot() {
    // Arrange: a schema on `main`.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "k1",
    )
    .await;

    // Act: tag main's current HEAD, then advance main with a new field.
    let tag = c
        .refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "v1.0.0".into(),
            target: Some(vref_branch("main")),
            message: String::new(),
        })
        .await
        .expect("create_tag")
        .into_inner();
    assert!(!tag.tag.unwrap().commit_hash.is_empty(), "tag should pin a commit");

    // The tag must already resolve to the pre-mutation snapshot.
    let at_tag_before =
        pull_source(&mut c.explore, "acme", "core", "user.proto", vref_tag("v1.0.0")).await;
    assert!(
        at_tag_before.contains("message User"),
        "tag should resolve to the schema, got:\n{at_tag_before}"
    );

    c.schema
        .apply_mutation(add_field_request(
            "acme", "core", "main", "user.proto", "User", "email", 3, "k2",
        ))
        .await
        .expect("apply_mutation on main");

    // Assert: the tag still shows the pre-mutation state; main shows the new field.
    let at_tag_after =
        pull_source(&mut c.explore, "acme", "core", "user.proto", vref_tag("v1.0.0")).await;
    let at_main_after =
        pull_source(&mut c.explore, "acme", "core", "user.proto", vref_branch("main")).await;
    assert!(
        !at_tag_after.contains("email"),
        "tag is immutable: it must NOT show the post-tag field, got:\n{at_tag_after}"
    );
    assert!(
        at_main_after.contains("email"),
        "main should show the new field, got:\n{at_main_after}"
    );

    // Assert: the tag is listed.
    let tags = c
        .refs
        .list_tags(pb::ListTagsRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: String::new(),
        })
        .await
        .expect("list_tags")
        .into_inner();
    assert!(
        tags.tags.iter().any(|t| t.name == "v1.0.0"),
        "v1.0.0 should be listed: {:?}",
        tags.tags.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}

// ── 3. Merge ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn merge_brings_branch_field_into_main() {
    // Arrange: a schema on `main` and a `feature/y` branch that adds a field.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "k1",
    )
    .await;
    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "feature/y".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create_branch");
    c.schema
        .apply_mutation(add_field_request(
            "acme",
            "core",
            "feature/y",
            "user.proto",
            "User",
            "email",
            3,
            "k2",
        ))
        .await
        .expect("apply_mutation on feature/y");

    // Sanity: main does not yet have the field.
    let main_before =
        pull_source(&mut c.explore, "acme", "core", "user.proto", vref_branch("main")).await;
    assert!(!main_before.contains("email"), "precondition: main lacks email");

    // Act: merge feature/y into main.
    let merge = c
        .refs
        .merge(pb::MergeRequest {
            project: "acme".into(),
            repo: "core".into(),
            source_branch: "feature/y".into(),
            target_branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "m1".into(),
            message: "merge feature/y".into(),
        })
        .await
        .expect("merge")
        .into_inner();

    // Assert: the merge produced a commit and main now carries the field.
    assert!(
        !merge.new_commit.is_empty(),
        "merge should return a non-empty commit id"
    );
    let main_after =
        pull_source(&mut c.explore, "acme", "core", "user.proto", vref_branch("main")).await;
    assert!(
        main_after.contains("email"),
        "main should have the merged-in field, got:\n{main_after}"
    );
}

// ── 4. Diff between refs ────────────────────────────────────────────────────────

#[tokio::test]
async fn diff_reports_changed_declaration_between_refs() {
    // Arrange: tag main's HEAD, then add a field on main so the two refs differ.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "k1",
    )
    .await;
    c.refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "before".into(),
            target: Some(vref_branch("main")),
            message: String::new(),
        })
        .await
        .expect("create_tag");
    c.schema
        .apply_mutation(add_field_request(
            "acme", "core", "main", "user.proto", "User", "email", 3, "k2",
        ))
        .await
        .expect("apply_mutation on main");

    // Act: diff the `before` tag (base) against `main` (head).
    let diff = c
        .refs
        .diff(pb::DiffRequest {
            project: "acme".into(),
            repo: "core".into(),
            base: Some(vref_tag("before")),
            head: Some(vref_branch("main")),
            schema_path: "user.proto".into(),
        })
        .await
        .expect("diff")
        .into_inner();

    // Assert: the diff is non-empty and reports the modified `User` declaration.
    assert!(
        !diff.schema_diffs.is_empty(),
        "diff should report at least one changed schema file"
    );
    let user_changes: Vec<&pb::DeclarationChange> = diff
        .schema_diffs
        .iter()
        .flat_map(|sd| sd.changes.iter())
        .filter(|ch| ch.decl_name == "User")
        .collect();
    assert!(
        !user_changes.is_empty(),
        "diff should report a change for the `User` declaration, got: {:?}",
        diff.schema_diffs
    );
    assert!(
        user_changes.iter().any(|ch| ch.change_type == "modified"),
        "User should be reported as modified, got: {:?}",
        user_changes.iter().map(|c| &c.change_type).collect::<Vec<_>>()
    );
}

// ── 5. History + multiple undos ─────────────────────────────────────────────────

#[tokio::test]
async fn history_log_and_repeated_undo_roll_back() {
    // Arrange: three writes on main (create + two add-field).
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "h1",
    )
    .await;
    c.schema
        .apply_mutation(add_field_request(
            "acme", "core", "main", "user.proto", "User", "email", 3, "h2",
        ))
        .await
        .expect("add email");
    c.schema
        .apply_mutation(add_field_request(
            "acme", "core", "main", "user.proto", "User", "phone", 4, "h3",
        ))
        .await
        .expect("add phone");

    // Act 1: read the commit log and the operation log.
    let log = c
        .history
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "core".into(),
        })
        .await
        .expect("log")
        .into_inner();
    let oplog = c
        .history
        .op_log(pb::OpLogRequest {
            project: "acme".into(),
            repo: "core".into(),
        })
        .await
        .expect("op_log")
        .into_inner();

    // Assert: three commits, newest-first, with linear parent links.
    assert_eq!(log.entries.len(), 3, "expected 3 commits in the graph");
    assert_eq!(
        log.entries[0].parents,
        vec![log.entries[1].commit_id.clone()],
        "head parent should be the middle commit"
    );
    assert_eq!(
        log.entries[1].parents,
        vec![log.entries[2].commit_id.clone()],
        "middle parent should be the base commit"
    );
    assert!(
        oplog.operations.len() >= 3,
        "expected at least 3 operations, got {}",
        oplog.operations.len()
    );

    // Sanity: both fields present before undo.
    let before = pull_source(&mut c.explore, "acme", "core", "user.proto", vref_branch("main")).await;
    assert!(before.contains("email") && before.contains("phone"), "got:\n{before}");

    // Act 2: undo twice (drops phone, then email).
    c.history
        .undo(pb::UndoRequest {
            project: "acme".into(),
            repo: "core".into(),
            author: "tester".into(),
        })
        .await
        .expect("first undo");
    c.history
        .undo(pb::UndoRequest {
            project: "acme".into(),
            repo: "core".into(),
            author: "tester".into(),
        })
        .await
        .expect("second undo");

    // Assert: the last two fields are gone; the initial state (id, name) remains.
    let after = pull_source(&mut c.explore, "acme", "core", "user.proto", vref_branch("main")).await;
    assert!(
        !after.contains("email") && !after.contains("phone"),
        "two undos should remove both added fields, got:\n{after}"
    );
    assert!(
        after.contains("id") && after.contains("name"),
        "the original fields should survive, got:\n{after}"
    );

    // Assert: the head moved back — log now shows the base commit at HEAD.
    let log_after = c
        .history
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "core".into(),
        })
        .await
        .expect("log after undo")
        .into_inner();
    assert!(
        log_after.entries.len() < log.entries.len(),
        "log should be shorter after two undos: before={}, after={}",
        log.entries.len(),
        log_after.entries.len()
    );
}

// ── 6. Idempotency dedupe ───────────────────────────────────────────────────────

#[tokio::test]
async fn repeated_mutation_with_same_key_is_deduped() {
    // Arrange: a schema on main, plus the op-log length before the mutation.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "k1",
    )
    .await;
    let ops_before = c
        .history
        .op_log(pb::OpLogRequest {
            project: "acme".into(),
            repo: "core".into(),
        })
        .await
        .expect("op_log before")
        .into_inner()
        .operations
        .len();

    // Act: send the SAME add-field mutation (same idempotency_key) twice.
    let req = add_field_request("acme", "core", "main", "user.proto", "User", "email", 3, "dup");
    let first = c
        .schema
        .apply_mutation(req.clone())
        .await
        .expect("first apply_mutation")
        .into_inner();
    let second = c
        .schema
        .apply_mutation(req)
        .await
        .expect("second apply_mutation")
        .into_inner();

    // Assert: both calls return the SAME commit (the second was deduped).
    assert!(!first.new_commit.is_empty(), "first call should commit");
    assert_eq!(
        first.new_commit, second.new_commit,
        "idempotent retry must return the same commit"
    );

    // Assert: the op-log grew by exactly ONE across both calls.
    let ops_after = c
        .history
        .op_log(pb::OpLogRequest {
            project: "acme".into(),
            repo: "core".into(),
        })
        .await
        .expect("op_log after")
        .into_inner()
        .operations
        .len();
    assert_eq!(
        ops_after - ops_before,
        1,
        "two identical mutations should add exactly one operation (before={ops_before}, after={ops_after})"
    );
}
