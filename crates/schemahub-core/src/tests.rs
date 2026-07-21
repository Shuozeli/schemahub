//! Core orchestration tests (AAA style) over a `MemoryObjectDb`-backed `Jj`
//! and the real `schemahub-compiler-protobuf` (a dev-dependency used only to
//! produce real op envelopes / parse real `.proto` — core itself stays
//! format-agnostic).

use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use bytes::Bytes;
use schemahub_compiler_openapi::OpenApiCompiler;
use schemahub_compiler_protobuf::{
    OpAddField, OpChangeCardinality, OpCreateMessage, OpDeleteMessage, OpUpdateImport, ProtoOp,
    ProtobufCompiler,
};
use schemahub_jj::{Jj, JjError, MemoryObjectDb, ObjectDb, RedbObjectDb, RefSpec, SchemaWrite};
use schemahub_types::{
    Action, AuthzError, AuthzPolicy, CodegenOptions, Compiler, DeclKind, Identity, Mutation,
    MutationEffect, NoopAuthn, NoopAuthz, ResourcePath, SchemaObjects, SchemaPath,
};

use crate::change_record::validation::PreparedSchemaChange;
use crate::change_record::{
    ApplyAcquisition, ChangeEdit, ChangeLedger, ChangeRecordStatus, CreateChange,
    MemoryChangeRecordStore, ObjectDbChangeRecordStore, SystemChangeClock, UuidChangeIdGenerator,
};
use crate::config::{RepoConfig, RepoConfigStore, ReviewPolicy, ServingPolicy};
use crate::repository::{MemoryRepositoryStore, Repository, RepositoryError, RepositoryStore};
use crate::request::{
    DeleteSchemaRequest, MutationRequest, TransactionDeadline, TransactionRequest,
};
use crate::{
    CompilerRegistry, Core, CoreError, FingerprintBuilder, IdempotencyStore, MutationResponse,
    SchemaArtifactKind,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

const PROJECT: &str = "p";
const REPO: &str = "r";
const SCHEMA: &str = "user.proto";

/// Compiler wrapper that pauses after the core has loaded its immutable base
/// but before it publishes. Tests advance the bookmark during that window to
/// prove the eventual JJ write still uses the captured commit.
struct BlockingCompiler {
    inner: ProtobufCompiler,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl BlockingCompiler {
    fn pause(&self) {
        self.entered.wait();
        self.release.wait();
    }
}

impl Compiler for BlockingCompiler {
    fn format_id(&self) -> &'static str {
        self.inner.format_id()
    }

    fn parse(
        &self,
        source: &str,
    ) -> Result<schemahub_types::ParsedSchema, schemahub_types::ParseError> {
        self.inner.parse(source)
    }

    fn print(&self, schema: &SchemaObjects) -> Result<String, schemahub_types::PrintError> {
        self.inner.print(schema)
    }

    fn diff_decl(
        &self,
        old: &schemahub_types::DeclBlob,
        new: &schemahub_types::DeclBlob,
    ) -> Result<schemahub_types::DeclChange, schemahub_types::DiffError> {
        self.inner.diff_decl(old, new)
    }

    fn apply_mutation(
        &self,
        schema: &SchemaObjects,
        op: &Mutation,
    ) -> Result<MutationEffect, schemahub_types::MutationError> {
        self.pause();
        self.inner.apply_mutation(schema, op)
    }

    fn apply_mutations(
        &self,
        schema: &SchemaObjects,
        ops: &[Mutation],
    ) -> Result<MutationEffect, schemahub_types::MutationError> {
        self.pause();
        self.inner.apply_mutations(schema, ops)
    }

    fn check_compatibility(
        &self,
        old: &schemahub_types::DeclBlob,
        new: &schemahub_types::DeclBlob,
        rules: &schemahub_types::CompatibilityRules,
    ) -> Result<(), Vec<schemahub_types::CompatibilityViolation>> {
        self.inner.check_compatibility(old, new, rules)
    }

    fn render_conflict(
        &self,
        sides: &schemahub_types::ConflictSides,
    ) -> Result<String, schemahub_types::ConflictError> {
        self.inner.render_conflict(sides)
    }

    fn validate_resolution(
        &self,
        resolved: &schemahub_types::DeclBlob,
    ) -> Result<(), schemahub_types::ConflictError> {
        self.inner.validate_resolution(resolved)
    }

    fn summarize_decl(
        &self,
        blob: &schemahub_types::DeclBlob,
    ) -> Result<schemahub_types::DeclSummary, schemahub_types::ReadError> {
        self.inner.summarize_decl(blob)
    }

    fn decl_detail(
        &self,
        blob: &schemahub_types::DeclBlob,
    ) -> Result<schemahub_types::DeclDetail, schemahub_types::ReadError> {
        self.inner.decl_detail(blob)
    }

    fn imports(
        &self,
        schema: &schemahub_types::SchemaObjects,
    ) -> Result<Vec<schemahub_types::Import>, schemahub_types::ReadError> {
        self.inner.imports(schema)
    }

    fn type_refs(
        &self,
        blob: &schemahub_types::DeclBlob,
    ) -> Result<Vec<schemahub_types::TypeRef>, schemahub_types::ReadError> {
        self.inner.type_refs(blob)
    }

    fn field_type_ref(
        &self,
        blob: &schemahub_types::DeclBlob,
        field_name: &str,
    ) -> Result<Option<schemahub_types::TypeRef>, schemahub_types::ReadError> {
        self.inner.field_type_ref(blob, field_name)
    }

    fn generate_descriptors(
        &self,
        closure: &schemahub_types::SchemaClosure,
    ) -> Result<Bytes, schemahub_types::DescriptorError> {
        self.inner.generate_descriptors(closure)
    }

    fn generate_code(
        &self,
        closure: &schemahub_types::SchemaClosure,
        lang: schemahub_types::Language,
        options: &CodegenOptions,
    ) -> Result<String, schemahub_types::CodegenError> {
        self.inner.generate_code(closure, lang, options)
    }
}

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

fn openapi_core() -> Core {
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(OpenApiCompiler::new()));
    Core::with_config(
        Arc::new(Jj::new(Arc::new(MemoryObjectDb::new()))),
        registry,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        RepoConfigStore::new(),
    )
}

fn blocking_core() -> (Arc<Core>, Arc<Barrier>, Arc<Barrier>) {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(BlockingCompiler {
        inner: ProtobufCompiler::new(),
        entered: entered.clone(),
        release: release.clone(),
    }));
    let core = Core::with_config(
        Arc::new(Jj::new(Arc::new(MemoryObjectDb::new()))),
        registry,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        RepoConfigStore::new(),
    );
    (Arc::new(core), entered, release)
}

fn core_over_db(db: Arc<dyn ObjectDb>) -> Core {
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    Core::with_config(
        Arc::new(Jj::new(db)),
        registry,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        RepoConfigStore::new(),
    )
}

fn core_over_durable_db(db: Arc<dyn ObjectDb>) -> Core {
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    let change_ledger = ChangeLedger::new(
        Arc::new(ObjectDbChangeRecordStore::new(db.clone())),
        Arc::new(SystemChangeClock),
        Arc::new(UuidChangeIdGenerator),
    );
    let idempotency = IdempotencyStore::over_object_db(db.clone());
    Core::with_config_and_all_stores(
        Arc::new(Jj::new(db)),
        registry,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        RepoConfigStore::new(),
        change_ledger,
        Arc::new(MemoryRepositoryStore::new()),
        idempotency,
    )
}

