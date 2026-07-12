//! End-to-end suite driving the REAL in-process gRPC server (over a
//! `MemoryObjectDb`) through the generated clients. Covers granular protobuf
//! mutations, compatibility enforcement on protected bookmarks, multi-op /
//! multi-file transactions, first-class conflict render + resolve, codegen, and
//! imports / follow-type.
//!
//! Compatibility model under test (design.md §7): every repo defaults to `main`
//! protected + FULL. On a protected bookmark a breaking change is rejected
//! unless `force=true`; any bookmark whose name != `main` is unprotected and the
//! compat gate is skipped. Removing a field / crossing a field's wire type is
//! breaking under FULL; adding an OPTIONAL field is compatible.

mod common;

use common::*;
use schemahub_api::schemahub_v1 as pb;

const CLOUDBUILD_COMMON_PROTO: &str =
    include_str!("../../../tests/integration/cloudbuild_common.proto");
const CLOUDBUILD_USER_PROTO: &str =
    include_str!("../../../tests/integration/cloudbuild_user.proto");
const CLOUDBUILD_ORDER_PROTO: &str =
    include_str!("../../../tests/integration/cloudbuild_order.proto");
const LEGACY_ACCOUNT_PROTO: &str = include_str!("../../../tests/integration/legacy_account.proto");
const CLOUDBUILD_MONEY_FBS: &str = include_str!("../../../tests/integration/cloudbuild_money.fbs");
const CLOUDBUILD_SUPPLIER_FBS: &str =
    include_str!("../../../tests/integration/cloudbuild_supplier.fbs");
const CLOUDBUILD_CATALOG_FBS: &str =
    include_str!("../../../tests/integration/cloudbuild_catalog.fbs");

// ── Small builders for the op envelope ────────────────────────────────────────

/// Wrap a `protobuf_mutation::Operation` into the top-level `ApplyMutationRequest`
/// operation oneof for `schema_path`.
fn proto_op(
    schema_path: &str,
    op: pb::protobuf_mutation::Operation,
) -> pb::apply_mutation_request::Operation {
    pb::apply_mutation_request::Operation::ProtobufOp(pb::ProtobufMutation {
        schema_path: schema_path.into(),
        operation: Some(op),
    })
}

/// Wrap a `flat_buffers_mutation::Operation` into the top-level
/// `ApplyMutationRequest` operation oneof for `schema_path`.
fn fbs_op(
    schema_path: &str,
    op: pb::flat_buffers_mutation::Operation,
) -> pb::apply_mutation_request::Operation {
    pb::apply_mutation_request::Operation::FbsOp(pb::FlatBuffersMutation {
        schema_path: schema_path.into(),
        operation: Some(op),
    })
}

/// Wrap a `protobuf_mutation::Operation` into a `TransactionOp` for `schema_path`.
fn tx_proto_op(schema_path: &str, op: pb::protobuf_mutation::Operation) -> pb::TransactionOp {
    pb::TransactionOp {
        operation: Some(pb::transaction_op::Operation::ProtobufOp(
            pb::ProtobufMutation {
                schema_path: schema_path.into(),
                operation: Some(op),
            },
        )),
    }
}

fn add_field(message: &str, name: &str, ty: &str, number: u32) -> pb::protobuf_mutation::Operation {
    pb::protobuf_mutation::Operation::AddField(pb::ProtoAddField {
        message_name: message.into(),
        field_name: name.into(),
        field_type: ty.into(),
        field_number: number,
        repeated: false,
        doc_comment: String::new(),
    })
}

fn remove_field(message: &str, name: &str) -> pb::protobuf_mutation::Operation {
    pb::protobuf_mutation::Operation::RemoveField(pb::ProtoRemoveField {
        message_name: message.into(),
        field_name: name.into(),
    })
}

fn rename_field(message: &str, old: &str, new: &str) -> pb::protobuf_mutation::Operation {
    pb::protobuf_mutation::Operation::RenameField(pb::ProtoRenameField {
        message_name: message.into(),
        old_field_name: old.into(),
        new_field_name: new.into(),
    })
}

fn change_field_type(
    message: &str,
    name: &str,
    new_type: &str,
) -> pb::protobuf_mutation::Operation {
    pb::protobuf_mutation::Operation::ChangeFieldType(pb::ProtoChangeFieldType {
        message_name: message.into(),
        field_name: name.into(),
        new_type: new_type.into(),
    })
}

fn rename_message(old: &str, new: &str) -> pb::protobuf_mutation::Operation {
    pb::protobuf_mutation::Operation::RenameMessage(pb::ProtoRenameMessage {
        old_name: old.into(),
        new_name: new.into(),
    })
}

