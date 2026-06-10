//! Core orchestration tests (AAA style) over a `MemoryObjectDb`-backed `Jj`
//! and the real `schemahub-compiler-protobuf` (a dev-dependency used only to
//! produce real op envelopes / parse real `.proto` — core itself stays
//! format-agnostic).

use std::sync::Arc;

use bytes::Bytes;
use schemahub_compiler_protobuf::{
    OpAddField, OpChangeCardinality, OpCreateMessage, ProtoOp, ProtobufCompiler,
};
use schemahub_jj::{Jj, MemoryObjectDb, RefSpec};
use schemahub_types::{
    Compiler, DeclKind, Mutation, MutationEffect, NoopAuthn, NoopAuthz, SchemaObjects, SchemaPath,
};

use crate::config::{RepoConfig, RepoConfigStore};
use crate::request::{MutationRequest, TransactionRequest};
use crate::{CompilerRegistry, Core, CoreError};

// ── Helpers ──────────────────────────────────────────────────────────────────

const PROJECT: &str = "p";
const REPO: &str = "r";
const SCHEMA: &str = "user.proto";

fn schema_path() -> SchemaPath {
    SchemaPath::new(PROJECT, REPO, SCHEMA)
}

/// A Core wired to an in-memory JJ, the protobuf compiler, Noop auth, and the
/// given repo config store.
fn core_with_config(configs: RepoConfigStore) -> Core {
    // Arrange the registry with the real protobuf compiler.
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    let jj = Arc::new(Jj::new(Arc::new(MemoryObjectDb::new())));
    Core::with_config(
        jj,
        registry,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        configs,
    )
}

/// Default Core: `main` protected, FULL compatibility.
fn core() -> Core {
    core_with_config(RepoConfigStore::new())
}

fn proto_mutation(op: ProtoOp) -> Mutation {
    Mutation {
        schema_path: schema_path(),
        format_id: "protobuf".to_string(),
        operation: Bytes::from(op.encode()),
    }
}

/// Build a `MutationRequest` for the given op on `bookmark`.
fn request(bookmark: &str, op: ProtoOp) -> MutationRequest {
    MutationRequest {
        bookmark: bookmark.to_string(),
        mutation: proto_mutation(op),
        author: "alice".to_string(),
        message: "test".to_string(),
        force: false,
        idempotency_key: None,
        token: None,
    }
}

/// Seed an initial `message User { int32 id = 1; }` on `main` directly through
/// the JJ layer, so flow tests start from a known clean state. Returns the commit id.
fn seed_user_message(core: &Core) -> String {
    // Arrange: parse a real .proto into per-decl objects and commit them.
    let compiler = ProtobufCompiler::new();
    let parsed = compiler
        .parse("syntax=\"proto3\";\nmessage User { int32 id = 1; }\n")
        .expect("parse");
    let effect = MutationEffect {
        meta: Some(parsed.meta),
        upserts: parsed.decls,
        removes: vec![],
    };
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "main",
            SCHEMA,
            &RefSpec::bookmark("main"),
            effect,
            "seed",
            "seed",
        )
        .expect("seed commit")
        .commit_id
}

// ── Mutation flow: load → apply → commit ──────────────────────────────────────

#[test]
fn apply_mutation_creates_message_on_fresh_bookmark() {
    // Arrange: an empty repo; create a message on a feature bookmark.
    let core = core();
    let req = request(
        "feature/x",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "Order".into(),
        }),
    );

    // Act
    let resp = core.apply_mutation(req).expect("mutation applies");

    // Assert: a commit landed cleanly and the declaration is visible.
    assert!(!resp.commit_id.is_empty());
    assert!(resp.conflicted_decls.is_empty());
    let decls = core
        .list_declarations(&schema_path(), &RefSpec::bookmark("feature/x"), None)
        .expect("list");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].name, "Order");
    assert_eq!(decls[0].kind, DeclKind::Message);
}

#[test]
fn apply_mutation_add_field_on_unprotected_bookmark_succeeds() {
    // Arrange: seed a message, branch off, add a field on the feature bookmark.
    let core = core();
    seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "feature/y",
        &RefSpec::bookmark("main"),
        "alice",
        None,
    )
    .expect("branch");
    let req = request(
        "feature/y",
        ProtoOp::AddField(OpAddField {
            message_name: "User".into(),
            field_name: "email".into(),
            field_type: "string".into(),
            field_number: 2,
            cardinality: String::new(),
        }),
    );

    // Act
    let resp = core.apply_mutation(req).expect("add field");

    // Assert
    assert!(resp.conflicted_decls.is_empty());
    let source = core
        .get_schema_source(&schema_path(), &RefSpec::bookmark("feature/y"), None)
        .expect("print");
    assert!(source.contains("string email = 2"), "got:\n{source}");
}

