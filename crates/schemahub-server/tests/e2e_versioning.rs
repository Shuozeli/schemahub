//! End-to-end integration tests for JJ-backed branch/tag/history workflows
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
#[allow(clippy::too_many_arguments)]
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
    let on_feature = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("feature/x"),
    )
    .await;
    let on_main = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
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
            page_size: 0,
            page_token: String::new(),
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
    assert!(
        !tag.tag.unwrap().commit_hash.is_empty(),
        "tag should pin a commit"
    );

    // The tag must already resolve to the pre-mutation snapshot.
    let at_tag_before = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_tag("v1.0.0"),
    )
    .await;
    assert!(
        at_tag_before.contains("message User"),
        "tag should resolve to the schema, got:\n{at_tag_before}"
    );

    c.schema
        .apply_mutation(add_field_request(
            "acme",
            "core",
            "main",
            "user.proto",
            "User",
            "email",
            3,
            "k2",
        ))
        .await
        .expect("apply_mutation on main");

    // Assert: the tag still shows the pre-mutation state; main shows the new field.
    let at_tag_after = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_tag("v1.0.0"),
    )
    .await;
    let at_main_after = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
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
            page_size: 0,
            page_token: String::new(),
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

#[tokio::test]
async fn branch_and_tag_pages_are_prefix_scoped_and_stably_ordered() {
    // Arrange
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
        "ref-page-seed",
    )
    .await;
    for name in ["feature/b", "preview/a", "feature/a"] {
        c.refs
            .create_branch(pb::CreateBranchRequest {
                project: "acme".into(),
                repo: "core".into(),
                name: name.into(),
                from: Some(vref_branch("main")),
            })
            .await
            .expect("create branch");
    }
    for name in ["release/2", "preview/1", "release/1"] {
        c.refs
            .create_tag(pb::CreateTagRequest {
                project: "acme".into(),
                repo: "core".into(),
                name: name.into(),
                target: Some(vref_branch("main")),
                message: String::new(),
            })
            .await
            .expect("create tag");
    }

    // Act
    let first_branches = c
        .refs
        .list_branches(pb::ListBranchesRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: "feature/".into(),
            page_size: 1,
            page_token: String::new(),
        })
        .await
        .expect("list first branch page")
        .into_inner();
    let second_branches = c
        .refs
        .list_branches(pb::ListBranchesRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: "feature/".into(),
            page_size: 1,
            page_token: first_branches.next_page_token.clone(),
        })
        .await
        .expect("list second branch page")
        .into_inner();
    let first_tags = c
        .refs
        .list_tags(pb::ListTagsRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: "release/".into(),
            page_size: 1,
            page_token: String::new(),
        })
        .await
        .expect("list first tag page")
        .into_inner();
    let second_tags = c
        .refs
        .list_tags(pb::ListTagsRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: "release/".into(),
            page_size: 1,
            page_token: first_tags.next_page_token.clone(),
        })
        .await
        .expect("list second tag page")
        .into_inner();
    let branch = c
        .refs
        .get_branch(pb::GetBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "feature/b".into(),
        })
        .await
        .expect("get one branch")
        .into_inner()
        .branch
        .expect("branch");

    // Assert
    assert_eq!(
        first_branches
            .branches
            .iter()
            .map(|branch| branch.name.as_str())
            .collect::<Vec<_>>(),
        ["feature/a"]
    );
    assert!(!first_branches.next_page_token.is_empty());
    assert_eq!(
        second_branches
            .branches
            .iter()
            .map(|branch| branch.name.as_str())
            .collect::<Vec<_>>(),
        ["feature/b"]
    );
    assert!(second_branches.next_page_token.is_empty());
    assert_eq!(
        first_tags
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["release/1"]
    );
    assert!(!first_tags.next_page_token.is_empty());
    assert_eq!(
        second_tags
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["release/2"]
    );
    assert!(second_tags.next_page_token.is_empty());
    assert_eq!(branch.name, "feature/b");
    assert!(!branch.head_commit.is_empty());
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
    let main_before = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        !main_before.contains("email"),
        "precondition: main lacks email"
    );

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
    let main_after = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
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
            "acme",
            "core",
            "main",
            "user.proto",
            "User",
            "email",
            3,
            "k2",
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
        user_changes
            .iter()
            .map(|c| &c.change_type)
            .collect::<Vec<_>>()
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
            "acme",
            "core",
            "main",
            "user.proto",
            "User",
            "email",
            3,
            "h2",
        ))
        .await
        .expect("add email");
    c.schema
        .apply_mutation(add_field_request(
            "acme",
            "core",
            "main",
            "user.proto",
            "User",
            "phone",
            4,
            "h3",
        ))
        .await
        .expect("add phone");

    // Act 1: read the commit log and the operation log.
    let log = c
        .history
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "core".into(),
            at: None,
            limit: 0,
        })
        .await
        .expect("log")
        .into_inner();
    let oplog = c
        .history
        .op_log(pb::OpLogRequest {
            project: "acme".into(),
            repo: "core".into(),
            limit: 0,
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
    let before = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        before.contains("email") && before.contains("phone"),
        "got:\n{before}"
    );

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
    let after = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
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
            at: None,
            limit: 0,
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
            limit: 0,
        })
        .await
        .expect("op_log before")
        .into_inner()
        .operations
        .len();

    // Act: send the SAME add-field mutation (same idempotency_key) twice.
    let req = add_field_request(
        "acme",
        "core",
        "main",
        "user.proto",
        "User",
        "email",
        3,
        "dup",
    );
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
            limit: 0,
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

