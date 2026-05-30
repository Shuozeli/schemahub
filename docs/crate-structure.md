# schemahub — Crate Structure (v2)

> The Rust workspace layout for the v2 architecture (`design.md`): a format-agnostic **VCS layer** (jj-lib over a database) and per-format **compilers** (wrapping `protobuf-rs` / `flatbuffers-rs`; OpenAPI in-tree). The crate graph enforces the two-layer boundary at the type-system level — the VCS layer cannot call into a compiler except through the `Compiler` trait.
>
> Supersedes v1 (preserved in git history). The principal changes: `schemahub-plugin-*` → `schemahub-compiler-*` (wrapping the sibling compilers), `schemahub-storage` is reworked into `schemahub-vcs` (jj-lib backends over a database), and `FormatPlugin` → `Compiler`.

---

## 1. Crate Inventory

| Crate | Kind | Purpose |
|-------|------|---------|
| `schemahub-types` | lib | Shared types + the `Compiler` trait and auth traits; the boundary between VCS and compilers |
| `schemahub-vcs` | lib | jj-lib integration: `DbBackend` + `DbOpStore` over an `ObjectDb`; `redb`/`postgres` impls |
| `schemahub-core` | lib | Orchestration: mutation/transaction flows, exploration, codegen closure, compatibility, conflict + GC over `schemahub-vcs` and the compiler registry |
| `schemahub-api` | lib | Generated Rust types from the `.proto` files (tonic/prost output) |
| `schemahub-compiler-protobuf` | lib | Protobuf compiler — wraps `protobuf-rs` (parser, schema, codegen); owns the `.proto` printer |
| `schemahub-compiler-flatbuffers` | lib | FlatBuffers compiler — wraps `flatbuffers-rs` (parser, schema, codegen); owns the `.fbs` printer |
| `schemahub-compiler-openapi` | lib | OpenAPI compiler — in-tree AST/parser/printer (no sibling compiler) |
| `schemahub-server` | binary | gRPC server; the composition root that wires everything |
| `schemahub-cli` | binary | CLI client; talks to the server over gRPC |

Nine crates. Only `schemahub-server` and `schemahub-cli` produce binaries.

---

## 2. Dependency Graph

```
                         schemahub-types
                  (Compiler trait, DeclBlob/MetaBlob,
                   ParsedSchema, Mutation, auth traits, errors)
                                  │
        ┌──────────────┬──────────┴───────────┬───────────────────────┐
        ▼              ▼                      ▼                       ▼
 schemahub-vcs   schemahub-compiler-   schemahub-compiler-     schemahub-compiler-
 (jj-lib +        protobuf              flatbuffers             openapi
  ObjectDb:       (wraps protobuf-rs)   (wraps flatbuffers-rs)  (in-tree)
  redb|postgres)
        │
        ▼
  schemahub-core
  (mutation/txn flows, exploration,
   codegen closure, conflict, GC,
   compiler registry)
        │
        ├───────────────────────┐
        ▼                       ▼
  schemahub-api          (all three compilers)
  (generated gRPC types)
        │
   ┌────┴─────┐
   ▼          ▼
schemahub-  schemahub-
server      cli
```

**Constraints enforced by the graph:**

- The compiler crates depend **only** on `schemahub-types` (+ their sibling compiler crate). They know nothing about the VCS, the database, or each other.
- `schemahub-vcs` depends on `schemahub-types` and `jj-lib`; it knows the `DeclBlob`/`MetaBlob` types are opaque bytes, nothing more.
- `schemahub-core` depends on `schemahub-vcs` and `schemahub-types` — the VCS interface and the `Compiler` trait, but no concrete compiler.
- Compilers are injected into the core at startup by `schemahub-server` (the composition root) — the only crate depending on all three compilers.
- `schemahub-cli` depends on `schemahub-api` only — a pure network client.

---

## 3. What Lives Where

### 3.1 `schemahub-types`

Type definitions and trait interfaces only.