// ── Protected-bookmark compatibility rejection ─────────────────────────────────

#[test]
fn breaking_change_on_protected_bookmark_is_rejected() {
    // Arrange: default config protects `main` with FULL compatibility. Seed a
    // message, then attempt a breaking cardinality change on `main`.
    let core = core();
    seed_user_message(&core);
    let req = request(
        "main",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "User".into(),
            field_name: "id".into(),
            new_cardinality: "repeated".into(),
        }),
    );

    // Act
    let result = core.apply_mutation(req);

    // Assert: blocked with collected violations; nothing committed.
    match result {
        Err(CoreError::Incompatible(violations)) => {
            assert!(!violations.is_empty());
            assert_eq!(violations[0].declaration_name, "User");
        }
        other => panic!("expected Incompatible, got {other:?}"),
    }
}

#[test]
fn breaking_change_on_unprotected_bookmark_is_allowed() {
    // Arrange: same breaking change but on a feature bookmark (not protected).
    let core = core();
    seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "feature/z",
        &RefSpec::bookmark("main"),
        "alice",
        None,
    )
    .expect("branch");
    let req = request(
        "feature/z",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "User".into(),
            field_name: "id".into(),
            new_cardinality: "repeated".into(),
        }),
    );

    // Act
    let resp = core.apply_mutation(req);

    // Assert: the unprotected bookmark skips the gate and commits.
    assert!(resp.is_ok(), "unprotected write should pass: {resp:?}");
}

#[test]
fn force_bypasses_compat_gate_on_protected_bookmark() {
    // Arrange: a breaking change on `main`, but with force=true.
    let core = core();
    seed_user_message(&core);
    let mut req = request(
        "main",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "User".into(),
            field_name: "id".into(),
            new_cardinality: "repeated".into(),
        }),
    );
    req.force = true;

    // Act
    let resp = core.apply_mutation(req);

    // Assert: force skips the gate.
    assert!(resp.is_ok(), "force should bypass gate: {resp:?}");
}

#[test]
fn disabled_compatibility_allows_breaking_change_on_protected_bookmark() {
    // Arrange: a repo whose protected `main` has compatibility Disabled.
    let mut configs = RepoConfigStore::new();
    configs.set(
        PROJECT,
        REPO,
        RepoConfig {
            default_bookmark: "main".to_string(),
            compatibility_direction: schemahub_types::CompatibilityDirection::Disabled,
            protected_bookmarks: vec!["main".to_string()],
        },
    );
    let core = core_with_config(configs);
    seed_user_message(&core);
    let req = request(
        "main",
        ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: "User".into(),
            field_name: "id".into(),
            new_cardinality: "repeated".into(),
        }),
    );

    // Act
    let resp = core.apply_mutation(req);

    // Assert
    assert!(resp.is_ok(), "disabled compat should allow: {resp:?}");
}

// ── Idempotency dedupe ─────────────────────────────────────────────────────────

#[test]
fn idempotent_retry_returns_stored_result_without_reapplying() {
    // Arrange: a create-message mutation with an idempotency key.
    let core = core();
    let mut req = request(
        "feature/idem",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "Once".into(),
        }),
    );
    req.idempotency_key = Some("key-123".to_string());

    // Act: apply twice with the same key.
    let first = core.apply_mutation(req.clone()).expect("first");
    let second = core.apply_mutation(req).expect("retry");

    // Assert: identical response, and only ONE declaration exists (not applied
    // twice into two commits).
    assert_eq!(first, second);
    let decls = core
        .list_declarations(&schema_path(), &RefSpec::bookmark("feature/idem"), None)
        .expect("list");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].name, "Once");
}

// ── Exploration read ───────────────────────────────────────────────────────────

#[test]
fn list_and_get_declaration_round_trip_through_compiler() {
    // Arrange
    let core = core();
    seed_user_message(&core);

    // Act
    let summaries = core
        .list_declarations(&schema_path(), &RefSpec::bookmark("main"), None)
        .expect("list");
    let detail = core
        .get_declaration(&schema_path(), &RefSpec::bookmark("main"), "User", None)
        .expect("detail");

    // Assert
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "User");
    let text = String::from_utf8(detail.as_bytes().to_vec()).expect("utf8");
    assert!(text.contains("User"), "detail should mention User:\n{text}");
}