fn core_with_repository_config(config: RepoConfig) -> Core {
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let repository_store = Arc::new(MemoryRepositoryStore::new());
    repository_store
        .create(Repository::new(PROJECT, REPO, config, "config", 1_000))
        .expect("seed repository policy");
    let change_ledger = ChangeLedger::new(
        Arc::new(MemoryChangeRecordStore::new()),
        Arc::new(SystemChangeClock),
        Arc::new(UuidChangeIdGenerator),
    );
    Core::with_config_and_resource_stores(
        Arc::new(Jj::new(db)),
        registry,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        RepoConfigStore::new(),
        change_ledger,
        repository_store,
    )
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
        base_revision: None,
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

fn seed_proto_schema(
    core: &Core,
    project: &str,
    repo: &str,
    bookmark: &str,
    schema_name: &str,
    source: &str,
) -> String {
    let parsed = ProtobufCompiler::new().parse(source).expect("parse schema");
    core.jj()
        .commit_write(
            project,
            repo,
            bookmark,
            schema_name,
            &RefSpec::bookmark(bookmark),
            MutationEffect {
                meta: Some(parsed.meta),
                upserts: parsed.decls,
                removes: Vec::new(),
            },
            "seed",
            "seed schema",
        )
        .expect("seed schema")
        .commit_id
}

fn seed_openapi_schema(
    core: &Core,
    project: &str,
    repo: &str,
    bookmark: &str,
    schema_name: &str,
    source: &str,
) -> String {
    let parsed = OpenApiCompiler::new()
        .parse(source)
        .expect("parse OpenAPI schema");
    core.jj()
        .commit_write(
            project,
            repo,
            bookmark,
            schema_name,
            &RefSpec::bookmark(bookmark),
            MutationEffect {
                meta: Some(parsed.meta),
                upserts: parsed.decls,
                removes: Vec::new(),
            },
            "seed",
            "seed OpenAPI schema",
        )
        .expect("seed OpenAPI schema")
        .commit_id
}

struct DenyHiddenProject;

impl AuthzPolicy for DenyHiddenProject {
    fn check(
        &self,
        _caller: &Identity,
        _action: Action,
        resource: &ResourcePath,
    ) -> Result<(), AuthzError> {
        if resource.project == "hidden" {
            Err(AuthzError::PermissionDenied(
                "hidden project is not visible".to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

fn protobuf_source_effect(source: &str) -> MutationEffect {
    let parsed = ProtobufCompiler::new().parse(source).expect("parse source");
    MutationEffect {
        meta: Some(parsed.meta),
        upserts: parsed.decls,
        removes: Vec::new(),
    }
}

fn protobuf_declaration_effect(source: &str) -> MutationEffect {
    let mut effect = protobuf_source_effect(source);
    effect.meta = None;
    effect
}

fn source_change(source: &str) -> CreateChange {
    CreateChange {
        project: PROJECT.to_string(),
        repo: REPO.to_string(),
        change_id: None,
        target_bookmark: "main".to_string(),
        base_revision: None,
        title: "Propose schema source".to_string(),
        description: String::new(),
        external_references: Vec::new(),
        edits: vec![ChangeEdit::ReplaceSource {
            schema: schema_path(),
            format_id: "protobuf".to_string(),
            source: source.to_string(),
        }],
    }
}

fn seed_dependency_pair(core: &Core) {
    let compiler = ProtobufCompiler::new();
    let common = compiler
        .parse("syntax=\"proto3\"; package common; message Shared { string id = 1; }")
        .expect("parse provider");
    let consumer = compiler
        .parse(
            "syntax=\"proto3\"; package orders; import \"p/r/common/types.proto\"; \
             message Order { common.Shared shared = 1; }",
        )
        .expect("parse consumer");
    core.jj()
        .commit_write_multi(
            PROJECT,
            REPO,
            "dev",
            &RefSpec::bookmark("dev"),
            vec![
                (
                    "common/types.proto".to_string(),
                    MutationEffect {
                        meta: Some(common.meta),
                        upserts: common.decls,
                        removes: Vec::new(),
                    },
                ),
                (
                    "orders/order.proto".to_string(),
                    MutationEffect {
                        meta: Some(consumer.meta),
                        upserts: consumer.decls,
                        removes: Vec::new(),
                    },
                ),
            ],
            "seed",
            "seed dependency pair",
        )
        .expect("seed dependency pair");
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

#[test]
fn protected_bookmark_rejects_top_level_declaration_removal() {
    // Arrange
    let core = core();
    seed_user_message(&core);
    let req = request(
        "main",
        ProtoOp::DeleteMessage(OpDeleteMessage {
            message_name: "User".into(),
        }),
    );

    // Act
    let error = core
        .apply_mutation(req)
        .expect_err("protected declaration removal must fail");

    // Assert
    assert!(matches!(error, CoreError::Incompatible(_)));
    let schema = core
        .jj()
        .load_schema(PROJECT, REPO, SCHEMA, &RefSpec::bookmark("main"))
        .expect("original schema remains");
    assert!(schema.decls.contains_key("User"));
}

#[test]
fn single_mutation_publishes_from_the_immutable_planning_snapshot() {
    // Arrange: pause writer A after it loads `dev`, publish a same-declaration
    // writer B update, then let A continue.
    let (core, entered, release) = blocking_core();
    seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "dev",
        &RefSpec::bookmark("main"),
        "alice",
        None,
    )
    .expect("create dev");
    let req = request(
        "dev",
        ProtoOp::AddField(OpAddField {
            message_name: "User".into(),
            field_name: "email".into(),
            field_type: "string".into(),
            field_number: 2,
            cardinality: String::new(),
        }),
    );
    let writer = {
        let core = core.clone();
        std::thread::spawn(move || core.apply_mutation(req))
    };
    entered.wait();
    let parsed = ProtobufCompiler::new()
        .parse("syntax=\"proto3\"; message User { int32 id = 1; string name = 3; }")
        .expect("parse concurrent source");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "dev",
            SCHEMA,
            &RefSpec::bookmark("dev"),
            MutationEffect {
                meta: Some(parsed.meta),
                upserts: parsed.decls,
                removes: Vec::new(),
            },
            "bob",
            "concurrent same-declaration edit",
        )
        .expect("publish writer B");

    // Act
    release.wait();
    let response = writer
        .join()
        .expect("writer thread")
        .expect("writer A publishes a conflict");

    // Assert
    assert!(response
        .conflicted_decls
        .iter()
        .any(|path| path.ends_with("User")));
    let conflict = core
        .jj()
        .read_conflict(PROJECT, REPO, SCHEMA, "User", &RefSpec::bookmark("dev"))
        .expect("read first-class conflict");
    assert_eq!(conflict.sides.len(), 2);
}

#[test]
fn protected_bookmark_rejects_a_concurrent_conflict_at_publication() {
    // Arrange: writer A plans from protected `main`, then writer B changes the
    // same declaration before A reaches the publication boundary.
    let (core, entered, release) = blocking_core();
    seed_user_message(&core);
    let mut req = request(
        "main",
        ProtoOp::AddField(OpAddField {
            message_name: "User".into(),
            field_name: "email".into(),
            field_type: "string".into(),
            field_number: 2,
            cardinality: String::new(),
        }),
    );
    req.force = true;
    req.idempotency_key = Some("protected-race".to_string());
    let writer = {
        let core = core.clone();
        std::thread::spawn(move || core.apply_mutation(req))
    };
    entered.wait();
    let parsed = ProtobufCompiler::new()
        .parse("syntax=\"proto3\"; message User { int32 id = 1; string name = 3; }")
        .expect("parse concurrent source");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "main",
            SCHEMA,
            &RefSpec::bookmark("main"),
            MutationEffect {
                meta: Some(parsed.meta),
                upserts: parsed.decls,
                removes: Vec::new(),
            },
            "bob",
            "concurrent protected edit",
        )
        .expect("publish writer B");

    // Act
    release.wait();
    let result = writer.join().expect("writer thread");

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::FailedPrecondition(message))
            if message.contains("protected bookmark") && message.contains("User")
    ));
    assert!(core
        .jj()
        .list_conflicted_declarations(PROJECT, REPO, &RefSpec::bookmark("main"))
        .expect("inspect protected bookmark")
        .is_empty());
    let source = core
        .get_schema_source(&schema_path(), &RefSpec::bookmark("main"), None)
        .expect("read winning source");
    assert!(source.contains("name"));
    assert!(!source.contains("email"));
}

#[test]
fn protected_merge_rejection_aborts_its_idempotency_receipt() {
    // Arrange: main and dev diverge on the same declaration.
    let core = core();
    let base = seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "dev",
        &RefSpec::commit(base.clone()),
        "alice",
        None,
    )
    .expect("create dev");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "main",
            SCHEMA,
            &RefSpec::commit(base.clone()),
            protobuf_source_effect(
                "syntax=\"proto3\"; message User { int32 id = 1; string main = 2; }",
            ),
            "alice",
            "edit main",
        )
        .expect("edit main");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "dev",
            SCHEMA,
            &RefSpec::commit(base.clone()),
            protobuf_source_effect(
                "syntax=\"proto3\"; message User { int32 id = 1; string dev = 3; }",
            ),
            "bob",
            "edit dev",
        )
        .expect("edit dev");

    // Act: the first attempt is rejected, then the destination is restored to
    // the merge base and the exact same idempotent request is retried.
    let rejected = core.merge_idempotent(
        PROJECT,
        REPO,
        "dev",
        "main",
        None,
        Some("protected-merge"),
        Some("merge dev into main"),
        "alice",
        None,
    );
    core.move_bookmark(PROJECT, REPO, "main", &RefSpec::commit(base), "alice", None)
        .expect("restore clean merge base");
    let retried = core.merge_idempotent(
        PROJECT,
        REPO,
        "dev",
        "main",
        None,
        Some("protected-merge"),
        Some("merge dev into main"),
        "alice",
        None,
    );

    // Assert
    assert!(matches!(
        rejected,
        Err(CoreError::FailedPrecondition(message))
            if message.contains("protected bookmark") && message.contains("User")
    ));
    assert!(retried
        .expect("retry after policy rejection")
        .conflicted_decls
        .is_empty());
}