fn add_enum_value(en: &str, value: &str, number: i32) -> pb::protobuf_mutation::Operation {
    pb::protobuf_mutation::Operation::AddEnumValue(pb::ProtoAddEnumValue {
        enum_name: en.into(),
        value_name: value.into(),
        number,
        doc_comment: String::new(),
    })
}

/// Apply a single granular protobuf mutation; returns the RPC result so callers
/// can assert success *or* an error (e.g. a compat rejection).
async fn apply(
    c: &mut pb::schema_service_client::SchemaServiceClient<tonic::transport::Channel>,
    project: &str,
    repo: &str,
    branch: &str,
    idem: &str,
    force: bool,
    op: pb::apply_mutation_request::Operation,
) -> Result<pb::ApplyMutationResponse, tonic::Status> {
    c.apply_mutation(pb::ApplyMutationRequest {
        project: project.into(),
        repo: repo.into(),
        branch: branch.into(),
        base_revision: String::new(),
        idempotency_key: idem.into(),
        force,
        operation: Some(op),
    })
    .await
    .map(|r| r.into_inner())
}

/// Count the commits reachable from a branch ref by draining the `ListCommits`
/// server stream. Used to prove a transaction lands as exactly ONE commit on the
/// branch it targets (the `Log` RPC always walks the default branch, so it can't
/// observe commits on a non-default branch).
async fn count_commits(
    c: &mut pb::ref_service_client::RefServiceClient<tonic::transport::Channel>,
    project: &str,
    repo: &str,
    branch: &str,
) -> usize {
    let mut stream = c
        .list_commits(pb::ListCommitsRequest {
            project: project.into(),
            repo: repo.into(),
            from: Some(vref_branch(branch)),
            stop_at_commit: String::new(),
            schema_path: String::new(),
        })
        .await
        .expect("list_commits")
        .into_inner();
    let mut n = 0;
    while let Some(_commit) = stream.message().await.expect("list_commits stream") {
        n += 1;
    }
    n
}

// A base proto carrying a message and an enum, used by several tests.
const BASE_PROTO: &str = r#"syntax = "proto3";
package demo.v1;

message User {
  string id = 1;
  string name = 2;
}

enum Color {
  COLOR_UNSPECIFIED = 0;
  COLOR_RED = 1;
}
"#;

// ── 1. Granular protobuf mutations round-trip ─────────────────────────────────

#[tokio::test]
async fn granular_proto_mutations_round_trip_on_unprotected_branch() {
    // Arrange: a base schema on `main`, then a `dev` branch (unprotected, so the
    // compat gate never gets in the way of the breaking RemoveField below).
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE_PROTO,
        "create",
    )
    .await;
    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "dev".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create branch dev");

    // Act + Assert: AddField, then pull and see it.
    apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "k-add",
        false,
        proto_op("user.proto", add_field("User", "email", "string", 3)),
    )
    .await
    .expect("add field");
    let after_add = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(
        after_add.contains("email"),
        "AddField missing; got:\n{after_add}"
    );
    assert!(
        after_add.contains("= 3"),
        "field number missing; got:\n{after_add}"
    );

    // Act + Assert: RenameField email -> email_address.
    apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "k-rename-field",
        false,
        proto_op("user.proto", rename_field("User", "email", "email_address")),
    )
    .await
    .expect("rename field");
    let after_rename = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(
        after_rename.contains("email_address"),
        "RenameField missing; got:\n{after_rename}"
    );

    // Act + Assert: RemoveField name (field 2) — the server auto-reserves both
    // the number and the name (design.md §5.1, ProtoRemoveField).
    apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "k-remove",
        false,
        proto_op("user.proto", remove_field("User", "name")),
    )
    .await
    .expect("remove field");
    let after_remove = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(
        !after_remove.contains("string name ="),
        "removed field still present; got:\n{after_remove}"
    );
    assert!(
        after_remove.contains("reserved 2"),
        "RemoveField must reserve the number; got:\n{after_remove}"
    );
    assert!(
        after_remove.contains("reserved \"name\""),
        "RemoveField must reserve the name; got:\n{after_remove}"
    );

    // Act + Assert: RenameMessage User -> Account (same-file refs auto-updated).
    apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "k-rename-msg",
        false,
        proto_op("user.proto", rename_message("User", "Account")),
    )
    .await
    .expect("rename message");
    let decls = list_decl_names(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(
        decls.contains(&"Account".to_string()),
        "RenameMessage missing; decls: {decls:?}"
    );
    assert!(
        !decls.contains(&"User".to_string()),
        "old message name lingers; decls: {decls:?}"
    );

    // Act + Assert: AddEnumValue on Color.
    apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "k-enum",
        false,
        proto_op("user.proto", add_enum_value("Color", "COLOR_BLUE", 2)),
    )
    .await
    .expect("add enum value");
    let after_enum = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(
        after_enum.contains("COLOR_BLUE"),
        "AddEnumValue missing; got:\n{after_enum}"
    );
    assert!(
        after_enum.contains("= 2"),
        "enum value number missing; got:\n{after_enum}"
    );
}