#[tokio::test]
async fn log_honors_at_ref_and_limit() {
    // Arrange: three writes on main → 3 commits. Tag after the 2nd write.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let _ = create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\";\nmessage User { string id = 1; }\n",
        "create",
    )
    .await;
    let add_email = c
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "k-email".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "user.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::AddField(
                        pb::ProtoAddField {
                            message_name: "User".into(),
                            field_name: "email".into(),
                            field_type: "string".into(),
                            field_number: 2,
                            repeated: false,
                            doc_comment: String::new(),
                        },
                    )),
                },
            )),
        })
        .await
        .expect("add email")
        .into_inner();
    let tag_commit = add_email.new_commit.clone();
    c.refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "v1".into(),
            target: Some(pb::VersionRef {
                r#ref: Some(pb::version_ref::Ref::Commit(tag_commit.clone())),
            }),
            message: String::new(),
        })
        .await
        .expect("create_tag");
    let _ = c
        .schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "k-phone".into(),
            force: false,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "user.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::AddField(
                        pb::ProtoAddField {
                            message_name: "User".into(),
                            field_name: "phone".into(),
                            field_type: "string".into(),
                            field_number: 3,
                            repeated: false,
                            doc_comment: String::new(),
                        },
                    )),
                },
            )),
        })
        .await
        .expect("add phone");

    // Act: log at the tag should see only the first two commits (3rd write is
    // after the tag); log on main with limit=1 should return exactly one entry.
    let at_tag = c
        .history
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "core".into(),
            at: Some(vref_tag("v1")),
            limit: 0,
        })
        .await
        .expect("log at tag")
        .into_inner();
    let limited = c
        .history
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "core".into(),
            at: None,
            limit: 1,
        })
        .await
        .expect("log limited")
        .into_inner();

    // Assert: tag walk stops at the tagged commit; limit truncates.
    assert_eq!(
        at_tag.entries.len(),
        2,
        "log at v1 should see 2 commits, got {}",
        at_tag.entries.len()
    );
    assert_eq!(
        at_tag.entries[0].commit_id, tag_commit,
        "newest at v1 must be the tagged commit"
    );
    assert_eq!(
        limited.entries.len(),
        1,
        "limit=1 should truncate to 1 entry"
    );
}

#[tokio::test]
async fn search_honors_at_ref() {
    // Arrange: create a schema with `Account` on main; tag; then rename Account
    // to Customer on main so Account no longer exists at HEAD.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let _ = create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "u.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\";\nmessage Account { string id = 1; }\n",
        "k-create",
    )
    .await;
    c.refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "snap".into(),
            target: Some(vref_branch("main")),
            message: String::new(),
        })
        .await
        .expect("create_tag");
    // Rename Account → Customer on main.
    c.schema
        .apply_mutation(pb::ApplyMutationRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "k-rename".into(),
            force: true,
            operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                pb::ProtobufMutation {
                    schema_path: "u.proto".into(),
                    operation: Some(pb::protobuf_mutation::Operation::RenameMessage(
                        pb::ProtoRenameMessage {
                            old_name: "Account".into(),
                            new_name: "Customer".into(),
                        },
                    )),
                },
            )),
        })
        .await
        .expect("rename");

    // Act: search for `Account` at the tag (pre-rename) and at main (post-rename).
    let at_tag = c
        .explore
        .search(pb::SearchRequest {
            query: "Account".into(),
            project: "acme".into(),
            repo: "core".into(),
            kind: 0,
            limit: 0,
            at: Some(vref_tag("snap")),
        })
        .await
        .expect("search at tag")
        .into_inner();
    let at_main = c
        .explore
        .search(pb::SearchRequest {
            query: "Account".into(),
            project: "acme".into(),
            repo: "core".into(),
            kind: 0,
            limit: 0,
            at: None,
        })
        .await
        .expect("search at main")
        .into_inner();

    // Assert: tag still has Account; main lost it to the rename.
    assert!(
        at_tag
            .results
            .iter()
            .any(|r| r.declaration.as_ref().is_some_and(|d| d.name == "Account")),
        "Account should be visible at the tag, got: {:?}",
        at_tag
            .results
            .iter()
            .filter_map(|r| r.declaration.as_ref().map(|d| &d.name))
            .collect::<Vec<_>>()
    );
    assert!(
        !at_main
            .results
            .iter()
            .any(|r| r.declaration.as_ref().is_some_and(|d| d.name == "Account")),
        "Account should NOT be visible on main after rename"
    );
}