#[test]
fn protected_bookmark_move_rejects_a_conflicted_target() {
    // Arrange
    let core = core();
    let base = seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "dev",
        &RefSpec::commit(base.clone()),
        "alice",
        None,
    )
    .expect("create dev");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "dev",
            SCHEMA,
            &RefSpec::commit(base.clone()),
            protobuf_declaration_effect(
                "syntax=\"proto3\"; message User { int32 id = 1; string left = 2; }",
            ),
            "alice",
            "left side",
        )
        .expect("publish left side");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "dev",
            SCHEMA,
            &RefSpec::commit(base),
            protobuf_declaration_effect(
                "syntax=\"proto3\"; message User { int32 id = 1; string right = 3; }",
            ),
            "bob",
            "right side",
        )
        .expect("publish conflicted dev");

    // Act
    let result = core.move_bookmark(
        PROJECT,
        REPO,
        "main",
        &RefSpec::bookmark("dev"),
        "alice",
        None,
    );

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::FailedPrecondition(message))
            if message.contains("protected bookmark") && message.contains("User")
    ));
    assert!(core
        .jj()
        .list_conflicted_declarations(PROJECT, REPO, &RefSpec::bookmark("main"))
        .expect("inspect main")
        .is_empty());
}

#[test]
fn undo_cannot_restore_a_conflict_to_a_protected_bookmark() {
    // Arrange: create a legacy conflict through the raw JJ layer, resolve it,
    // then ask Core to undo the resolution.
    let core = core();
    let base = seed_user_message(&core);
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "main",
            SCHEMA,
            &RefSpec::commit(base.clone()),
            protobuf_declaration_effect(
                "syntax=\"proto3\"; message User { int32 id = 1; string left = 2; }",
            ),
            "alice",
            "left side",
        )
        .expect("publish left side");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "main",
            SCHEMA,
            &RefSpec::commit(base),
            protobuf_declaration_effect(
                "syntax=\"proto3\"; message User { int32 id = 1; string right = 3; }",
            ),
            "bob",
            "right side",
        )
        .expect("publish legacy conflict");
    let resolved = ProtobufCompiler::new()
        .parse("syntax=\"proto3\"; message User { int32 id = 1; string final_name = 4; }")
        .expect("parse resolution")
        .decls
        .into_iter()
        .find(|(name, _)| name == "User")
        .expect("User declaration")
        .1;
    core.jj()
        .resolve_conflict(
            PROJECT,
            REPO,
            "main",
            SCHEMA,
            "User",
            resolved,
            "alice",
            "resolve legacy conflict",
        )
        .expect("resolve conflict");
    assert!(core
        .jj()
        .list_conflicted_declarations(PROJECT, REPO, &RefSpec::bookmark("main"))
        .expect("inspect resolved main")
        .is_empty());

    // Act
    let result = core.undo(PROJECT, REPO, "alice", None);

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::FailedPrecondition(message))
            if message.contains("protected bookmark") && message.contains("User")
    ));
    assert!(core
        .jj()
        .list_conflicted_declarations(PROJECT, REPO, &RefSpec::bookmark("main"))
        .expect("inspect main")
        .is_empty());
}

#[test]
fn consumer_racing_a_schema_delete_is_rejected_at_publication() {
    // Arrange: the consumer plans an unpinned import while the provider still
    // exists, then pauses while the provider deletion publishes first.
    let (core, entered, release) = blocking_core();
    let compiler = ProtobufCompiler::new();
    let provider = compiler
        .parse("syntax=\"proto3\"; package common; message Shared { string id = 1; }")
        .expect("parse provider");
    let consumer = compiler
        .parse("syntax=\"proto3\"; package orders; message Order { string id = 1; }")
        .expect("parse consumer");
    core.jj()
        .commit_write_multi(
            PROJECT,
            REPO,
            "dev",
            &RefSpec::bookmark("dev"),
            vec![
                (
                    "common/types.proto".to_string(),
                    MutationEffect {
                        meta: Some(provider.meta),
                        upserts: provider.decls,
                        removes: Vec::new(),
                    },
                ),
                (
                    "orders/order.proto".to_string(),
                    MutationEffect {
                        meta: Some(consumer.meta),
                        upserts: consumer.decls,
                        removes: Vec::new(),
                    },
                ),
            ],
            "seed",
            "seed race fixtures",
        )
        .expect("seed provider and consumer");
    let request = MutationRequest {
        bookmark: "dev".to_string(),
        mutation: Mutation {
            schema_path: SchemaPath::new(PROJECT, REPO, "orders/order.proto"),
            format_id: "protobuf".to_string(),
            operation: Bytes::from(
                ProtoOp::UpdateImport(OpUpdateImport {
                    import_path: format!("{PROJECT}/{REPO}/common/types.proto"),
                    resolved_commit: String::new(),
                    remove: false,
                })
                .encode(),
            ),
        },
        author: "alice".to_string(),
        message: "add live provider import".to_string(),
        force: false,
        idempotency_key: Some("consumer-delete-race".to_string()),
        base_revision: None,
        token: None,
    };
    let writer = {
        let core = core.clone();
        std::thread::spawn(move || core.apply_mutation(request))
    };
    entered.wait();
    core.delete_schema(DeleteSchemaRequest {
        schema: SchemaPath::new(PROJECT, REPO, "common/types.proto"),
        bookmark: "dev".to_string(),
        author: "bob".to_string(),
        message: "delete provider".to_string(),
        force: false,
        idempotency_key: Some("delete-racing-provider".to_string()),
        base_revision: None,
        token: None,
    })
    .expect("provider deletion publishes first");

    // Act
    release.wait();
    let result = writer.join().expect("consumer writer thread");

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::FailedPrecondition(message))
            if message.contains("orders/order.proto -> common/types.proto")
    ));
    assert!(matches!(
        core.jj().load_schema(
            PROJECT,
            REPO,
            "common/types.proto",
            &RefSpec::bookmark("dev")
        ),
        Err(JjError::SchemaNotFound(_))
    ));
    let consumer = core
        .get_schema_source(
            &SchemaPath::new(PROJECT, REPO, "orders/order.proto"),
            &RefSpec::bookmark("dev"),
            None,
        )
        .expect("read consumer after rejected race");
    assert!(!consumer.contains("common/types.proto"));
}

#[test]
fn change_apply_policy_rejection_releases_the_durable_lease() {
    // Arrange: validate a mutation-backed record, then race its second Apply
    // validation with a same-declaration writer on protected main.
    let (core, entered, release) = blocking_core();
    seed_user_message(&core);
    let operation = ProtoOp::AddField(OpAddField {
        message_name: "User".into(),
        field_name: "email".into(),
        field_type: "string".into(),
        field_number: 2,
        cardinality: String::new(),
    })
    .encode();
    let draft = core
        .create_change_record(
            CreateChange {
                project: PROJECT.to_string(),
                repo: REPO.to_string(),
                change_id: None,
                target_bookmark: "main".to_string(),
                base_revision: None,
                title: "Add email".to_string(),
                description: String::new(),
                external_references: Vec::new(),
                edits: vec![ChangeEdit::Mutation {
                    schema: schema_path(),
                    format_id: "protobuf".to_string(),
                    operation,
                }],
            },
            None,
        )
        .expect("create change");
    let validation = {
        let core = core.clone();
        let name = draft.name.clone();
        let etag = draft.etag.clone();
        std::thread::spawn(move || core.validate_change_record(&name, &etag, None))
    };
    entered.wait();
    release.wait();
    let validated = validation
        .join()
        .expect("validation thread")
        .expect("validate change");
    let ready = core
        .mark_change_ready(&validated.name, &validated.etag, None)
        .expect("mark ready");
    let apply = {
        let core = core.clone();
        let name = ready.name.clone();
        let etag = ready.etag.clone();
        std::thread::spawn(move || core.apply_change_record(&name, &etag, "apply-race", None))
    };
    // First Apply validation verifies the stored Ready snapshot.
    entered.wait();
    release.wait();
    // Second validation runs after the durable lease is acquired. Advance main
    // while it is paused, then let Apply reach atomic final-tree validation.
    entered.wait();
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "main",
            SCHEMA,
            &RefSpec::bookmark("main"),
            protobuf_source_effect(
                "syntax=\"proto3\"; message User { int32 id = 1; string name = 3; }",
            ),
            "bob",
            "concurrent apply edit",
        )
        .expect("publish concurrent edit");

    // Act
    release.wait();
    let result = apply.join().expect("apply thread");
    let persisted = core
        .get_change_record(&ready.name, None)
        .expect("reload rejected apply");

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::FailedPrecondition(message))
            if message.contains("protected bookmark") && message.contains("User")
    ));
    assert_eq!(persisted.status, ChangeRecordStatus::Ready);
    assert!(persisted.apply_attempt.is_none());
    assert!(persisted.apply_result.is_none());
    assert!(core
        .jj()
        .list_conflicted_declarations(PROJECT, REPO, &RefSpec::bookmark("main"))
        .expect("inspect main")
        .is_empty());
}