// ── 2. Compat: protected `main` rejects breaking changes; force bypasses ──────

#[tokio::test]
async fn protected_main_allows_compatible_add_rejects_breaking_unless_forced() {
    // Arrange: schema on the protected default bookmark `main`.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE_PROTO,
        "create",
    )
    .await;

    // Act + Assert (a): adding an OPTIONAL field is FULL-compatible -> SUCCEEDS.
    apply(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "k-add-ok",
        false,
        proto_op("user.proto", add_field("User", "email", "string", 3)),
    )
    .await
    .expect("compatible add on protected main should succeed");

    // Act + Assert (b): removing a field is breaking under FULL; on protected
    // `main` without force the RPC must return an Err (FAILED_PRECONDITION).
    let rejected = apply(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "k-remove-blocked",
        false,
        proto_op("user.proto", remove_field("User", "name")),
    )
    .await;
    let err = rejected.expect_err("breaking change on protected main must be rejected");
    assert_eq!(
        err.code(),
        tonic::Code::FailedPrecondition,
        "compat rejection should be FAILED_PRECONDITION; got {err:?}"
    );

    // The rejected write must NOT have mutated the schema.
    let still_there = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        still_there.contains("string name ="),
        "rejected removal must leave the field intact; got:\n{still_there}"
    );

    // Act + Assert (c): the same removal with force=true bypasses the gate.
    apply(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "k-remove-forced",
        true,
        proto_op("user.proto", remove_field("User", "name")),
    )
    .await
    .expect("forced breaking change on protected main should succeed");
    let after_force = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        !after_force.contains("string name ="),
        "forced removal should drop the field; got:\n{after_force}"
    );
    assert!(
        after_force.contains("reserved \"name\""),
        "forced removal still reserves the name; got:\n{after_force}"
    );
}

#[tokio::test]
async fn protected_main_rejects_wire_incompatible_field_type_change() {
    // Arrange: schema on protected `main`. `id` is a string (length-delimited).
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE_PROTO,
        "create",
    )
    .await;

    // Act: changing string -> int32 crosses the wire type.
    let rejected = apply(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "k-change-type",
        false,
        proto_op("user.proto", change_field_type("User", "id", "int32")),
    )
    .await;

    // Assert: the write is rejected. A cross-wire-type ChangeFieldType is rejected
    // at MUTATION-VALIDATION time (the op itself is "always breaking"), so it
    // surfaces as INVALID_ARGUMENT — before the protected-bookmark compat gate is
    // even reached. (The FAILED_PRECONDITION compat path is exercised by the
    // RemoveField case in `protected_main_allows_compatible_add_rejects_breaking_unless_forced`.)
    let err = rejected.expect_err("cross-wire-type change must be rejected");
    assert_eq!(err.code(), tonic::Code::InvalidArgument, "got {err:?}");

    // And the schema is unchanged: `id` is still a string.
    let after = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        after.contains("string id ="),
        "rejected change must leave id a string; got:\n{after}"
    );
}

// ── 3. Compat: unprotected branch allows breaking changes ─────────────────────

#[tokio::test]
async fn unprotected_branch_allows_breaking_remove_field() {
    // Arrange: base on `main`, branch `dev` off it (unprotected).
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE_PROTO,
        "create",
    )
    .await;
    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "dev".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create branch dev");

    // Act: a breaking RemoveField on the unprotected branch, force=false.
    apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "k-remove-dev",
        false,
        proto_op("user.proto", remove_field("User", "name")),
    )
    .await
    .expect("compat is skipped on an unprotected branch");

    // Assert: the field is gone on `dev`, while `main` is untouched.
    let dev = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(
        !dev.contains("string name ="),
        "field should be removed on dev; got:\n{dev}"
    );
    let main = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        main.contains("string name ="),
        "main must be unchanged; got:\n{main}"
    );
}

#[tokio::test]
async fn protobuf_removed_field_reserves_name_and_number_at_server_boundary() {
    // Arrange: use an unprotected branch so the compatibility gate does not
    // reject the removal before the compiler can reserve the field identity.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE_PROTO,
        "reserve-base",
    )
    .await;
    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "dev".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create dev branch");
    apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "reserve-remove-name",
        false,
        proto_op("user.proto", remove_field("User", "name")),
    )
    .await
    .expect("remove field on dev");

    // Act: reusing the removed field number should fail because RemoveField
    // reserves it.
    let err = apply(
        &mut c.schema,
        "acme",
        "core",
        "dev",
        "reserve-reuse-number",
        false,
        proto_op("user.proto", add_field("User", "display_name", "string", 2)),
    )
    .await
    .expect_err("reserved field number should reject reuse");
    let pulled = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;

    // Assert.
    assert_eq!(err.code(), tonic::Code::InvalidArgument, "got {err:?}");
    assert!(
        err.message().contains("reserved"),
        "expected reserved-field error, got {err:?}"
    );
    assert!(
        pulled.contains("reserved 2"),
        "field number not reserved:\n{pulled}"
    );
    assert!(
        pulled.contains("reserved \"name\""),
        "field name not reserved:\n{pulled}"
    );
}