```
schemahub-types/src/
  lib.rs
  blob.rs          # DeclBlob, MetaBlob (Vec<u8> newtypes)
  schema_path.rs   # SchemaPath = (project, repo, schema_name)
  parsed.rs        # ParsedSchema { meta, decls }, SchemaObjects, SchemaClosure
  mutation.rs      # Mutation (typed op envelope), MutationEffect
  change.rs        # DeclChange (diff result)
  compiler.rs      # Compiler trait (design.md §2)
  conflict.rs      # ConflictSides, ConflictError
  auth.rs          # AuthnProvider, AuthzPolicy, Identity, Action; NoopAuthn/NoopAuthz
  compat.rs        # CompatibilityRules, CompatibilityDirection, CompatibilityViolation
  decl.rs          # DeclSummary, DeclDetail, DeclKind, TypeRef
  import.rs        # Import { path, resolved_commit }
  language.rs      # Language enum
  errors.rs        # Parse/Print/Diff/Mutation/Read/Descriptor/Codegen/Conflict/Authn/Authz errors
```

The `Compiler` trait (design.md §2) and the no-op auth impls live here. **Dependencies:** `bytes` only.

### 3.2 `schemahub-vcs`

The jj-lib integration and database persistence — this is where v1's `schemahub-storage` + `schemahub-core/version_control` are reworked.

```
schemahub-vcs/src/
  lib.rs
  object_db.rs     # ObjectDb trait (content-addressed object store + op-log tables)
  redb_db.rs       # RedbObjectDb: ObjectDb   (embedded default)
  postgres_db.rs   # PgObjectDb: ObjectDb      (server option)
  backend.rs       # DbBackend: jj_lib::backend::Backend over ObjectDb
  op_store.rs      # DbOpStore: jj_lib::op_store::OpStore over ObjectDb
  repo.rs          # repo loader / transaction helpers (load_at_head, begin_tx, commit_op)
  tree.rs          # mapping schema files ↔ jj trees; per-declaration file entries (§4.2)
  conflict.rs      # conflict (de)construction at DeclBlob granularity
  bookmark.rs      # bookmark create/move/list, protected-bookmark matching
```

`schemahub-vcs` exposes a schemahub-shaped API (load a repo, read a declaration at a ref, run a transaction that upserts/removes declarations and moves a bookmark, list operations, undo) and hides jj-lib behind it. **Dependencies:** `schemahub-types`, `jj-lib`, `redb`, `tokio-postgres`/`sqlx` (feature-gated), `bytes`.

### 3.3 `schemahub-core`

Orchestration over the VCS and the compiler registry. No `main.rs`.

```
schemahub-core/src/
  lib.rs
  registry.rs        # HashMap<format_id, Arc<dyn Compiler>>
  mutation/
    mod.rs
    single.rs        # single-mutation flow (design.md §5.1)
    transaction.rs   # transaction flow (design.md §5.2)
    idempotency.rs   # RPC-edge idempotency dedupe
    compat.rs        # compatibility orchestration (protected-bookmark gating)
    closure.rs       # transitive import closure BFS (codegen)
  conflict.rs        # render/resolve conflicted declarations via the compiler
  exploration.rs     # ListDeclarations, GetDeclaration, FollowType, ListDependencies, Search
  codegen.rs         # GetDescriptors, PreviewCodegen
  history.rs         # log (commits/changes), op log, diff
  gc.rs              # GC via jj-lib (op-log + reachable objects)
  auth.rs            # AuthnProvider + AuthzPolicy invocation in flows
```

**Dependencies:** `schemahub-types`, `schemahub-vcs`.

### 3.4 `schemahub-api`

Generated gRPC types (unchanged from v1 in role). `build.rs` runs `tonic_build` over `proto/schemahub/v1/*.proto`; nothing checked in. New/changed protos vs v1: a conflict-resolution RPC and an operation-log / `undo` RPC (in a revised `admin_service.proto` / a new `history_service.proto`); responses carry `change_id` alongside `commit_id`.

**Dependencies:** `tonic`, `prost`, `prost-types`. No schemahub crates.

### 3.5 Compiler Crates

Each wraps its sibling compiler and adds the schemahub-specific pieces (decl split, printer, mutation validation, compatibility, conflict rendering).

```
schemahub-compiler-protobuf/src/
  lib.rs           # ProtobufCompiler; Compiler impl
  parse.rs         # protoc-rs-parser::parse → split FileDescriptorProto into ParsedSchema
  printer.rs       # FileDescriptorProto (+ source_code_info) → canonical .proto   (NEW; no upstream printer)
  blob.rs          # encode/decode DeclBlob (DescriptorProto/EnumDescriptorProto/ServiceDescriptorProto) + MetaBlob
  diff.rs          # diff_decl
  compat.rs        # check_compatibility (design.md §3.1 tables)
  mutations.rs     # apply_mutation/apply_mutations against the real descriptor
  conflict.rs      # render_conflict / validate_resolution
  codegen.rs       # FileDescriptorSet assembly → protoc-rs-codegen::generate_rust
```