#[test]
fn transaction_publishes_from_the_immutable_planning_snapshot() {
    // Arrange
    let (core, entered, release) = blocking_core();
    seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "dev",
        &RefSpec::bookmark("main"),
        "alice",
        None,
    )
    .expect("create dev");
    let req = TransactionRequest {
        bookmark: "dev".to_string(),
        mutations: vec![proto_mutation(ProtoOp::AddField(OpAddField {
            message_name: "User".into(),
            field_name: "email".into(),
            field_type: "string".into(),
            field_number: 2,
            cardinality: String::new(),
        }))],
        author: "alice".to_string(),
        message: "transaction writer A".to_string(),
        force: false,
        idempotency_key: None,
        base_revision: None,
        token: None,
    };
    let writer = {
        let core = core.clone();
        std::thread::spawn(move || core.apply_mutations(req))
    };
    entered.wait();
    let parsed = ProtobufCompiler::new()
        .parse("syntax=\"proto3\"; message User { int32 id = 1; string name = 3; }")
        .expect("parse concurrent source");
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "dev",
            SCHEMA,
            &RefSpec::bookmark("dev"),
            MutationEffect {
                meta: Some(parsed.meta),
                upserts: parsed.decls,
                removes: Vec::new(),
            },
            "bob",
            "concurrent same-declaration edit",
        )
        .expect("publish writer B");

    // Act
    release.wait();
    let response = writer
        .join()
        .expect("writer thread")
        .expect("transaction writer A publishes a conflict");

    // Assert
    assert!(response
        .conflicted_decls
        .iter()
        .any(|path| path.ends_with("User")));
}

#[test]
fn cancelled_transaction_deadline_prevents_late_publication() {
    // Arrange: pause compiler execution after the immutable base is loaded,
    // then cancel the server-owned deadline before planning can continue.
    let (core, entered, release) = blocking_core();
    seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "dev",
        &RefSpec::bookmark("main"),
        "alice",
        None,
    )
    .expect("create dev");
    let before = core
        .jj()
        .resolve_ref_id(PROJECT, REPO, &RefSpec::bookmark("dev"))
        .expect("resolve dev before transaction");
    let request = TransactionRequest {
        bookmark: "dev".to_string(),
        mutations: vec![proto_mutation(ProtoOp::AddField(OpAddField {
            message_name: "User".into(),
            field_name: "late".into(),
            field_type: "string".into(),
            field_number: 2,
            cardinality: String::new(),
        }))],
        author: "alice".to_string(),
        message: "must not publish after timeout".to_string(),
        force: false,
        idempotency_key: Some("deadline-cancelled".to_string()),
        base_revision: None,
        token: None,
    };
    let deadline = TransactionDeadline::after(Duration::from_secs(60));
    let worker = {
        let core = core.clone();
        let deadline = deadline.clone();
        std::thread::spawn(move || core.apply_mutations_with_deadline(request, deadline))
    };
    entered.wait();

    // Act
    deadline.cancel();
    release.wait();
    let result = worker.join().expect("transaction worker");

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::TransactionDeadlineExceeded)
    ));
    assert_eq!(
        core.jj()
            .resolve_ref_id(PROJECT, REPO, &RefSpec::bookmark("dev"))
            .expect("resolve dev after deadline"),
        before
    );
}

#[test]
fn deadline_expiring_while_waiting_to_publish_aborts_the_receipt() {
    // Arrange: hold the repository publication guard until the transaction has
    // claimed its durable idempotency receipt and is queued at the final gate.
    let db = Arc::new(MemoryObjectDb::new());
    let core = Arc::new(core_over_durable_db(db.clone()));
    seed_user_message(&core);
    core.create_bookmark(
        PROJECT,
        REPO,
        "dev",
        &RefSpec::bookmark("main"),
        "alice",
        None,
    )
    .expect("create dev");
    let before = core
        .jj()
        .resolve_ref_id(PROJECT, REPO, &RefSpec::bookmark("dev"))
        .expect("resolve dev before transaction");
    let guard = db
        .acquire_publication_guard("p/r")
        .expect("hold publication guard");
    let request = TransactionRequest {
        bookmark: "dev".to_string(),
        mutations: vec![proto_mutation(ProtoOp::AddField(OpAddField {
            message_name: "User".into(),
            field_name: "late".into(),
            field_type: "string".into(),
            field_number: 2,
            cardinality: String::new(),
        }))],
        author: "alice".to_string(),
        message: "deadline at publication gate".to_string(),
        force: false,
        idempotency_key: Some("deadline-at-publication".to_string()),
        base_revision: None,
        token: None,
    };
    let deadline = TransactionDeadline::after(Duration::from_secs(60));
    let worker = {
        let core = core.clone();
        let deadline = deadline.clone();
        std::thread::spawn(move || core.apply_mutations_with_deadline(request, deadline))
    };
    let wait_until = Instant::now() + Duration::from_secs(5);
    while db
        .list_records("schemahub.idempotency.v1")
        .expect("list receipts")
        .is_empty()
    {
        assert!(
            Instant::now() < wait_until,
            "transaction did not reach the publication gate"
        );
        std::thread::yield_now();
    }

    // Act
    deadline.cancel();
    drop(guard);
    let result = worker.join().expect("transaction worker");

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::TransactionDeadlineExceeded)
    ));
    assert_eq!(
        core.jj()
            .resolve_ref_id(PROJECT, REPO, &RefSpec::bookmark("dev"))
            .expect("resolve dev after deadline"),
        before
    );
    assert!(db
        .list_records("schemahub.idempotency.v1")
        .expect("list receipts after rejection")
        .is_empty());
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

    // Assert: force skips the gate and remains visible in the durable audit
    // operation rather than living only in an expiring idempotency receipt.
    assert!(resp.is_ok(), "force should bypass gate: {resp:?}");
    let operation = core
        .jj()
        .list_operations(PROJECT, REPO)
        .expect("read audit operations")
        .into_iter()
        .find(|operation| operation.attributes.contains_key("schemahub.force"))
        .expect("forced mutation operation");
    assert_eq!(
        operation
            .attributes
            .get("schemahub.force")
            .map(String::as_str),
        Some("true")
    );
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
            ..RepoConfig::default()
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

#[test]
fn idempotent_mutation_receipt_survives_redb_restart() {
    // Arrange
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("schemahub.redb");
    let mut req = request(
        "feature/restart-idem",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "OnceAfterRestart".into(),
        }),
    );
    req.idempotency_key = Some("restart-key".to_string());
    let (first, operation_count) = {
        let db: Arc<dyn ObjectDb> = Arc::new(RedbObjectDb::open(&db_path).expect("open redb"));
        let core = core_over_durable_db(db);
        let first = core.apply_mutation(req.clone()).expect("first write");
        let operation_count = core
            .op_log(PROJECT, REPO, None, None)
            .expect("op log before restart")
            .len();
        (first, operation_count)
    };
    let db: Arc<dyn ObjectDb> = Arc::new(RedbObjectDb::open(&db_path).expect("reopen redb"));
    let restarted = core_over_durable_db(db);

    // Act
    let replay = restarted.apply_mutation(req).expect("restart replay");
    let operations_after = restarted
        .op_log(PROJECT, REPO, None, None)
        .expect("op log after restart")
        .len();

    // Assert
    assert_eq!(replay, first);
    assert_eq!(operations_after, operation_count);
}

#[test]
fn pending_receipt_recovers_correlated_jj_write_after_redb_restart() {
    // Arrange: emulate a process stopping after JJ publication but before the
    // receipt's completion CAS.
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("schemahub.redb");
    let scope = "test-crash-recovery/p/r";
    let mut fingerprint = FingerprintBuilder::new("test-crash-recovery");
    fingerprint.update(b"same-request");
    let fingerprint = fingerprint.finish();
    let first = {
        let db: Arc<dyn ObjectDb> = Arc::new(RedbObjectDb::open(&db_path).expect("open redb"));
        let core = core_over_durable_db(db);
        let attempt = match core
            .begin_idempotent_write(
                scope,
                Some("crash-key"),
                &fingerprint,
                PROJECT,
                REPO,
                "main",
            )
            .expect("claim receipt")
        {
            crate::mutation::idempotency::IdempotentWrite::Proceed(Some(attempt)) => attempt,
            _ => panic!("first request must own the receipt lease"),
        };
        let compiler = ProtobufCompiler::new();
        let parsed = compiler
            .parse("syntax=\"proto3\"; message CrashSafe {}")
            .expect("parse schema");
        let effect = MutationEffect {
            meta: Some(parsed.meta),
            upserts: parsed.decls,
            removes: Vec::new(),
        };
        let write = core
            .jj()
            .commit_schema_changes(
                PROJECT,
                REPO,
                "main",
                &RefSpec::bookmark("main"),
                vec![SchemaWrite::Patch {
                    schema_path: SCHEMA.to_string(),
                    effect,
                }],
                "alice",
                "crash-safe write",
                attempt.attributes(),
            )
            .expect("publish correlated write");
        MutationResponse {
            commit_id: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        }
    };
    let db: Arc<dyn ObjectDb> = Arc::new(RedbObjectDb::open(&db_path).expect("reopen redb"));
    let restarted = core_over_durable_db(db);
    let operations_before = restarted
        .op_log(PROJECT, REPO, None, None)
        .expect("op log before recovery")
        .len();

    // Act: the supplied write would create another commit if reconciliation
    // failed, but the historical operation repairs and replays the receipt.
    let replay = restarted
        .commit_idempotent_schema_changes(
            scope,
            Some("crash-key"),
            &fingerprint,
            PROJECT,
            REPO,
            "main",
            &RefSpec::bookmark("main"),
            Vec::new(),
            "alice",
            "crash-safe write",
        )
        .expect("recover receipt");
    let operations_after = restarted
        .op_log(PROJECT, REPO, None, None)
        .expect("op log after recovery")
        .len();

    // Assert
    assert_eq!(replay, first);
    assert_eq!(operations_after, operations_before);
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
fn detailed_search_fails_closed_on_an_unknown_schema_format() {
    // Arrange
    let core = core();
    core.jj()
        .commit_write(
            PROJECT,
            REPO,
            "main",
            "opaque.unknown",
            &RefSpec::bookmark("main"),
            protobuf_source_effect("syntax=\"proto3\"; message Hidden {}"),
            "seed",
            "seed unknown-format object",
        )
        .expect("seed unknown-format schema");

    // Act
    let result = core.search_detailed(PROJECT, REPO, &RefSpec::bookmark("main"), "hidden", None);

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::UndetectableFormat(schema)) if schema == "opaque.unknown"
    ));
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