// ── 4. Multi-op + multi-file transactions ─────────────────────────────────────

#[tokio::test]
async fn transaction_two_ops_same_file_commits_atomically() {
    // Arrange: base on unprotected `dev` so the txn isn't compat-gated.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE_PROTO,
        "create",
    )
    .await;
    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "dev".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create branch dev");
    let commits_before = count_commits(&mut c.refs, "acme", "core", "dev").await;

    // Act: one transaction, two ops on the SAME file (add two fields).
    let resp = c
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "dev".into(),
            base_revision: String::new(),
            idempotency_key: "tx-same-file".into(),
            force: false,
            operations: vec![
                tx_proto_op("user.proto", add_field("User", "email", "string", 3)),
                tx_proto_op("user.proto", add_field("User", "phone", "string", 4)),
            ],
        })
        .await
        .expect("transaction (same file)")
        .into_inner();
    assert!(
        !resp.new_commit.is_empty(),
        "transaction should produce a commit"
    );

    // Assert: both effects are present, and exactly ONE new commit was created on
    // `dev` (the transaction commits atomically — both ops under one commit, not
    // one commit per op).
    let pulled = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(pulled.contains("email"), "txn op 1 missing; got:\n{pulled}");
    assert!(pulled.contains("phone"), "txn op 2 missing; got:\n{pulled}");
    let commits_after = count_commits(&mut c.refs, "acme", "core", "dev").await;
    assert_eq!(
        commits_after,
        commits_before + 1,
        "a transaction must commit atomically as exactly one commit (before={commits_before}, after={commits_after})"
    );
}

#[tokio::test]
async fn transaction_two_files_commits_atomically() {
    // Arrange: two proto files on unprotected `dev`.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "a.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\";\npackage demo.v1;\n\nmessage A {\n  string a1 = 1;\n}\n",
        "create-a",
    )
    .await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "b.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\";\npackage demo.v1;\n\nmessage B {\n  string b1 = 1;\n}\n",
        "create-b",
    )
    .await;
    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "dev".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create branch dev");

    // Act: one transaction touching TWO different files. The core groups ops by
    // file and commits all effects through `commit_write_multi` (one commit).
    let resp = c
        .schema
        .apply_transaction(pb::ApplyTransactionRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "dev".into(),
            base_revision: String::new(),
            idempotency_key: "tx-multi-file".into(),
            force: false,
            operations: vec![
                tx_proto_op("a.proto", add_field("A", "a2", "string", 2)),
                tx_proto_op("b.proto", add_field("B", "b2", "string", 2)),
            ],
        })
        .await
        .expect("multi-file transaction should commit");
    let resp = resp.into_inner();
    assert!(!resp.new_commit.is_empty());

    // Assert: both files reflect their respective op under the one commit.
    let a = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "a.proto",
        vref_branch("dev"),
    )
    .await;
    let b = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "b.proto",
        vref_branch("dev"),
    )
    .await;
    assert!(a.contains("a2"), "a.proto op missing; got:\n{a}");
    assert!(b.contains("b2"), "b.proto op missing; got:\n{b}");
}

// ── 5. First-class conflict render + resolve ──────────────────────────────────

#[tokio::test]
async fn merge_diverging_decls_produces_first_class_conflict_then_resolve() {
    // Arrange: base on `main`, then two unprotected branches `a` and `b`.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        BASE_PROTO,
        "create",
    )
    .await;
    for name in ["a", "b"] {
        c.refs
            .create_branch(pb::CreateBranchRequest {
                project: "acme".into(),
                repo: "core".into(),
                name: name.into(),
                from: Some(vref_branch("main")),
            })
            .await
            .unwrap_or_else(|e| panic!("create branch {name}: {e}"));
    }

    // Each branch edits the SAME declaration (User) DIFFERENTLY: same field
    // number 3, but different name + type, so the stored decl blob diverges.
    apply(
        &mut c.schema,
        "acme",
        "core",
        "a",
        "edit-a",
        false,
        proto_op("user.proto", add_field("User", "email", "string", 3)),
    )
    .await
    .expect("edit on branch a");
    apply(
        &mut c.schema,
        "acme",
        "core",
        "b",
        "edit-b",
        false,
        proto_op("user.proto", add_field("User", "age", "int32", 3)),
    )
    .await
    .expect("edit on branch b");

    // Act: merge `b` into `a`. jj produces a first-class conflict on the
    // diverging decl rather than failing (design.md §6).
    c.refs
        .merge(pb::MergeRequest {
            project: "acme".into(),
            repo: "core".into(),
            source_branch: "b".into(),
            target_branch: "a".into(),
            base_revision: String::new(),
            idempotency_key: "merge-b-into-a".into(),
            message: "merge b into a".into(),
        })
        .await
        .expect("merge should not error even with a conflict");

    // Assert: `RenderConflict` surfaces the competing sides for User on `a`.
    // (MergeResponse carries only new_commit, so the conflict is observed here.)
    let rendered = c
        .history
        .render_conflict(pb::RenderConflictRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "user.proto".into(),
            declaration_name: "User".into(),
            at: Some(vref_branch("a")),
        })
        .await
        .expect("render_conflict must report the conflict")
        .into_inner();
    assert!(
        !rendered.rendered.is_empty(),
        "render_conflict should return non-empty content for a conflicted decl"
    );

    // Act + Assert: resolve the conflict by submitting a clean User definition.
    let resolved_source = r#"syntax = "proto3";