#[test]
fn search_finds_declaration_by_name_substring() {
    // Arrange
    let core = core();
    seed_user_message(&core);

    // Act
    let hits = core
        .search(PROJECT, REPO, &RefSpec::bookmark("main"), "use", None)
        .expect("search");

    // Assert
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].schema_name, SCHEMA);
    assert_eq!(hits[0].decl_name, "User");
}

#[test]
fn list_schemas_returns_committed_files() {
    // Arrange
    let core = core();
    seed_user_message(&core);

    // Act
    let schemas = core
        .list_schemas(PROJECT, REPO, &RefSpec::bookmark("main"), None)
        .expect("schemas");

    // Assert
    assert_eq!(schemas, vec![SCHEMA.to_string()]);
}

// ── Transaction flow ───────────────────────────────────────────────────────────

#[test]
fn transaction_applies_batch_under_one_commit() {
    // Arrange: create a message then add a field, in one transaction, on a
    // fresh feature bookmark.
    let core = core();
    let ops = vec![
        proto_mutation(ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "Account".into(),
        })),
        proto_mutation(ProtoOp::AddField(OpAddField {
            message_name: "Account".into(),
            field_name: "balance".into(),
            field_type: "int64".into(),
            field_number: 1,
            cardinality: String::new(),
        })),
    ];
    let req = TransactionRequest {
        bookmark: "feature/tx".to_string(),
        mutations: ops,
        author: "alice".to_string(),
        message: "tx".to_string(),
        force: false,
        idempotency_key: None,
        token: None,
    };

    // Act
    let resp = core.apply_mutations(req).expect("transaction");

    // Assert: one commit, both effects present.
    assert!(resp.conflicted_decls.is_empty());
    let source = core
        .get_schema_source(&schema_path(), &RefSpec::bookmark("feature/tx"), None)
        .expect("print");
    assert!(source.contains("message Account"), "got:\n{source}");
    assert!(source.contains("int64 balance = 1"), "got:\n{source}");
}

#[test]
fn empty_transaction_is_rejected() {
    // Arrange
    let core = core();
    let req = TransactionRequest {
        bookmark: "main".to_string(),
        mutations: vec![],
        author: "alice".to_string(),
        message: "tx".to_string(),
        force: false,
        idempotency_key: None,
        token: None,
    };

    // Act
    let result = core.apply_mutations(req);

    // Assert
    assert!(matches!(result, Err(CoreError::EmptyTransaction)));
}

#[test]
fn multi_file_transaction_commits_both_files_in_one_commit() {
    // Arrange: two ops targeting different schema files in the same repo.
    let core = core();
    let proto_op_in = |schema: &str, name: &str| Mutation {
        schema_path: SchemaPath::new(PROJECT, REPO, schema),
        format_id: "protobuf".to_string(),
        operation: Bytes::from(
            ProtoOp::CreateMessage(OpCreateMessage {
                message_name: name.into(),
            })
            .encode(),
        ),
    };
    let req = TransactionRequest {
        bookmark: "feature/multi".to_string(),
        mutations: vec![proto_op_in("a.proto", "A"), proto_op_in("b.proto", "B")],
        author: "alice".to_string(),
        message: "tx".to_string(),
        force: false,
        idempotency_key: None,
        token: None,
    };

    // Act: a single transaction touching two files.
    let resp = core.apply_mutations(req).expect("multi-file transaction");

    // Assert: one commit landed, and BOTH files are present at that commit,
    // each with its declaration — i.e. the write was atomic.
    assert!(!resp.commit_id.is_empty());
    assert!(resp.conflicted_decls.is_empty());

    let at = RefSpec::commit(resp.commit_id.clone());
    let schemas = core
        .jj()
        .list_schemas(PROJECT, REPO, &at)
        .expect("list schemas at the new commit");
    assert_eq!(schemas, vec!["a.proto".to_string(), "b.proto".to_string()]);

    let a = core
        .list_declarations(
            &SchemaPath::new(PROJECT, REPO, "a.proto"),
            &RefSpec::bookmark("feature/multi"),
            None,
        )
        .expect("list a.proto");
    let b = core
        .list_declarations(
            &SchemaPath::new(PROJECT, REPO, "b.proto"),
            &RefSpec::bookmark("feature/multi"),
            None,
        )
        .expect("list b.proto");
    assert_eq!(
        a.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        vec!["A"]
    );
    assert_eq!(
        b.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        vec!["B"]
    );
}

