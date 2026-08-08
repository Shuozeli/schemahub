<!-- agent-updated: 2026-07-30T04:16:42Z -->
# schemahub — Crate Structure (v2)

> The Rust workspace layout for the v2 architecture (`design.md`): a format-agnostic **JJ layer** (jj-lib over a database) and per-format **compilers** (wrapping `protobuf-rs` / `flatbuffers-rs`; OpenAPI in-tree). The crate graph enforces the two-layer boundary at the type-system level — the JJ layer cannot call into a compiler except through the `Compiler` trait.
>
> Supersedes v1 (preserved in git history). The principal changes: `schemahub-plugin-*` → `schemahub-compiler-*` (wrapping the sibling compilers), `schemahub-storage` is reworked into `schemahub-jj` (jj-lib backends over a database), and `FormatPlugin` → `Compiler`.

---

## 1. Crate Inventory

| Crate | Kind | Purpose |
|-------|------|---------|
| `schemahub-types` | lib | Shared types + the `Compiler` trait and auth traits; the boundary between JJ and compilers |
| `schemahub-jj` | lib | jj-lib integration plus control-plane and immutable-artifact records over an `ObjectDb`; `redb`/`postgres` impls |
| `schemahub-core` | lib | Orchestration: durable change records, mutation/transaction flows, first-materialized immutable serving, exploration, codegen closure, compatibility, conflict + GC |
| `schemahub-api` | lib | Generated Rust types from the `.proto` files (tonic/prost output) |
| `schemahub-compiler-protobuf` | lib | Protobuf compiler — wraps `protobuf-rs` (parser, schema, codegen); owns the `.proto` printer |
| `schemahub-compiler-flatbuffers` | lib | FlatBuffers compiler — wraps `flatbuffers-rs` (parser, schema, codegen); owns the `.fbs` printer |
| `schemahub-compiler-openapi` | lib | OpenAPI compiler — fail-closed in-tree AST/parser/printer (no sibling compiler) |
| `schemahub-server` | binary | gRPC/HTTP server; the composition root that wires compilers, persistence, BFF, operations, and optional same-origin GUI assets |
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
 schemahub-jj   schemahub-compiler-   schemahub-compiler-     schemahub-compiler-
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

- The compiler crates depend **only** on `schemahub-types` (+ their sibling compiler crate). They know nothing about the JJ layer, the database, or each other.
- `schemahub-jj` depends on `schemahub-types` and `jj-lib`; it knows the `DeclBlob`/`MetaBlob` types are opaque bytes, nothing more.
- `schemahub-core` depends on `schemahub-jj` and `schemahub-types` — the JJ interface and the `Compiler` trait, but no concrete compiler.
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
  decl.rs          # DeclSummary, DeclDetail, DeclKind, TypeRef (+ optional import coordinate)
  import.rs        # Import { path, resolved_commit }
  language.rs      # Language enum
  errors.rs        # Parse/Print/Diff/Mutation/Read/Descriptor/Codegen/Conflict/Authn/Authz errors
```

The `Compiler` trait (design.md §2) and the no-op auth impls live here. **Dependencies:** `bytes` only.

### 3.2 `schemahub-jj`

The jj-lib integration and database persistence — this is where v1's `schemahub-storage` + `schemahub-core/version_control` are reworked.

```
schemahub-jj/src/
  lib.rs            # Jj handle plus narrow object_db() seam for durable non-JJ records
  object_db.rs      # ObjectDb trait (objects/op-log/refs/records + repo inventory + GC fence)
  redb_db.rs        # RedbObjectDb: ObjectDb (embedded default; single-file MVCC store)
  memory_db.rs      # MemoryObjectDb: ObjectDb (in-memory, for tests)
  pg_db.rs          # PgObjectDb: migrations, fixed SQLx executor, advisory GC/publication fences
  jj_backend.rs     # DbBackend: jj_lib::backend::Backend over ObjectDb
  jj_op_store.rs    # DbOpStore: jj_lib::op_store::OpStore over ObjectDb
  jj_op_heads.rs    # DbOpHeadsStore: jj_lib::op_heads_store::OpHeadsStore (durable head pointer)
  repo.rs           # repo loader / Store (per-Jj dedicated tokio current-thread runtime)
  bookmark.rs       # protected-bookmark glob matching (`is_protected`)
  tests.rs          # in-repo unit tests against MemoryObjectDb/RedbObjectDb