package demo.v1;

message User {
  string id = 1;
  string name = 2;
  string email = 3;
}
"#;
    let resolve = c
        .history
        .resolve_conflict(pb::ResolveConflictRequest {
            project: "acme".into(),
            repo: "core".into(),
            bookmark: "a".into(),
            schema_path: "user.proto".into(),
            declaration_name: "User".into(),
            resolved_source: resolved_source.into(),
            author: "tester".into(),
            message: "resolve User conflict".into(),
        })
        .await
        .expect("resolve_conflict")
        .into_inner();
    assert!(
        !resolve.new_commit.is_empty(),
        "resolution should produce a commit"
    );

    // The decl is no longer conflicted: render_conflict now returns
    // FAILED_PRECONDITION (NotConflicted), and the resolved field is present.
    let after = c
        .history
        .render_conflict(pb::RenderConflictRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "user.proto".into(),
            declaration_name: "User".into(),
            at: Some(vref_branch("a")),
        })
        .await;
    let err = after.expect_err("a resolved decl is no longer conflicted");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition, "got {err:?}");
    let resolved_pull = pull_source(
        &mut c.explore,
        "acme",
        "core",
        "user.proto",
        vref_branch("a"),
    )
    .await;
    assert!(
        resolved_pull.contains("email"),
        "resolved field missing; got:\n{resolved_pull}"
    );
}

// ── 6. Codegen ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn codegen_descriptors_and_preview_rust() {
    // Arrange: a self-contained proto with a message.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let src = "syntax = \"proto3\";\npackage demo.v1;\n\nmessage Ping {\n  string msg = 1;\n}\n";
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "ping.proto",
        pb::SchemaFormat::Protobuf,
        src,
        "create",
    )
    .await;

    // Act + Assert: GetDescriptors returns a non-empty descriptor artifact.
    let descriptors = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "ping.proto".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("get_descriptors")
        .into_inner();
    assert!(
        !descriptors.descriptor_bytes.is_empty(),
        "descriptor bytes should be non-empty"
    );
    assert_eq!(descriptors.format, pb::SchemaFormat::Protobuf as i32);

    // Act + Assert: PreviewCodegen for Rust returns source mentioning the message.
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "ping.proto".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
            rust_pluggable_buffer: false,
        })
        .await
        .expect("preview_codegen")
        .into_inner();
    let code = String::from_utf8(preview.content).expect("rust code is utf-8");
    assert!(!code.is_empty(), "generated rust should be non-empty");
    assert!(
        code.contains("Ping"),
        "generated rust should mention the message; got:\n{code}"
    );
}

// ── 7. Imports + follow_type ──────────────────────────────────────────────────

#[tokio::test]
async fn follow_type_resolves_imported_message() {
    // Arrange: base.proto defines Address; user.proto imports it and references
    // it from User.addr.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "base.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\";\npackage demo.v1;\n\nmessage Address {\n  string city = 1;\n}\n",
        "create-base",
    )
    .await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\";\npackage demo.v1;\nimport \"base.proto\";\n\nmessage User {\n  Address addr = 1;\n}\n",
        "create-user",
    )
    .await;

    // Act: follow the type of User.addr.
    let resp = c
        .explore
        .follow_type(pb::FollowTypeRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "user.proto".into(),
            declaration_name: "User".into(),
            field_name: "addr".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("follow_type")
        .into_inner();

    // Assert: it resolves to the imported base.proto (in the same project/repo).
    // NOTE: the server's FollowType returns only the resolved location; summary /
    // detail are intentionally empty in v2, so we assert on the resolved path.
    assert_eq!(
        resp.resolved_schema_path, "base.proto",
        "should resolve to the import"
    );
    assert_eq!(resp.resolved_project, "acme");
    assert_eq!(resp.resolved_repo, "core");

    // And base.proto really does define Address (sanity check on the target).
    let base_decls = list_decl_names(
        &mut c.explore,
        "acme",
        "core",
        "base.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        base_decls.contains(&"Address".to_string()),
        "target decl missing; got: {base_decls:?}"
    );
}