#[test]
fn forward_dependency_resolves_live_cross_repository_snapshot() {
    // Arrange
    let core = core();
    let provider_commit = seed_proto_schema(
        &core,
        "acme",
        "provider",
        "main",
        "types.proto",
        "syntax=\"proto3\"; message Shared {}",
    );
    let consumer_commit = seed_proto_schema(
        &core,
        "billing",
        "consumer",
        "main",
        "invoice.proto",
        "syntax=\"proto3\"; import \"acme/provider/types.proto\"; message Invoice { Shared shared = 1; }",
    );
    let consumer = SchemaPath::new("billing", "consumer", "invoice.proto");

    // Act
    let (dependencies, root_commit) = core
        .list_dependencies_detailed(&consumer, &RefSpec::bookmark("main"), false, None)
        .expect("list forward dependencies");

    // Assert
    assert_eq!(root_commit, consumer_commit);
    assert_eq!(dependencies.len(), 1);
    assert_eq!(
        dependencies[0].imported_schema,
        SchemaPath::new("acme", "provider", "types.proto")
    );
    assert_eq!(dependencies[0].target_commit, provider_commit);
    assert!(dependencies[0].resolved);
    assert!(dependencies[0].import.resolved_commit.is_empty());
}

#[test]
fn openapi_external_ref_uses_immutable_same_repository_snapshot_and_blocks_delete() {
    // Arrange
    let core = openapi_core();
    let provider_v1 = r#"
openapi: "3.1.0"
info: { title: Contracts, version: "1.0.0" }
paths: {}
components:
  schemas:
    Order:
      type: object
      description: v1 contract
"#;
    seed_openapi_schema(&core, PROJECT, REPO, "main", "contracts.yaml", provider_v1);
    let consumer = SchemaPath::new(PROJECT, REPO, "apis/api.yaml");
    let consumer_commit = seed_openapi_schema(
        &core,
        PROJECT,
        REPO,
        "main",
        "apis/api.yaml",
        r#"
openapi: "3.1.0"
info: { title: Orders, version: "1.0.0" }
paths:
  /orders:
    get:
      responses:
        '200':
          description: Success
          content:
            application/json:
              schema:
                $ref: '../contracts.yaml#/components/schemas/Order'
components:
  schemas:
    Envelope:
      type: object
      properties:
        order:
          $ref: '../contracts.yaml#/components/schemas/Order'
"#,
    );
    seed_openapi_schema(
        &core,
        PROJECT,
        REPO,
        "main",
        "contracts.yaml",
        &provider_v1.replace("v1 contract", "v2 contract"),
    );

    // Act
    let (dependencies, root_commit) = core
        .list_dependencies_detailed(
            &consumer,
            &RefSpec::commit(consumer_commit.clone()),
            false,
            None,
        )
        .expect("list OpenAPI dependencies");
    let descriptors = core
        .generate_descriptors_at(&consumer, &RefSpec::commit(consumer_commit.clone()), None)
        .expect("build OpenAPI descriptor closure");
    let followed = core
        .follow_field_type(
            &consumer,
            &RefSpec::commit(consumer_commit.clone()),
            "schema:Envelope",
            "order",
            None,
        )
        .expect("follow external OpenAPI field type");
    let dependents = core
        .list_dependents(&SchemaPath::new(PROJECT, REPO, "contracts.yaml"), None)
        .expect("discover OpenAPI dependent");
    let delete_result = core.delete_schema(DeleteSchemaRequest {
        schema: SchemaPath::new(PROJECT, REPO, "contracts.yaml"),
        bookmark: "main".to_string(),
        author: "agent".to_string(),
        message: "delete live provider".to_string(),
        force: true,
        idempotency_key: None,
        base_revision: None,
        token: None,
    });

    // Assert
    assert_eq!(root_commit, consumer_commit);
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].import.path, "../contracts.yaml");
    assert_eq!(dependencies[0].import.decl_name, "schema:Order");
    assert_eq!(dependencies[0].target_commit, consumer_commit);
    assert!(dependencies[0].resolved);
    assert_eq!(
        followed.target_schema,
        SchemaPath::new(PROJECT, REPO, "contracts.yaml")
    );
    assert_eq!(followed.target_commit, consumer_commit);
    assert_eq!(followed.summary.name, "schema:Order");
    assert!(!followed.pinned);
    assert_eq!(dependents.dependents.len(), 1);
    assert_eq!(dependents.dependents[0].importing_schema, consumer);
    assert_eq!(dependents.dependents[0].import.decl_name, "schema:Order");
    let descriptors = std::str::from_utf8(&descriptors).expect("OpenAPI descriptors are YAML");
    assert!(descriptors.contains("v1 contract"), "{descriptors}");
    assert!(!descriptors.contains("v2 contract"), "{descriptors}");
    assert!(matches!(
        delete_result,
        Err(CoreError::FailedPrecondition(message)) if message.contains("apis/api.yaml")
    ));
}

#[test]
fn openapi_relative_import_cannot_escape_repository_root() {
    // Arrange
    let importing = SchemaPath::new(PROJECT, REPO, "apis/api.yaml");
    let schemas = std::collections::HashSet::new();

    // Act
    let result =
        crate::exploration::normalize_import_path(&importing, "../../contracts.yaml", &schemas);

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::FailedPrecondition(message)) if message.contains("escapes the repository root")
    ));
}

#[test]
fn forward_dependency_keeps_unreadable_target_explicit_without_traversing_it() {
    // Arrange
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    let core = Core::with_config(
        Arc::new(Jj::new(Arc::new(MemoryObjectDb::new()))),
        registry,
        Arc::new(NoopAuthn),
        Arc::new(DenyHiddenProject),
        RepoConfigStore::new(),
    );
    seed_proto_schema(
        &core,
        "hidden",
        "provider",
        "main",
        "types.proto",
        "syntax=\"proto3\"; message Secret {}",
    );
    seed_proto_schema(
        &core,
        "acme",
        "consumer",
        "main",
        "public.proto",
        "syntax=\"proto3\"; import \"hidden/provider/types.proto\"; message Public { Secret secret = 1; }",
    );
    let consumer = SchemaPath::new("acme", "consumer", "public.proto");

    // Act
    let (dependencies, _) = core
        .list_dependencies_detailed(&consumer, &RefSpec::bookmark("main"), true, None)
        .expect("list visible edge without reading hidden target");

    // Assert
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].import.path, "hidden/provider/types.proto");
    assert_eq!(dependencies[0].imported_schema.project, "hidden");
    assert!(!dependencies[0].resolved);
    assert!(dependencies[0].target_commit.is_empty());
}