schemahub-jj/migrations/
  202607210001_initial_schema.sql # adoption-safe PostgreSQL baseline
```

Per-declaration storage lives in `lib.rs` directly: `decl_path` / `encode_decl_name` map a `(<schema>, <decl>)` pair to a single jj path component (percent-encoding `%` and `/`), and `__meta__` is the well-known entry name for the file `MetaBlob` within each schema subtree. No separate `tree.rs` / `conflict.rs` files: jj inlines conflicts as multi-side tree entries, and the `ConflictId` / `ObjectKind::Conflict` types in `object_db.rs` are retained as shims but unused (see `DECISIONS.md`).

`schemahub-jj` exposes a schemahub-shaped API (load a repo, read a declaration at a ref, run a transaction that upserts/removes declarations and moves a bookmark, list commits/operations, filter schema-touch history, and undo) and hides jj-lib behind it. Every raw commit is proven reachable from the named repository's current or historical operation views before any globally deduplicated object is read or published. Stored JJ records validate required fields and exact ID lengths, and distinguish true absence from backend faults. Bounded operation-log reads walk only the requested suffix on linear histories and fall back to the full graph algorithm when a branch enters that suffix. Immutable tree reads additionally expose lexicographical schema-name pages that stop after one page plus lookahead without loading declaration blobs, plus exact conflict statistics grouped only for a bounded caller-selected schema set. `ObjectDb` also provides create/get/list/compare-and-swap plus bounded stable range reads for control-plane resources such as `ChangeRecord` and the immutable `schemahub.artifacts.v1` first-materialization collection; `transact_records` atomically combines distinct-key create/CAS/delete mutations so resource state cannot split from an audit or list-index entry. ChangeRecord creation/status indexes live in these collections, are backfilled once behind a durable marker, and never enter JJ's content-addressed namespace. `Jj::object_db()` exposes a cloned trait-object handle so Core shares the exact backing database without depending on a concrete backend. Repository inventory and shared/exclusive maintenance guards make global GC safe across deduplicated repositories and PostgreSQL server instances. A second exclusive repository publication guard spans op-head load, final-tree policy, and JJ commit; memory/redb use a mutex and PostgreSQL uses a repository-keyed advisory lock, preserving concurrency between different repositories. `PublicationSnapshot` exposes schema-shaped reads of the exact candidate tree to Core without leaking jj-lib types. **Dependencies:** `schemahub-types`, `jj-lib` (default features off — no git interop), `redb`, `sqlx` (optional, behind `postgres`), `bytes`, `pollster`, `async-trait`, `tokio`, `futures`, `tempfile`, `blake2`, `prost`, `serde`, `serde_json`, `uuid`, `sha2`, `hex`, `thiserror`. Features: `postgres` (compile `PgObjectDb`), `postgres-integration` (run real-Postgres tests, requires `SCHEMAHUB_TEST_POSTGRES_URL`).

Dashboard inventory uses `Jj::load_schemas`: one immutable tree traversal
loads only the selected page's blobs while collecting all repository-local
schema names for import normalization. `Core::summarize_schema_inventory_at`
then compiler-validates those declarations and counts unique declared direct
imports without calling one full-tree schema load per row.

### 3.3 `schemahub-core`

Orchestration over the JJ and the compiler registry. No `main.rs`.

```
schemahub-core/src/
  lib.rs
  config.rs          # RepoConfig (default_bookmark, direction, protected_bookmarks) + RepoConfigStore
  error.rs           # CoreError + CoreResult (wraps JJ / auth / compiler errors)
  registry.rs        # CompilerRegistry: HashMap<format_id, Arc<dyn Compiler>>
  request.rs         # Lifecycle/mutation/transaction requests, deadline token, limits, and read response shapes
  lifecycle.rs       # whole-schema create/update/delete policy and source replacement
  reference_integrity.rs # final-state live-unpinned import checks
  change_record/     # lifecycle ledger plus atomic creation/status indexes and bounded page store
  changes.rs         # authorized Create/Get/bounded List/Update/Abandon orchestration
  mutation/
    mod.rs
    single.rs        # single-mutation flow (design.md §5.1)
    transaction.rs   # bounded/deadline-aware transaction flow (multi-file via one JJ commit)
    idempotency.rs   # bounded ObjectDb receipts + JJ crash reconciliation
    compat.rs        # compatibility orchestration (protected-bookmark gating)
    closure.rs       # transitive import closure BFS (codegen)
  conflict.rs        # render/resolve conflicted declarations via the compiler
  exploration.rs     # immutable local reads, exact FollowType, forward closure, bounded ListDependents
  codegen.rs         # GetDescriptors, PreviewCodegen
  serving.rs         # immutable revisions + versioned first-materialization records/digests
  control_plane_audit.rs # typed immutable admin events, injected clock/IDs, bounded indexed reads
  history.rs         # immutable log/list ranges, schema-touch filtering, op log, undo, repository diff
  refs.rs            # bookmark + tag orchestration (create/delete/direct get/bounded pages, merge wrappers)
  projects.rs        # project/member CRUD + bounded catalogs, caller-role lookup, coordination
  repository.rs      # durable repository resources, per-project catalog pages, CAS/archive/runtime policy
  gc.rs              # GC via jj-lib (op-log + reachable objects)
  auth.rs            # AuthnProvider + AuthzPolicy invocation in flows
  auth_store.rs      # ProjectStore + RoleStore traits, ETags/timestamps/archive metadata
  auth_object_db.rs  # project/role records, bounded member ranges, active/all catalog indexes/backfill
  auth_files.rs      # legacy JSON migration readers and compatibility tests
  auth_impls.rs      # BearerTokenAuthn (token → Identity) + RoleBasedAuthz
  tests.rs           # in-repo unit tests using a mock Compiler + MemoryObjectDb