#[tokio::test]
async fn protobuf_cloud_build_closure_and_tagged_descriptors_are_stable() {
    // Arrange: a root schema imports user/profile.proto, which imports
    // common/types.proto. This mirrors cloud build descriptor generation: the
    // root artifact must include the whole transitive closure, not just the
    // root file.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "common/types.proto",
        pb::SchemaFormat::Protobuf,
        CLOUDBUILD_COMMON_PROTO,
        "cloud-pb-common",
    )
    .await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "user/profile.proto",
        pb::SchemaFormat::Protobuf,
        CLOUDBUILD_USER_PROTO,
        "cloud-pb-user",
    )
    .await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "orders/purchase.proto",
        pb::SchemaFormat::Protobuf,
        CLOUDBUILD_ORDER_PROTO,
        "cloud-pb-order",
    )
    .await;

    // Act: direct dependencies should only include the immediate import.
    let direct = c
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "orders/purchase.proto".into(),
            at: Some(vref_branch("main")),
            transitive: false,
        })
        .await
        .expect("list direct dependencies")
        .into_inner()
        .dependencies;
    let direct_paths: Vec<String> = direct.into_iter().map(|d| d.imported_schema).collect();

    // Assert.
    assert_eq!(
        direct_paths,
        vec!["acme/core/user/profile.proto".to_string()],
        "direct dependency set changed"
    );

    // Act: transitive dependencies should include the nested common schema.
    let transitive = c
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "orders/purchase.proto".into(),
            at: Some(vref_branch("main")),
            transitive: true,
        })
        .await
        .expect("list transitive dependencies")
        .into_inner()
        .dependencies;
    let transitive_paths: Vec<String> = transitive.into_iter().map(|d| d.imported_schema).collect();

    // Assert.
    assert!(
        transitive_paths.contains(&"acme/core/user/profile.proto".to_string()),
        "missing direct import in transitive set: {transitive_paths:?}"
    );
    assert!(
        transitive_paths.contains(&"acme/core/common/types.proto".to_string()),
        "missing nested import in transitive set: {transitive_paths:?}"
    );

    // Arrange: pin the current repo state with a tag, then mutate main.
    c.refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "core".into(),
            name: "build-2026-06-02".into(),
            target: Some(vref_branch("main")),
            message: "cloud build input".into(),
        })
        .await
        .expect("create build tag");

    let tag_before = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "orders/purchase.proto".into(),
            at: Some(vref_tag("build-2026-06-02")),
        })
        .await
        .expect("tag descriptors before")
        .into_inner();
    assert!(
        !tag_before.descriptor_bytes.is_empty(),
        "tag descriptor should be non-empty"
    );
    assert!(
        !tag_before.at_commit.is_empty(),
        "tag descriptor should report resolved commit"
    );
    let commit_before = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "orders/purchase.proto".into(),
            at: Some(vref_commit(&tag_before.at_commit)),
        })
        .await
        .expect("commit descriptors before")
        .into_inner();
    assert_eq!(
        tag_before.descriptor_bytes, commit_before.descriptor_bytes,
        "tag and its resolved commit should produce identical descriptors"
    );

    apply(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "cloud-pb-order-add-build-id",
        false,
        proto_op(
            "orders/purchase.proto",
            add_field("PurchaseOrder", "build_id", "string", 4),
        ),
    )
    .await
    .expect("compatible add field on main");

    // Act: resolving the same tag after main moved must return identical bytes.
    let tag_after = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "orders/purchase.proto".into(),
            at: Some(vref_tag("build-2026-06-02")),
        })
        .await
        .expect("tag descriptors after")
        .into_inner();
    let main_after = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "orders/purchase.proto".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("main descriptors after")
        .into_inner();

    // Assert.
    assert_eq!(
        tag_before.descriptor_bytes, tag_after.descriptor_bytes,
        "tag-pinned descriptor bytes changed after main moved"
    );
    assert_eq!(
        tag_before.at_commit, tag_after.at_commit,
        "tag resolved to a different commit after main moved"
    );
    assert_ne!(
        tag_after.descriptor_bytes, main_after.descriptor_bytes,
        "main descriptor should reflect the new field while the tag stays pinned"
    );
}