#[test]
fn dependent_discovery_returns_direct_edges_and_immutable_repo_snapshots() {
    // Arrange
    let mut configs = RepoConfigStore::new();
    configs.set(
        "acme",
        "live-consumer",
        RepoConfig {
            default_bookmark: "trunk".to_string(),
            ..RepoConfig::default()
        },
    );
    configs.set(
        "acme",
        "stale-consumer",
        RepoConfig {
            default_bookmark: "trunk".to_string(),
            ..RepoConfig::default()
        },
    );
    let core = core_with_config(configs);
    let provider_commit = seed_proto_schema(
        &core,
        "acme",
        "provider",
        "main",
        "types.proto",
        "syntax=\"proto3\"; message Shared { string id = 1; }",
    );
    let live_commit = seed_proto_schema(
        &core,
        "acme",
        "live-consumer",
        "trunk",
        "orders.proto",
        "syntax=\"proto3\"; import \"acme/provider/types.proto\"; message Order { string id = 1; }",
    );
    seed_proto_schema(
        &core,
        "acme",
        "stale-consumer",
        "main",
        "stale.proto",
        "syntax=\"proto3\"; import \"acme/provider/types.proto\"; message Stale { string id = 1; }",
    );
    seed_proto_schema(
        &core,
        "billing",
        "pinned-consumer",
        "main",
        "invoice.proto",
        "syntax=\"proto3\"; message Invoice { string id = 1; }",
    );
    let pinned_result = core
        .apply_mutation(MutationRequest {
            bookmark: "main".to_string(),
            mutation: Mutation {
                schema_path: SchemaPath::new("billing", "pinned-consumer", "invoice.proto"),
                format_id: "protobuf".to_string(),
                operation: Bytes::from(
                    ProtoOp::UpdateImport(OpUpdateImport {
                        import_path: "acme/provider/types.proto".to_string(),
                        resolved_commit: provider_commit.clone(),
                        remove: false,
                    })
                    .encode(),
                ),
            },
            author: "agent".to_string(),
            message: "pin provider".to_string(),
            force: false,
            idempotency_key: None,
            base_revision: None,
            token: None,
        })
        .expect("pin provider import");
    let target = SchemaPath::new("acme", "provider", "types.proto");

    // Act
    let scan = core
        .list_dependents(&target, None)
        .expect("scan visible repositories");

    // Assert
    let importing_paths: Vec<_> = scan
        .dependents
        .iter()
        .map(|dependent| dependent.importing_schema.to_string())
        .collect();
    assert_eq!(
        importing_paths,
        vec![
            "acme/live-consumer/orders.proto".to_string(),
            "billing/pinned-consumer/invoice.proto".to_string(),
        ]
    );
    assert_eq!(scan.dependents[0].importing_bookmark, "trunk");
    assert_eq!(scan.dependents[0].importing_commit, live_commit);
    assert!(scan.dependents[0].import.resolved_commit.is_empty());
    assert_eq!(scan.dependents[1].import.resolved_commit, provider_commit);
    assert_eq!(scan.dependents[1].importing_commit, pinned_result.commit_id);
    assert!(scan
        .snapshots
        .iter()
        .all(|snapshot| snapshot.repo != "stale-consumer"));
    assert_eq!(scan.schemas_scanned, 3);
}

#[test]
fn dependent_discovery_does_not_disclose_unreadable_repositories() {
    // Arrange
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    let core = Core::with_config(
        Arc::new(Jj::new(Arc::new(MemoryObjectDb::new()))),
        registry,
        Arc::new(NoopAuthn),
        Arc::new(DenyHiddenProject),
        RepoConfigStore::new(),
    );
    seed_proto_schema(
        &core,
        "acme",
        "provider",
        "main",
        "types.proto",
        "syntax=\"proto3\"; message Shared { string id = 1; }",
    );
    seed_proto_schema(
        &core,
        "acme",
        "visible-consumer",
        "main",
        "visible.proto",
        "syntax=\"proto3\"; import \"acme/provider/types.proto\"; message Visible { string id = 1; }",
    );
    seed_proto_schema(
        &core,
        "hidden",
        "private-consumer",
        "main",
        "private.proto",
        "syntax=\"proto3\"; import \"acme/provider/types.proto\"; message Private { string id = 1; }",
    );
    let target = SchemaPath::new("acme", "provider", "types.proto");

    // Act
    let scan = core
        .list_dependents(&target, None)
        .expect("scan readable repositories");

    // Assert
    assert_eq!(scan.dependents.len(), 1);
    assert_eq!(scan.dependents[0].importing_schema.project, "acme");
    assert!(scan
        .snapshots
        .iter()
        .all(|snapshot| snapshot.project != "hidden"));
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
        base_revision: None,
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
        base_revision: None,
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
        base_revision: None,
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
        base_revision: None,
        token: None,
    };

    // Act
    let result = core.apply_mutations(req);

    // Assert: a transaction may not mix formats.
    assert!(matches!(result, Err(CoreError::MixedTransaction(_))));
}

#[test]
fn repository_policy_can_require_change_records_for_publication() {
    // Arrange
    let config = RepoConfig {
        review_policy: ReviewPolicy {
            required_approvals: 0,
            require_change_record: true,
        },
        ..RepoConfig::default()
    };
    let core = core_with_repository_config(config);
    let request = request(
        "main",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "DirectWrite".to_string(),
        }),
    );

    // Act
    let result = core.apply_mutation(request);

    // Assert
    assert!(matches!(
        result,
        Err(CoreError::Repository(RepositoryError::FailedPrecondition(
            _
        )))
    ));
}

// ── Durable change validation ─────────────────────────────────────────────────

#[test]
fn validate_change_replays_source_and_persists_deterministic_snapshot() {
    // Arrange
    let core = core();
    let draft = core
        .create_change_record(
            source_change("syntax = \"proto3\"; message Order { string id = 1; }"),
            None,
        )
        .expect("create executable draft");

    // Act
    let first = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate draft");
    let second = core
        .validate_change_record(&first.name, &first.etag, None)
        .expect("repeat validation");

    // Assert
    let first_result = first.validation.expect("first validation result");
    let second_result = second.validation.expect("second validation result");
    assert!(first_result.valid);
    assert!(first_result.issues.is_empty());
    assert!(!first_result.resolved_base_commit.is_empty());
    assert!(first_result.edit_digest.starts_with("sha256:"));
    assert_eq!(first_result.edit_digest, second_result.edit_digest);
    assert_eq!(
        first_result.resolved_base_commit,
        second_result.resolved_base_commit
    );
    assert_eq!(second.etag, "v3");
}

#[test]
fn invalid_source_is_stored_as_validation_data_and_blocks_ready() {
    // Arrange
    let core = core();
    let draft = core
        .create_change_record(source_change("this is not protobuf"), None)
        .expect("create executable draft");

    // Act
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validation findings are a successful RPC result");
    let ready = core.mark_change_ready(&validated.name, &validated.etag, None);

    // Assert
    let result = validated.validation.expect("validation result");
    assert!(!result.valid);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.code == "source_invalid"));
    assert!(matches!(ready, Err(CoreError::ChangeLedger(_))));
}

#[test]
fn unresolvable_base_revision_is_stored_as_validation_data() {
    // Arrange
    let core = core();
    let mut input = source_change("syntax = \"proto3\"; message Order {}");
    input.base_revision = Some("not-a-commit".to_string());
    let draft = core
        .create_change_record(input, None)
        .expect("create pinned draft");

    // Act
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate pinned draft");

    // Assert
    let result = validated.validation.expect("validation result");
    assert!(!result.valid);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.code == "base_revision_unresolvable"));
}

#[test]
fn validation_blocks_breaking_source_replacement_on_protected_bookmark() {
    // Arrange
    let core = core();
    seed_user_message(&core);
    let draft = core
        .create_change_record(
            source_change("syntax = \"proto3\"; message User { string id = 1; }"),
            None,
        )
        .expect("create breaking draft");

    // Act
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate breaking draft");

    // Assert
    let result = validated.validation.expect("validation result");
    assert!(!result.valid);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.code == "compatibility_violation"
            && issue.declaration_name.as_deref() == Some("User")));
    assert_eq!(validated.status, ChangeRecordStatus::Draft);
}

#[test]
fn validation_rejects_schema_delete_with_a_live_unpinned_dependent() {
    // Arrange
    let core = core();
    seed_dependency_pair(&core);
    let draft = core
        .create_change_record(
            CreateChange {
                project: PROJECT.to_string(),
                repo: REPO.to_string(),
                change_id: None,
                target_bookmark: "dev".to_string(),
                base_revision: None,
                title: "Delete shared types".to_string(),
                description: String::new(),
                external_references: Vec::new(),
                edits: vec![ChangeEdit::DeleteSchema {
                    schema: SchemaPath::new(PROJECT, REPO, "common/types.proto"),
                    format_id: "protobuf".to_string(),
                }],
            },
            None,
        )
        .expect("create delete draft");

    // Act
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate delete draft");

    // Assert
    let result = validated.validation.expect("validation result");
    assert!(!result.valid);
    assert!(result.issues.iter().any(|issue| {
        issue.code == "live_schema_dependency"
            && issue.message.contains("orders/order.proto")
            && issue.schema_name.as_deref() == Some("common/types.proto")
    }));
}