```

**Dependencies:** `schemahub-types`, `schemahub-jj`, `serde` + `serde_json`
(control-plane resource encoding), `bytes` (Compiler-trait re-export), `sha2`,
`hex`, `uuid`, `thiserror`. Dev-deps: `schemahub-compiler-protobuf` (only for
tests), `tempfile`.

### 3.4 `schemahub-api`

Generated gRPC types. `build.rs` runs `tonic_build` over
`proto/schemahub/v1/*.proto`; nothing generated is checked in. The build script
selects `protoc-bin-vendored` for the host platform so clean CI, container, and
cross-platform release builds do not depend on a system `protoc` executable.
`change_service.proto` owns the durable intent resource and lifecycle,
`serving_service.proto` owns immutable revisions and artifacts, and
`history_service.proto` owns the operation log, undo, and conflict resolution;
schema writes carry `change_id` alongside `commit_id`.

**Dependencies:** `tonic`, `prost`, `prost-types`; build dependencies are
`tonic-build` and `protoc-bin-vendored`. No schemahub crates.

### 3.5 Compiler Crates

Each wraps its sibling compiler and adds the schemahub-specific pieces (decl
split, printer, exact field/property type lookup, mutation validation,
compatibility, conflict rendering).

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

Sibling compiler crates must resolve through canonical Git URLs at immutable
commit revisions. Protobuf is pinned at
`a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac`; FlatBuffers is pinned at
`59756d23993538b722f68675c35129c3cebb7aa1`. That published FlatBuffers
revision is warning-clean for SchemaHub's generated-code contract and passed
the sibling compiler's complete GitHub Actions matrix.

Protobuf closure codegen keeps the compiler adapter format-specific: it maps
the sibling generator's package-keyed outputs into one deterministic nested
Rust module tree, imports only resolved cross-package roots, and re-exports the
explicitly requested root package. Multi-file generation without a root fails
closed instead of guessing from map or lexical order.

### 3.6 `schemahub-server`

The composition root.

```
schemahub-server/src/
  main.rs            # startup/probe: args (--print-openapi/--check-ready/--listen/--http-listen/--gui-dir/--db/--db-url/--config),
                     # open ObjectDb, build Core, start tonic; binds to
                     # TAILSCALE_IP:port when set, else 0.0.0.0:50051.
  lib.rs             # composition root + release-tag BUILD_VERSION; exposes
                     # build_core(), build_core_with_authn(), and build_router().
  config.rs          # schemahub.toml: storage/listen, exact-origin/body/GUI HTTP
                     # policy, repos/projects, and mutually exclusive static/JWT auth.
  jwt_auth.rs        # strict JWT verifier, injected clock, bounded HTTPS/file
                     # JWKS loader, atomic rotation, stale-key readiness task.
  wire.rs            # schemahub-api types ↔ schemahub-core types
  error.rs           # core errors → tonic::Status
  http.rs            # bounded same-origin BFF, scoped project/repo page DTOs,
                     # explicit SPA/static serving, annotated handlers, probes/metrics
  http/openapi.rs    # shared runtime-route/OpenAPI assembly and bearer metadata
  observability.rs   # shared HTTP/gRPC counters and latency histogram
  services/
    mod.rs           # token_from() helper (Authorization: Bearer <token>)
    schema.rs        # SchemaService adapter; transaction blocking worker + hard server deadline
    bookmark.rs      # RefService — commits, diff, bounded branch/tag pages, direct branch get, merge
    exploration.rs   # ExplorationService; blocking reverse-dependency scan adapter
    codegen.rs       # CodegenService (GetDescriptors, PreviewCodegen)
    serving.rs       # ServingService (resolve immutable revision + fetch artifacts)
    change.rs        # ChangeService (durable edits, validation, review, Apply)
    project.rs       # durable Project/Repo/member lifecycle plus bound opaque pagination
    history.rs       # HistoryService — Log, OpLog, Undo, Render/Resolve conflict
    admin.rs         # AdminService — GC/index/config/capabilities and public limits
```

The matching React client lives outside the Rust crate graph under
`apps/schemahub-gui`. Its project/repository hooks retain incremental page
DTOs, while `apps/browser-cdp.mjs` is shared by the GUI and workflow-demo
browser smokes to normalize remote Chrome discovery onto the configured CDP
host.

```rust
// main.rs — startup sketch
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_file("schemahub.toml")?;

    let db: Arc<dyn ObjectDb> = match config.storage.backend {
        Backend::Redb => Arc::new(RedbObjectDb::open(&config.storage.path)?),
        Backend::Postgres => Arc::new(PgObjectDb::connect(&config.storage.url)?),
    };
    // JWT mode initializes its JWKS asynchronously and injects the provider;
    // noop/static mode uses the synchronous build_core convenience path.
    let jwt_runtime = match &config.auth.jwt {
        Some(jwt) => Some(JwtAuthRuntime::initialize(jwt).await?),
        None => None,
    };
    let core = match jwt_runtime.as_ref() {
        Some(runtime) => build_core_with_authn(db, &config, runtime.provider()),
        None => build_core(db, &config),
    };
    // The real composition root supervises jwt_runtime.run() beside both
    // listeners and gives every task the same graceful-shutdown signal.
    build_router(core, config.storage.backend.as_str())
        .serve(config.listen_addr)
        .await?;
    Ok(())
}
```

**Dependencies:** `schemahub-core`, `schemahub-jj`, `schemahub-api`, all three
compiler crates, `tonic`, `tokio`, `serde`, `toml`, `anyhow`, `jsonwebtoken`,
`reqwest`, `axum`, `tower-http` (CORS, request IDs, tracing, and filesystem
serving), `utoipa`, and `utoipa-axum`.

### 3.7 `schemahub-cli`

Pure gRPC client.

```
schemahub-cli/src/
  main.rs            # clap entrypoint; subcommands: repo / project / change / artifact /
                     # schema / field / branch / tag / log / op / undo / resolve / codegen / diff
  config.rs          # ~/.schemahub config; reads --server/--token or env vars
  client.rs          # tonic Channel + Authorization header attachment
  cmd/
    mod.rs           # parse_ref: "@hex"→Commit, "tag:N"→Tag, else Branch
    repo.rs          # `repo init` (project + repo creation)
    project.rs       # `project create` + paged `project member {list,add,remove,set-role}` (RBAC)
    change.rs        # draft edits + validate/ready/review/apply/abandon; stable --json output
    artifact.rs      # immutable `artifact resolve/fetch/verify`; stable metadata JSON
    schema.rs        # `schema create/update/pull/delete`
    field.rs         # Protobuf granular field ops: add/remove/rename
    branch.rs        # `branch create/delete/list/merge`
    tag.rs           # `tag create/delete/list`
    log.rs           # `log` — RefService.ListCommits
    history.rs       # `op log`, `undo`, `resolve` — HistoryService
    codegen.rs       # `codegen get/preview`
```

There is no `message` / `enum` / `service` CLI subcommand yet (those `ApplyMutation` ops exist on the wire only). No top-level `search`, `import`, or `op` subcommands beyond `op log`.

**Dependencies:** `schemahub-api`, `clap` (with `derive` and `env` features),
`tonic`, `tokio`, `prost`, `prost-types`, `serde`, `serde_json`, `toml`,
`anyhow`, `bytes`, `sha2`, `hex`, `uuid`.

---

## 4. Workspace `Cargo.toml`

Reflects the actual root `Cargo.toml` on `v2-rearchitecture`.

```toml
[workspace]
resolver = "2"
members = [
    "crates/schemahub-types",
    "crates/schemahub-jj",
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
# jj-lib is the real Jujutsu JJ library; schemahub-jj implements its
# Backend + OpStore traits over our ObjectDb (see schemahub-jj/DECISIONS.md).
jj-lib      = { version = "0.41", default-features = false }
pollster    = "0.4"
async-trait = "0.1"
futures     = "0.3"
tempfile    = "3"
redb        = "2"
bytes       = "1"
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
serde_yaml  = "0.9"
toml        = "0.8"
clap        = { version = "4", features = ["derive", "env"] }
anyhow      = "1"
thiserror   = "1"
sha2        = "0.10"
hex         = "0.4"
uuid        = { version = "1", features = ["v4"] }
# Postgres backend for schemahub-jj (optional, behind the `postgres` feature).
sqlx        = { version = "0.9", default-features = false,
                features = ["postgres", "runtime-tokio", "tls-rustls", "macros"] }

# sibling compilers (immutable Git revisions for independent checkouts)
protoc-rs-parser  = { git = "https://github.com/Shuozeli/protobuf-rs.git", rev = "a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac" }
protoc-rs-schema  = { git = "https://github.com/Shuozeli/protobuf-rs.git", rev = "a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac" }
protoc-rs-codegen = { git = "https://github.com/Shuozeli/protobuf-rs.git", rev = "a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac" }

flatc-rs-parser   = { git = "https://github.com/Shuozeli/flatbuffers-rs.git", rev = "59756d23993538b722f68675c35129c3cebb7aa1" }
flatc-rs-schema   = { git = "https://github.com/Shuozeli/flatbuffers-rs.git", rev = "59756d23993538b722f68675c35129c3cebb7aa1" }
flatc-rs-codegen  = { git = "https://github.com/Shuozeli/flatbuffers-rs.git", rev = "59756d23993538b722f68675c35129c3cebb7aa1" }
```

`jj-lib` is pinned at `0.41` with `default-features = false` so the `git` interop feature is off (we bypass git — see `design.md` §4.6). `sqlx` is workspace-declared but only depended on through the optional `postgres` feature on `schemahub-jj` (and forwarded into `schemahub-server`).

> Both sibling compiler boundaries are immutable Git dependencies. Independent
> SchemaHub checkouts do not require the local `~/projects/shuozeli/compilers/`
> layout.

---

## 5. Third-Party / Sibling Libraries

| Concern | Library | Notes |
|---------|---------|-------|
| Protobuf parse + AST + codegen | `protobuf-rs` (`protoc-rs-parser`, `protoc-rs-schema`, `protoc-rs-codegen`) | Sibling project; conformance-tested. **No printer** — schemahub writes it. |
| FlatBuffers parse + AST + codegen | `flatbuffers-rs` (`flatc-rs-parser`, `flatc-rs-schema`, `flatc-rs-codegen`) | Sibling project. **No printer** — schemahub writes it. |
| OpenAPI parse | `serde_yaml` / `serde_json` into the in-tree AST | No sibling compiler. |
| Version control model | `jj-lib` | Commits, change IDs, conflicts, op-log; we implement `Backend` + `OpStore`. |
| Object/op-log persistence | `redb` (default) or `postgres` | Behind the `ObjectDb` trait. |

**Rule:** a compiler never exposes its sibling crate's types across the `Compiler` boundary — only `DeclBlob`/`MetaBlob` (serialized AST) cross it. This insulates the JJ layer from sibling-crate version churn and keeps blob versioning under schemahub's control.

---

## 6. Testing Strategy

- `schemahub-types`: trait object-safety of `dyn Compiler`.
- `schemahub-jj`: object round-trip, transaction atomicity, op-log + undo, conflict construction at decl granularity, against both `redb` and (gated) `postgres`.
- `schemahub-core`: mutation/transaction flows with a mock `Compiler` and an
  in-memory `ObjectDb`; unprotected concurrency producing first-class conflicts;
  atomic protected-conflict and delete/import race rejection, including
  ChangeRecord lease cleanup; deadline cancellation before publication and
  receipt cleanup while queued at the publication guard; direct reverse
  discovery with immutable per-repository snapshots and auth filtering;
  forward live/pinned closure resolution; exact cross-format field-type
  traversal; immutable history/diff snapshots; and raw-commit ownership.
- `schemahub-compiler-*`: **round-trip** (`parse → print → parse` yields an equivalent AST — the headline test the v1 AST could not pass), diff correctness, one compatibility test per rule-table row, decl-split/reassemble fidelity.
- Integration (`schemahub-server/tests/`): in-process server over an ephemeral
  `redb`, driven via gRPC — end-to-end mutation, conflict-and-resolve, undo,
  codegen, serving, and live/pinned cross-repository dependency discovery
  (`e2e_dependencies.rs`).

---

## 7. Key Design Decisions

**Why `schemahub-jj` is its own crate (not folded into core):** the jj-lib integration and database persistence are a self-contained concern with a heavy dependency (`jj-lib`). Isolating it keeps `schemahub-core` testable against a mock JJ and lets the database backend (`redb`/`postgres`) vary without touching orchestration logic.

**Why compilers wrap the siblings instead of re-parsing:** the sibling compilers already have correct, conformance-tested ASTs (nested types, labels, options, oneofs, presence) that the v1 hand-rolled AST got wrong. Re-implementing them is both wasted effort and a correctness regression. The only format-specific code schemahub must own is the *printer* (round-trip) and the *mutation/compat/conflict* logic — none of which the siblings provide.

**Why `schemahub-types` stays separate:** it breaks the `core ↔ compiler` dependency cycle (both depend on `types`, neither on the other) — the standard Rust shared-trait-boundary pattern, unchanged from v1.

**Why the CLI is a pure client:** ship/build the CLI against a running server without compiling the server stack, jj-lib, the database, or the compilers.