#[tokio::test]
async fn flatbuffers_cloud_build_include_closure_is_in_descriptor_bundle() {
    // Arrange: catalog.fbs includes supplier.fbs, which includes money.fbs.
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "common/money.fbs",
        pb::SchemaFormat::Flatbuffers,
        CLOUDBUILD_MONEY_FBS,
        "cloud-fbs-money",
    )
    .await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "supplier/supplier.fbs",
        pb::SchemaFormat::Flatbuffers,
        CLOUDBUILD_SUPPLIER_FBS,
        "cloud-fbs-supplier",
    )
    .await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "catalog/sku.fbs",
        pb::SchemaFormat::Flatbuffers,
        CLOUDBUILD_CATALOG_FBS,
        "cloud-fbs-catalog",
    )
    .await;

    // Act.
    let direct = c
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "catalog/sku.fbs".into(),
            at: Some(vref_branch("main")),
            transitive: false,
        })
        .await
        .expect("list direct fbs dependencies")
        .into_inner()
        .dependencies;
    let transitive = c
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "catalog/sku.fbs".into(),
            at: Some(vref_branch("main")),
            transitive: true,
        })
        .await
        .expect("list transitive fbs dependencies")
        .into_inner()
        .dependencies;
    let descriptors = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "catalog/sku.fbs".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("get fbs descriptors")
        .into_inner();
    let bundle = String::from_utf8(descriptors.descriptor_bytes).expect("fbs bundle is utf-8");

    // Assert: direct vs transitive dependency sets differ.
    let direct_paths: Vec<String> = direct.into_iter().map(|d| d.imported_schema).collect();
    assert_eq!(
        direct_paths,
        vec!["acme/core/supplier/supplier.fbs".to_string()],
        "direct fbs dependency set changed"
    );
    let transitive_paths: Vec<String> = transitive.into_iter().map(|d| d.imported_schema).collect();
    assert!(
        transitive_paths.contains(&"acme/core/supplier/supplier.fbs".to_string()),
        "missing direct fbs import in transitive set: {transitive_paths:?}"
    );
    assert!(
        transitive_paths.contains(&"acme/core/common/money.fbs".to_string()),
        "missing nested fbs import in transitive set: {transitive_paths:?}"
    );

    // Assert: descriptor bundle contains every schema in the closure.
    for needle in [
        "catalog/sku.fbs",
        "supplier/supplier.fbs",
        "common/money.fbs",
        "table Sku",
        "table Supplier",
        "table Money",
        "struct Decimal",
    ] {
        assert!(bundle.contains(needle), "bundle lost `{needle}`:\n{bundle}");
    }
}

#[tokio::test]
async fn flatbuffers_preview_codegen_rust_returns_build_artifact() {
    // Arrange: keep this schema deliberately simple because this test is a
    // server/codegen smoke, while richer FlatBuffers AST features are covered by
    // round-trip and descriptor-bundle tests.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let src = r#"namespace build.output;

table BuildItem {
  id: string;
  count: int;
}

root_type BuildItem;
"#;
    create_schema(
        &mut c.schema,
        "build",
        "artifacts",
        "main",
        "build_item.fbs",
        pb::SchemaFormat::Flatbuffers,
        src,
        "fbs-codegen-preview",
    )
    .await;

    // Act.
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "build".into(),
            repo: "artifacts".into(),
            schema_path: "build_item.fbs".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
            rust_pluggable_buffer: false,
        })
        .await
        .expect("preview flatbuffers rust codegen")
        .into_inner();
    let code = String::from_utf8(preview.content).expect("generated rust is utf-8");

    // Assert.
    assert!(
        !preview.is_archive,
        "single-file preview should not be archive"
    );
    assert!(
        !preview.at_commit.is_empty(),
        "preview should report resolved commit"
    );
    assert!(
        code.contains("BuildItem"),
        "generated rust should mention BuildItem; got:\n{code}"
    );
}

#[tokio::test]
async fn flatbuffers_table_mutations_round_trip_and_struct_mutation_is_rejected() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let src = r#"namespace inventory;

struct Vec2 {
  x: float;
  y: float;
}

table Item {
  id: string (key);
  old_rank: int;
  position: Vec2;
}