#[test]
fn change_can_remove_a_consumer_import_and_delete_its_provider_atomically() {
    // Arrange
    let core = core();
    seed_dependency_pair(&core);
    let draft = core
        .create_change_record(
            CreateChange {
                project: PROJECT.to_string(),
                repo: REPO.to_string(),
                change_id: None,
                target_bookmark: "dev".to_string(),
                base_revision: None,
                title: "Retire shared types".to_string(),
                description: String::new(),
                external_references: Vec::new(),
                edits: vec![
                    ChangeEdit::ReplaceSource {
                        schema: SchemaPath::new(PROJECT, REPO, "orders/order.proto"),
                        format_id: "protobuf".to_string(),
                        source: "syntax=\"proto3\"; package orders; \
                                 message Order { string id = 1; }"
                            .to_string(),
                    },
                    ChangeEdit::DeleteSchema {
                        schema: SchemaPath::new(PROJECT, REPO, "common/types.proto"),
                        format_id: "protobuf".to_string(),
                    },
                ],
            },
            None,
        )
        .expect("create atomic migration");
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate atomic migration");
    let ready = core
        .mark_change_ready(&validated.name, &validated.etag, None)
        .expect("mark migration ready");

    // Act
    let applied = core
        .apply_change_record(&ready.name, &ready.etag, "atomic-delete", None)
        .expect("apply atomic migration");

    // Assert
    assert_eq!(applied.status, ChangeRecordStatus::Applied);
    assert!(matches!(
        core.jj().load_schema(
            PROJECT,
            REPO,
            "common/types.proto",
            &RefSpec::bookmark("dev")
        ),
        Err(JjError::SchemaNotFound(_))
    ));
    let consumer = core
        .get_schema_source(
            &SchemaPath::new(PROJECT, REPO, "orders/order.proto"),
            &RefSpec::bookmark("dev"),
            None,
        )
        .expect("read migrated consumer");
    assert!(!consumer.contains("common/types.proto"));
}

#[test]
fn validated_source_change_can_transition_to_ready() {
    // Arrange
    let core = core();
    let draft = core
        .create_change_record(
            source_change("syntax = \"proto3\"; message Order { string id = 1; }"),
            None,
        )
        .expect("create executable draft");
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate draft");

    // Act
    let ready = core
        .mark_change_ready(&validated.name, &validated.etag, None)
        .expect("mark ready");

    // Assert
    assert_eq!(ready.status, ChangeRecordStatus::Ready);
    assert!(ready.validation.is_some_and(|result| result.valid));
}

#[test]
fn apply_change_publishes_once_and_returns_same_receipt_on_retry() {
    // Arrange
    let core = core();
    let draft = core
        .create_change_record(
            source_change("syntax = \"proto3\"; message Order { string id = 1; }"),
            None,
        )
        .expect("create executable draft");
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate draft");
    let ready = core
        .mark_change_ready(&validated.name, &validated.etag, None)
        .expect("mark ready");

    // Act
    let applied = core
        .apply_change_record(&ready.name, &ready.etag, "apply-request-1", None)
        .expect("apply change");
    let retried = core
        .apply_change_record(&ready.name, &ready.etag, "apply-request-1", None)
        .expect("retry apply");

    // Assert
    assert_eq!(applied.status, ChangeRecordStatus::Applied);
    assert_eq!(applied.apply_result, retried.apply_result);
    assert_eq!(applied.etag, retried.etag);
    let receipt = applied.apply_result.expect("apply receipt");
    assert!(!receipt.commit_id.is_empty());
    assert!(!receipt.change_id.is_empty());
    assert!(!receipt.operation_id.is_empty());
    let schema = core
        .jj()
        .load_schema(PROJECT, REPO, SCHEMA, &RefSpec::bookmark("main"))
        .expect("read applied schema");
    assert!(schema.decls.contains_key("Order"));
    let correlated: Vec<_> = core
        .jj()
        .list_operations(PROJECT, REPO)
        .expect("op log")
        .into_iter()
        .filter(|operation| {
            operation.attributes.get("schemahub.change_record") == Some(&ready.name)
        })
        .collect();
    assert_eq!(correlated.len(), 1);
}

#[test]
fn apply_retry_after_redb_restart_recovers_commit_written_before_record_completion() {
    // Arrange: acquire the durable lease and publish its correlated JJ write,
    // then drop every in-process handle before `complete_apply` to model a
    // process crash and restart.
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("schemahub.redb");
    let db: Arc<dyn ObjectDb> =
        Arc::new(RedbObjectDb::open(&db_path).expect("open redb before crash"));
    let core = core_over_durable_db(db);
    let draft = core
        .create_change_record(
            source_change("syntax = \"proto3\"; message Order { string id = 1; }"),
            None,
        )
        .expect("create executable draft");
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate draft");
    let ready = core
        .mark_change_ready(&validated.name, &validated.etag, None)
        .expect("mark ready");
    let applying = match core
        .change_ledger()
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "crash-request",
            &Identity::Anonymous,
            300_000,
        )
        .expect("acquire apply")
    {
        ApplyAcquisition::Acquired(record) => record,
        other => panic!("expected acquired apply, got {other:?}"),
    };
    let attempt = applying.apply_attempt.as_ref().expect("attempt");
    let outcome =
        crate::change_record::validation::validate(&core, &applying).expect("rebuild write plan");
    let prepared = outcome.prepared.expect("valid write plan");
    let writes = prepared
        .writes
        .into_iter()
        .map(|write| match write {
            PreparedSchemaChange::Patch {
                schema_name,
                effect,
            } => SchemaWrite::Patch {
                schema_path: schema_name,
                effect,
            },
            PreparedSchemaChange::Delete { schema_name } => SchemaWrite::Delete {
                schema_path: schema_name,
            },
        })
        .collect();
    let attributes = BTreeMap::from([
        ("schemahub.change_record".to_string(), ready.name.clone()),
        (
            "schemahub.apply_attempt".to_string(),
            attempt.attempt_id.clone(),
        ),
        (
            "schemahub.apply_request".to_string(),
            "crash-request".to_string(),
        ),
    ]);
    let orphaned_receipt = core
        .jj()
        .commit_schema_changes(
            PROJECT,
            REPO,
            "main",
            &RefSpec::commit(prepared.resolved_base_commit),
            writes,
            "anonymous",
            &applying.title,
            attributes,
        )
        .expect("publish before simulated crash");
    drop(core);
    let reopened: Arc<dyn ObjectDb> =
        Arc::new(RedbObjectDb::open(&db_path).expect("reopen redb after crash"));
    let restarted = core_over_durable_db(reopened);

    // Act
    let recovered = restarted
        .apply_change_record(&ready.name, &ready.etag, "crash-request", None)
        .expect("retry reconciles orphaned receipt");

    // Assert
    assert_eq!(recovered.status, ChangeRecordStatus::Applied);
    let receipt = recovered.apply_result.expect("recovered receipt");
    assert_eq!(receipt.commit_id, orphaned_receipt.commit_id);
    assert_eq!(receipt.operation_id, orphaned_receipt.operation_id);
    let correlated: Vec<_> = restarted
        .jj()
        .list_operations(PROJECT, REPO)
        .expect("read operations after restart")
        .into_iter()
        .filter(|operation| {
            operation.attributes.get("schemahub.change_record") == Some(&ready.name)
        })
        .collect();
    assert_eq!(correlated.len(), 1);
}

#[test]
fn repository_review_policy_blocks_apply_until_approval_threshold_is_met() {
    // Arrange
    let config = RepoConfig {
        review_policy: ReviewPolicy {
            required_approvals: 1,
            require_change_record: true,
        },
        ..RepoConfig::default()
    };
    let core = core_with_repository_config(config);
    let draft = core
        .create_change_record(
            source_change("syntax = \"proto3\"; message Reviewed { string id = 1; }"),
            None,
        )
        .expect("create change");
    let validated = core
        .validate_change_record(&draft.name, &draft.etag, None)
        .expect("validate change");
    let ready = core
        .mark_change_ready(&validated.name, &validated.etag, None)
        .expect("mark ready");

    // Act: apply before review, then add a distinct reviewer and retry.
    let blocked = core.apply_change_record(&ready.name, &ready.etag, "review-policy", None);
    let approved = core
        .change_ledger()
        .approve(
            &ready.name,
            &ready.etag,
            &Identity::user("maintainer"),
            "approved".to_string(),
        )
        .expect("approve change");
    let applied = core
        .apply_change_record(&approved.name, &approved.etag, "review-policy", None)
        .expect("apply reviewed change");

    // Assert
    assert!(matches!(
        blocked,
        Err(CoreError::ChangeLedger(
            crate::change_record::ChangeLedgerError::FailedPrecondition(_)
        ))
    ));
    assert_eq!(applied.status, ChangeRecordStatus::Applied);
}

// ── Immutable serving ────────────────────────────────────────────────────────