#[test]
fn transaction_mixing_formats_is_rejected() {
    // Arrange: two ops with different format_ids in one batch.
    let core = core();
    let proto = proto_mutation(ProtoOp::CreateMessage(OpCreateMessage {
        message_name: "A".into(),
    }));
    let mut other = proto_mutation(ProtoOp::CreateMessage(OpCreateMessage {
        message_name: "B".into(),
    }));
    other.format_id = "flatbuffers".to_string();
    let req = TransactionRequest {
        bookmark: "feature/mixed".to_string(),
        mutations: vec![proto, other],
        author: "alice".to_string(),
        message: "tx".to_string(),
        force: false,
        idempotency_key: None,
        token: None,
    };

    // Act
    let result = core.apply_mutations(req);

    // Assert: a transaction may not mix formats.
    assert!(matches!(result, Err(CoreError::MixedTransaction(_))));
}

// ── History ────────────────────────────────────────────────────────────────────

#[test]
fn op_log_records_each_write() {
    // Arrange
    let core = core();
    seed_user_message(&core);
    core.apply_mutation(request(
        "main",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "Extra".into(),
        }),
    ))
    .expect("second write");

    // Act
    let ops = core.op_log(PROJECT, REPO, None, None).expect("op log");

    // Assert: at least the two writes are recorded.
    assert!(ops.len() >= 2, "expected >=2 ops, got {}", ops.len());
}

#[test]
fn log_returns_real_commit_and_change_ids_distinct_from_op_ids() {
    // Arrange: two writes on `main` produce two real commits.
    let core = core();
    seed_user_message(&core);
    core.apply_mutation(request(
        "main",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "Extra".into(),
        }),
    ))
    .expect("second write");

    // Act: the real commit/change log vs. the operation log.
    let log = core
        .log(PROJECT, REPO, None, None, None)
        .expect("commit log");
    let ops = core.op_log(PROJECT, REPO, None, None).expect("op log");

    // Assert: log reports content-addressed commit ids and stable change ids,
    // and each commit's change id differs from its commit id and from any op id.
    assert!(log.len() >= 2, "expected >=2 commits, got {}", log.len());
    let op_ids: std::collections::HashSet<&str> = ops.iter().map(|o| o.op_id.as_str()).collect();
    for entry in &log {
        assert!(!entry.commit_id.is_empty(), "commit_id must be set");
        assert!(!entry.change_id.is_empty(), "change_id must be set");
        assert_ne!(
            entry.commit_id, entry.change_id,
            "commit id and change id are distinct identities"
        );
        assert!(
            !op_ids.contains(entry.commit_id.as_str()),
            "commit ids must NOT be op ids (real commit graph, not op-log derived)"
        );
        assert!(
            !op_ids.contains(entry.change_id.as_str()),
            "change ids must NOT be op ids"
        );
    }

    // The newest commit is first; it carries the second write's author/message
    // and a single parent (the seed commit).
    assert_eq!(log[0].author, "alice");
    assert_eq!(log[0].message, "test");
    assert_eq!(log[0].parents.len(), 1, "linear history => one parent");
    assert_eq!(log[0].parents[0], log[1].commit_id);
}

#[test]
fn diff_between_refs_reports_added_declaration() {
    // Arrange: seed on main, branch, add a message on the branch.
    let core = core();
    seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "feature/diff",
        &RefSpec::bookmark("main"),
        "alice",
        None,
    )
    .expect("branch");
    core.apply_mutation(request(
        "feature/diff",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "Added".into(),
        }),
    ))
    .expect("add on branch");

    // Act
    let changes = core
        .diff_bookmarks(&schema_path(), "main", "feature/diff", None)
        .expect("diff");

    // Assert: the new declaration shows as added.
    assert!(
        changes.iter().any(|c| matches!(
            c,
            schemahub_types::DeclChange::DeclarationAdded { name } if name == "Added"
        )),
        "expected Added in {changes:?}"
    );
}

// ── Format routing ─────────────────────────────────────────────────────────────

#[test]
fn unknown_format_id_is_rejected() {
    // Arrange: a mutation tagged with a format the registry does not have.
    let core = core();
    let req = MutationRequest {
        bookmark: "main".to_string(),
        mutation: Mutation {
            schema_path: schema_path(),
            format_id: "thrift".to_string(),
            operation: Bytes::new(),
        },
        author: "alice".to_string(),
        message: "x".to_string(),
        force: false,
        idempotency_key: None,
        token: None,
    };

    // Act
    let result = core.apply_mutation(req);

    // Assert
    assert!(matches!(result, Err(CoreError::UnknownFormat(f)) if f == "thrift"));
}

// Reference SchemaObjects so the type import stays exercised even if seed paths
// change; harmless otherwise.
#[allow(dead_code)]
fn _assert_schema_objects_default() -> SchemaObjects {
    SchemaObjects::default()
}