`schemahub-compiler-flatbuffers` mirrors this over `flatbuffers-rs` (`flatc-rs-parser`, `flatc-rs-schema`, `flatc-rs-codegen`; printer is in-tree). `schemahub-compiler-openapi` keeps its in-tree AST/parser/printer (`openapi-ast.md`) and the whole-document v1 mutation surface.

**Dependencies:**
- protobuf: `schemahub-types`, `protoc-rs-parser`, `protoc-rs-schema`, `protoc-rs-codegen`, `prost`.
- flatbuffers: `schemahub-types`, `flatc-rs-parser`, `flatc-rs-schema`, `flatc-rs-codegen`.
- openapi: `schemahub-types`, `serde`, `serde_yaml`/`serde_json`.

The sibling compiler crates are referenced as **path dependencies** within the `~/projects/shuozeli/compilers/` tree (or git deps once published).

### 3.6 `schemahub-server`

The composition root.

```
schemahub-server/src/
  main.rs            # startup: config, open ObjectDb, build registry, start tonic
  config.rs          # schemahub.toml (db backend choice, protected bookmarks, bootstrap roles)
  wire.rs            # schemahub-api types ↔ schemahub-core types
  services/
    schema.rs        # SchemaService
    bookmark.rs      # bookmarks/tags (was ref_service)
    exploration.rs   # ExplorationService
    codegen.rs       # CodegenService
    project.rs       # ProjectService
    history.rs       # log / op log / undo / resolve-conflict
    admin.rs         # GC, rebuild index
  error.rs           # core errors → tonic::Status
```

```rust
// main.rs — startup sketch
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_file("schemahub.toml")?;

    let db: Arc<dyn ObjectDb> = match config.storage.backend {
        Backend::Redb => Arc::new(RedbObjectDb::open(&config.storage.path)?),
        Backend::Postgres => Arc::new(PgObjectDb::connect(&config.storage.url).await?),
    };
    let vcs = Arc::new(Vcs::new(db));            // DbBackend + DbOpStore inside

    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    registry.register(Arc::new(FlatBuffersCompiler::new()));
    registry.register(Arc::new(OpenApiCompiler::new()));

    let core = Arc::new(Core::new(vcs, registry, Arc::new(NoopAuthn), Arc::new(NoopAuthz)));

    Server::builder()
        .add_service(SchemaServiceServer::new(SchemaHandler::new(core.clone())))
        // ... bookmark, exploration, codegen, project, history, admin
        .serve(config.listen_addr)
        .await?;
    Ok(())
}
```

**Dependencies:** `schemahub-core`, `schemahub-vcs`, `schemahub-api`, all three compiler crates, `tonic`, `tokio`, `serde`, `toml`, `anyhow`.

### 3.7 `schemahub-cli`

Pure gRPC client.

```
schemahub-cli/src/
  main.rs
  config.rs          # ~/.schemahub/config + .schemahub project file
  format.rs          # extension → format
  output.rs          # DeclDetail bytes → human-readable per format
  client.rs          # channel setup, auth header injection
  commands/
    schema.rs        # create / update / pull / delete
    field.rs message.rs enum_.rs service.rs   # granular mutations
    bookmark.rs tag.rs                          # bookmarks/tags
    log.rs op_log.rs undo.rs resolve.rs diff.rs # jj-style history/recovery
    import_.rs codegen.rs search.rs project.rs
```

**Dependencies:** `schemahub-api`, `clap`, `tonic`, `tokio`, `serde`, `toml`, `anyhow`.

---

## 4. Workspace `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
    "crates/schemahub-types",
    "crates/schemahub-vcs",
    "crates/schemahub-core",
    "crates/schemahub-api",
    "crates/schemahub-compiler-protobuf",
    "crates/schemahub-compiler-flatbuffers",
    "crates/schemahub-compiler-openapi",
    "crates/schemahub-server",
    "crates/schemahub-cli",
]