root_type Item;
"#;
    create_schema(
        &mut c.schema,
        "warehouse",
        "catalog",
        "main",
        "item.fbs",
        pb::SchemaFormat::Flatbuffers,
        src,
        "fbs-mutation-base",
    )
    .await;

    // Act: table field add, rename, and deprecate are accepted.
    apply(
        &mut c.schema,
        "warehouse",
        "catalog",
        "main",
        "fbs-add-field",
        false,
        fbs_op(
            "item.fbs",
            pb::flat_buffers_mutation::Operation::AddField(pb::FbsAddField {
                table_name: "Item".into(),
                field_name: "price_cents".into(),
                field_type: "int".into(),
                default_value: "0".into(),
                doc_comment: String::new(),
            }),
        ),
    )
    .await
    .expect("add fbs table field");
    apply(
        &mut c.schema,
        "warehouse",
        "catalog",
        "main",
        "fbs-rename-field",
        false,
        fbs_op(
            "item.fbs",
            pb::flat_buffers_mutation::Operation::RenameField(pb::FbsRenameField {
                table_name: "Item".into(),
                old_field_name: "old_rank".into(),
                new_field_name: "legacy_rank".into(),
            }),
        ),
    )
    .await
    .expect("rename fbs table field");
    apply(
        &mut c.schema,
        "warehouse",
        "catalog",
        "main",
        "fbs-deprecate-field",
        false,
        fbs_op(
            "item.fbs",
            pb::flat_buffers_mutation::Operation::DeprecateField(pb::FbsDeprecateField {
                table_name: "Item".into(),
                field_name: "legacy_rank".into(),
            }),
        ),
    )
    .await
    .expect("deprecate fbs table field");
    let pulled = pull_source(
        &mut c.explore,
        "warehouse",
        "catalog",
        "item.fbs",
        vref_branch("main"),
    )
    .await;

    // Assert.
    assert!(
        pulled.contains("price_cents: int = 0"),
        "added field missing:\n{pulled}"
    );
    assert!(
        pulled.contains("legacy_rank: int (deprecated)"),
        "renamed/deprecated field missing:\n{pulled}"
    );
    assert!(
        !pulled.contains("old_rank"),
        "old field name should be gone:\n{pulled}"
    );

    // Act: structs are immutable; adding a field to Vec2 must fail.
    let err = apply(
        &mut c.schema,
        "warehouse",
        "catalog",
        "main",
        "fbs-struct-add-rejected",
        false,
        fbs_op(
            "item.fbs",
            pb::flat_buffers_mutation::Operation::AddField(pb::FbsAddField {
                table_name: "Vec2".into(),
                field_name: "z".into(),
                field_type: "float".into(),
                default_value: String::new(),
                doc_comment: String::new(),
            }),
        ),
    )
    .await
    .expect_err("struct mutation should be rejected");

    // Assert.
    assert_eq!(err.code(), tonic::Code::InvalidArgument, "got {err:?}");
    assert!(
        err.message().contains("struct"),
        "error should explain struct immutability, got {err:?}"
    );
}

#[tokio::test]
async fn flatbuffers_union_member_mutation_is_explicitly_unimplemented_at_server_boundary() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let src = r#"namespace inventory;

table Item {
  id: string;
}

table Bundle {
  id: string;
}

union Entity {
  Item
}

root_type Item;
"#;
    create_schema(
        &mut c.schema,
        "warehouse",
        "catalog",
        "main",
        "entity.fbs",
        pb::SchemaFormat::Flatbuffers,
        src,
        "fbs-union-base",
    )
    .await;

    // Act.
    let err = apply(
        &mut c.schema,
        "warehouse",
        "catalog",
        "main",
        "fbs-union-add-member",
        false,
        fbs_op(
            "entity.fbs",
            pb::flat_buffers_mutation::Operation::AddUnionMember(pb::FbsAddUnionMember {
                union_name: "Entity".into(),
                member_type: "Bundle".into(),
            }),
        ),
    )
    .await
    .expect_err("union member mutation should be unimplemented");

    // Assert.
    assert_eq!(err.code(), tonic::Code::Unimplemented, "got {err:?}");
    assert!(
        err.message().contains("union member add/remove"),
        "unexpected unimplemented message: {err:?}"
    );
}

#[tokio::test]
async fn proto2_fixture_round_trips_through_server() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;

    // Act.
    create_schema(
        &mut c.schema,
        "legacy",
        "accounts",
        "main",
        "legacy_account.proto",
        pb::SchemaFormat::Protobuf,
        LEGACY_ACCOUNT_PROTO,
        "legacy-proto2-create",
    )
    .await;
    let pulled = pull_source(
        &mut c.explore,
        "legacy",
        "accounts",
        "legacy_account.proto",
        vref_branch("main"),
    )
    .await;
    let names = list_decl_names(
        &mut c.explore,
        "legacy",
        "accounts",
        "legacy_account.proto",
        vref_branch("main"),
    )
    .await;

    // Assert.
    for needle in [
        "syntax = \"proto2\"",
        "required string id",
        "optional string email",
        "default = \"unknown@example.com\"",
        "extensions 100 to 536870911",
        "option allow_alias = true",
        "ENABLED = 1",
        "extend LegacyAccount",
        "external_reference",
        "service LegacyAccountService",
    ] {
        assert!(
            pulled.contains(needle),
            "proto2 output lost `{needle}`:\n{pulled}"
        );
    }
    assert!(
        names.contains(&"LegacyAccount".to_string()),
        "message declaration missing: {names:?}"
    );
    assert!(
        names.contains(&"LegacyAccountService".to_string()),
        "service declaration missing: {names:?}"
    );
}