#[test]
fn repository_serving_policy_can_disable_one_artifact_kind() {
    // Arrange
    let config = RepoConfig {
        serving_policy: ServingPolicy {
            source: false,
            descriptors: true,
            generated_code: true,
        },
        ..RepoConfig::default()
    };
    let core = core_with_repository_config(config);
    seed_user_message(&core);
    let revision = core
        .resolve_schema_revision(
            PROJECT,
            REPO,
            &RefSpec::bookmark("main"),
            "branch:main".to_string(),
            None,
        )
        .expect("resolve revision");

    // Act
    let source = core.get_schema_artifact(
        &revision.name,
        SCHEMA,
        SchemaArtifactKind::Source,
        None,
        &CodegenOptions::default(),
        None,
    );
    let descriptors = core.get_schema_artifact(
        &revision.name,
        SCHEMA,
        SchemaArtifactKind::Descriptors,
        None,
        &CodegenOptions::default(),
        None,
    );

    // Assert
    assert!(matches!(
        source,
        Err(CoreError::Repository(RepositoryError::FailedPrecondition(
            _
        )))
    ));
    assert!(descriptors.is_ok());
}

#[test]
fn resolved_revision_keeps_serving_same_source_after_bookmark_moves() {
    // Arrange
    let core = core();
    let initial_commit = seed_user_message(&core);
    let revision = core
        .resolve_schema_revision(
            PROJECT,
            REPO,
            &RefSpec::bookmark("main"),
            "branch:main".to_string(),
            None,
        )
        .expect("resolve immutable revision");
    let before = core
        .get_schema_artifact(
            &revision.name,
            SCHEMA,
            SchemaArtifactKind::Source,
            None,
            &CodegenOptions::default(),
            None,
        )
        .expect("serve initial source");
    core.apply_mutation(request(
        "main",
        ProtoOp::CreateMessage(OpCreateMessage {
            message_name: "Later".to_string(),
        }),
    ))
    .expect("move main with later schema");

    // Act
    let after = core
        .get_schema_artifact(
            &revision.name,
            SCHEMA,
            SchemaArtifactKind::Source,
            None,
            &CodegenOptions::default(),
            None,
        )
        .expect("serve pinned source again");
    let latest = core
        .resolve_schema_revision(
            PROJECT,
            REPO,
            &RefSpec::bookmark("main"),
            "branch:main".to_string(),
            None,
        )
        .expect("resolve latest revision");

    // Assert
    assert_eq!(revision.commit_id, initial_commit);
    assert_eq!(before.content, after.content);
    assert_eq!(before.artifact_digest, after.artifact_digest);
    assert_eq!(before.closure_digest, after.closure_digest);
    assert_ne!(latest.commit_id, revision.commit_id);
    assert!(!String::from_utf8(after.content.to_vec())
        .unwrap()
        .contains("Later"));
}

#[test]
fn descriptor_artifact_has_reproducible_payload_and_closure_digests() {
    // Arrange
    let core = core();
    seed_user_message(&core);
    let revision = core
        .resolve_schema_revision(
            PROJECT,
            REPO,
            &RefSpec::bookmark("main"),
            "branch:main".to_string(),
            None,
        )
        .expect("resolve revision");

    // Act
    let first = core
        .get_schema_artifact(
            &revision.name,
            SCHEMA,
            SchemaArtifactKind::Descriptors,
            None,
            &CodegenOptions::default(),
            None,
        )
        .expect("first descriptor artifact");
    let second = core
        .get_schema_artifact(
            &revision.name,
            SCHEMA,
            SchemaArtifactKind::Descriptors,
            None,
            &CodegenOptions::default(),
            None,
        )
        .expect("second descriptor artifact");

    // Assert
    assert!(!first.content.is_empty());
    assert_eq!(first.content, second.content);
    assert_eq!(first.artifact_digest, second.artifact_digest);
    assert_eq!(first.closure_digest, second.closure_digest);
    assert!(first.artifact_digest.starts_with("sha256:"));
    assert!(first.closure_digest.starts_with("sha256:"));
}

#[test]
fn first_materialized_descriptor_survives_redb_restart_without_a_renderer() {
    // Arrange
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("schemahub.redb");
    let (revision_name, expected_content, expected_digest) = {
        let db: Arc<dyn ObjectDb> = Arc::new(RedbObjectDb::open(&db_path).expect("open redb"));
        let core = core_over_db(db);
        seed_user_message(&core);
        let revision = core
            .resolve_schema_revision(
                PROJECT,
                REPO,
                &RefSpec::bookmark("main"),
                "branch:main".to_string(),
                None,
            )
            .expect("resolve before restart");
        let artifact = core
            .get_schema_artifact(
                &revision.name,
                SCHEMA,
                SchemaArtifactKind::Descriptors,
                None,
                &CodegenOptions::default(),
                None,
            )
            .expect("serve before restart");
        (revision.name, artifact.content, artifact.artifact_digest)
    };
    let db: Arc<dyn ObjectDb> =
        Arc::new(RedbObjectDb::open(&db_path).expect("reopen redb after restart"));
    // An empty compiler registry simulates a release in which the original
    // renderer is unavailable. A cache miss would fail before producing bytes.
    let restarted = Core::with_config(
        Arc::new(Jj::new(db)),
        CompilerRegistry::new(),
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        RepoConfigStore::new(),
    );

    // Act
    let restored = restarted
        .get_schema_artifact(
            &revision_name,
            SCHEMA,
            SchemaArtifactKind::Descriptors,
            None,
            &CodegenOptions::default(),
            None,
        )
        .expect("serve after restart");

    // Assert
    assert_eq!(restored.content, expected_content);
    assert_eq!(restored.artifact_digest, expected_digest);
}

#[test]
fn first_materialized_generated_code_survives_redb_restart_without_a_renderer() {
    // Arrange
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("schemahub.redb");
    let (revision_name, expected_content, expected_digest) = {
        let db: Arc<dyn ObjectDb> = Arc::new(RedbObjectDb::open(&db_path).expect("open redb"));
        let core = core_over_db(db);
        seed_user_message(&core);
        let revision = core
            .resolve_schema_revision(
                PROJECT,
                REPO,
                &RefSpec::bookmark("main"),
                "branch:main".to_string(),
                None,
            )
            .expect("resolve before restart");
        let artifact = core
            .get_schema_artifact(
                &revision.name,
                SCHEMA,
                SchemaArtifactKind::GeneratedCode,
                Some(schemahub_types::Language::Rust),
                &CodegenOptions::default(),
                None,
            )
            .expect("serve before restart");
        (revision.name, artifact.content, artifact.artifact_digest)
    };
    let db: Arc<dyn ObjectDb> =
        Arc::new(RedbObjectDb::open(&db_path).expect("reopen redb after restart"));
    let restarted = Core::with_config(
        Arc::new(Jj::new(db)),
        CompilerRegistry::new(),
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        RepoConfigStore::new(),
    );

    // Act
    let restored = restarted
        .get_schema_artifact(
            &revision_name,
            SCHEMA,
            SchemaArtifactKind::GeneratedCode,
            Some(schemahub_types::Language::Rust),
            &CodegenOptions::default(),
            None,
        )
        .expect("serve after restart without a renderer");

    // Assert
    assert_eq!(restored.content, expected_content);
    assert_eq!(restored.artifact_digest, expected_digest);
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
fn op_log_limit_returns_the_latest_operation() {
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
    let expected_id = core
        .jj()
        .list_operations(PROJECT, REPO)
        .expect("full operation log")
        .last()
        .expect("latest operation")
        .op_id
        .clone();

    // Act
    let ops = core
        .op_log(PROJECT, REPO, Some(1), None)
        .expect("bounded op log");

    // Assert
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].op_id, expected_id);
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
fn commit_listing_filters_to_commits_that_touched_the_requested_schema() {
    // Arrange
    let core = core();
    let user_commit = seed_user_message(&core);
    let newest_commit = seed_proto_schema(
        &core,
        PROJECT,
        REPO,
        "main",
        "other.proto",
        "syntax=\"proto3\"; message Other {}",
    );

    // Act
    let (entries, at_commit) = core
        .list_commits_resolved(
            PROJECT,
            REPO,
            Some(&RefSpec::bookmark("main")),
            None,
            Some(SCHEMA),
            100,
            None,
        )
        .expect("filter commit history by schema");

    // Assert
    assert_eq!(at_commit, newest_commit);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].commit_id, user_commit);
}

#[test]
fn commit_listing_stops_exclusively_at_a_retained_ancestor() {
    // Arrange
    let core = core();
    let stop_commit = seed_user_message(&core);
    let middle = seed_proto_schema(
        &core,
        PROJECT,
        REPO,
        "main",
        "middle.proto",
        "syntax=\"proto3\"; message Middle {}",
    );
    let newest = seed_proto_schema(
        &core,
        PROJECT,
        REPO,
        "main",
        "newest.proto",
        "syntax=\"proto3\"; message Newest {}",
    );

    // Act
    let (entries, at_commit) = core
        .list_commits_resolved(
            PROJECT,
            REPO,
            Some(&RefSpec::bookmark("main")),
            Some(&stop_commit),
            None,
            100,
            None,
        )
        .expect("stop commit history at ancestor");

    // Assert
    assert_eq!(at_commit, newest);
    assert_eq!(
        entries
            .into_iter()
            .map(|entry| entry.commit_id)
            .collect::<Vec<_>>(),
        vec![newest, middle]
    );
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
        base_revision: None,
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