[workspace.dependencies]
tokio       = { version = "1", features = ["full"] }
tonic       = "0.12"
tonic-build = "0.12"
prost       = "0.13"
prost-types = "0.13"
jj-lib      = "*"          # pin to a vendored version (see design.md §4.6)
redb        = "2"
bytes       = "1"
serde       = { version = "1", features = ["derive"] }
toml        = "0.8"
clap        = { version = "4", features = ["derive"] }
anyhow      = "1"
thiserror   = "1"

# sibling compilers (path deps within ~/projects/shuozeli/compilers/)
protoc-rs-parser  = { path = "../../compilers/protobuf-rs/parser" }
protoc-rs-schema  = { path = "../../compilers/protobuf-rs/schema" }
protoc-rs-codegen = { path = "../../compilers/protobuf-rs/codegen" }
flatc-rs-parser   = { path = "../../compilers/flatbuffers-rs/parser" }
flatc-rs-schema   = { path = "../../compilers/flatbuffers-rs/schema" }
flatc-rs-codegen  = { path = "../../compilers/flatbuffers-rs/codegen" }
```

> Path deps assume the post-2026-05-13 layout: schemahub at `~/projects/shuozeli/codegen/schemahub`, compilers at `~/projects/shuozeli/compilers/`. Confirm the relative paths against the actual workspace root before building.

---

## 5. Third-Party / Sibling Libraries

| Concern | Library | Notes |
|---------|---------|-------|
| Protobuf parse + AST + codegen | `protobuf-rs` (`protoc-rs-parser`, `protoc-rs-schema`, `protoc-rs-codegen`) | Sibling project; conformance-tested. **No printer** — schemahub writes it. |
| FlatBuffers parse + AST + codegen | `flatbuffers-rs` (`flatc-rs-parser`, `flatc-rs-schema`, `flatc-rs-codegen`) | Sibling project. **No printer** — schemahub writes it. |
| OpenAPI parse | `serde_yaml` / `serde_json` into the in-tree AST | No sibling compiler. |
| Version control model | `jj-lib` | Commits, change IDs, conflicts, op-log; we implement `Backend` + `OpStore`. |
| Object/op-log persistence | `redb` (default) or `postgres` | Behind the `ObjectDb` trait. |

**Rule:** a compiler never exposes its sibling crate's types across the `Compiler` boundary — only `DeclBlob`/`MetaBlob` (serialized AST) cross it. This insulates the VCS layer from sibling-crate version churn and keeps blob versioning under schemahub's control.

---

## 6. Testing Strategy

- `schemahub-types`: trait object-safety of `dyn Compiler`.
- `schemahub-vcs`: object round-trip, transaction atomicity, op-log + undo, conflict construction at decl granularity, against both `redb` and (gated) `postgres`.
- `schemahub-core`: mutation/transaction flows with a mock `Compiler` and an in-memory `ObjectDb`; concurrency producing first-class conflicts; protected-bookmark gating.
- `schemahub-compiler-*`: **round-trip** (`parse → print → parse` yields an equivalent AST — the headline test the v1 AST could not pass), diff correctness, one compatibility test per rule-table row, decl-split/reassemble fidelity.
- Integration (`schemahub-server/tests/`): in-process server over an ephemeral `redb`, driven via gRPC — end-to-end mutation, conflict-and-resolve, undo, codegen.

---

## 7. Key Design Decisions

**Why `schemahub-vcs` is its own crate (not folded into core):** the jj-lib integration and database persistence are a self-contained concern with a heavy dependency (`jj-lib`). Isolating it keeps `schemahub-core` testable against a mock VCS and lets the database backend (`redb`/`postgres`) vary without touching orchestration logic.

**Why compilers wrap the siblings instead of re-parsing:** the sibling compilers already have correct, conformance-tested ASTs (nested types, labels, options, oneofs, presence) that the v1 hand-rolled AST got wrong. Re-implementing them is both wasted effort and a correctness regression. The only format-specific code schemahub must own is the *printer* (round-trip) and the *mutation/compat/conflict* logic — none of which the siblings provide.

**Why `schemahub-types` stays separate:** it breaks the `core ↔ compiler` dependency cycle (both depend on `types`, neither on the other) — the standard Rust shared-trait-boundary pattern, unchanged from v1.

**Why the CLI is a pure client:** ship/build the CLI against a running server without compiling the server stack, jj-lib, the database, or the compilers.