// ── 10. Current OCC / tag-name semantics ─────────────────────────────────────

#[tokio::test]
async fn stale_base_revision_is_accepted_as_an_advisory_causal_commit() {
    // Arrange: create a schema on main and remember that initial commit as a
    // stale client base, then advance main once.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let initial = create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "occ-create",
    )
    .await
    .new_commit;
    c.schema
        .apply_mutation(add_field_request(
            "acme",
            "core",
            "main",
            "user.proto",
            "User",
            "email",
            3,
            "occ-advance-main",
        ))
        .await
        .expect("advance main");

    // Act: submit a second mutation with base_revision pinned to the old HEAD.
    let mut stale_request = add_field_request(
        "acme",
        "core",
        "main",
        "user.proto",
        "User",
        "phone",
        4,
        "occ-stale-write",
    );
    stale_request.base_revision = initial;
    let stale_write = c.schema.apply_mutation(stale_request).await;

    // Assert: the documented JJ contract accepts a retained stale base and
    // merges the write onto the branch's latest HEAD rather than CAS-rejecting.
    let stale_write = stale_write
        .expect("retained stale base_revision is advisory")
        .into_inner();
    assert!(
        !stale_write.new_commit.is_empty(),
        "accepted stale write should still return a commit"
    );
    let source = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        source.contains("email") && source.contains("phone"),
        "stale write should have landed on current HEAD; got:\n{source}"
    );
}

#[tokio::test]
async fn foreign_base_revision_is_rejected_before_mutation() {
    // Arrange
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
        "base-owned",
    )
    .await;
    let foreign = create_schema(
        &mut c.schema,
        "acme",
        "other",
        "main",
        "foreign.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\"; message Foreign { string id = 1; }",
        "base-foreign",
    )
    .await
    .new_commit;
    let mut request = add_field_request(
        "acme",
        "core",
        "main",
        "user.proto",
        "User",
        "phone",
        4,
        "foreign-base-write",
    );
    request.base_revision = foreign;

    // Act
    let result = c.schema.apply_mutation(request).await;

    // Assert
    assert_eq!(
        result.expect_err("foreign base must fail").code(),
        tonic::Code::FailedPrecondition
    );
}

#[tokio::test]
async fn duplicate_tag_creation_is_rejected_without_retargeting() {
    // Arrange: create a schema, tag the initial commit, then advance main.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let initial = create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        USER_PROTO,
        "tag-create",
    )
    .await
    .new_commit;
    let first_tag = c
        .refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "release".into(),
            target: Some(vref_commit(&initial)),
            message: String::new(),
        })
        .await
        .expect("create initial tag")
        .into_inner()
        .tag
        .expect("initial tag info");
    let advanced = c
        .schema
        .apply_mutation(add_field_request(
            "acme",
            "core",
            "main",
            "user.proto",
            "User",
            "email",
            3,
            "tag-advance-main",
        ))
        .await
        .expect("advance main after tag")
        .into_inner();

    // Act: create the same tag name again at the newer commit.
    let second_tag = c
        .refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "release".into(),
            target: Some(vref_commit(&advanced.new_commit)),
            message: String::new(),
        })
        .await;

    // Assert: tag names are immutable and the first target remains pinned.
    assert_eq!(
        second_tag.expect_err("duplicate tag name must fail").code(),
        tonic::Code::AlreadyExists
    );
    assert_eq!(first_tag.commit_hash, initial, "initial tag target changed");
    let tags = c
        .refs
        .list_tags(pb::ListTagsRequest {
            project: "acme".into(),
            repo: "core".into(),
            name_prefix: "release".into(),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect("list tags")
        .into_inner()
        .tags;
    assert_eq!(
        tags.len(),
        1,
        "tag namespace should contain one release tag"
    );
    assert_eq!(
        tags[0].commit_hash, initial,
        "release tag must remain pinned to the first target"
    );
    let source_at_tag = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_tag("release"),
    )
    .await;
    assert!(
        !source_at_tag.contains("email"),
        "immutable tag should resolve to the original schema; got:\n{source_at_tag}"
    );
}
