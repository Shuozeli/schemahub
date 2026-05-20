# schemahub — Crate Structure

> This document defines the Rust workspace layout: what crates exist, what each owns, and how they depend on each other. The goal is a structure that enforces the two-layer architecture from `design.md` at the Rust type system level — the compiler should make it impossible to accidentally call a format-specific function from the core layer.

---

## 1. Crate Inventory

| Crate | Kind | Purpose |
|-------|------|---------|
| `schemahub-types` | lib | Shared types and trait definitions; the trait boundary between core and plugins |
| `schemahub-storage` | lib | `StorageBackend` trait + `redb` implementation |
| `schemahub-core` | lib | Version control logic, mutation dispatch, GC, plugin registry |
| `schemahub-api` | lib | Generated Rust types from the `.proto` files (tonic/prost output) |
| `schemahub-plugin-protobuf` | lib | Protobuf format plugin |
| `schemahub-plugin-flatbuffers` | lib | FlatBuffers format plugin |
| `schemahub-plugin-openapi` | lib | OpenAPI format plugin |
| `schemahub-server` | binary | gRPC server; wires everything together |
| `schemahub-cli` | binary | CLI client; talks to the server over gRPC |

Nine crates total. The format plugins, core, and storage are all libraries. Only `schemahub-server` and `schemahub-cli` produce binaries.

---

## 2. Dependency Graph

```
                    schemahub-types
                    (FormatPlugin trait,
                     Blob, Mutation,
                     auth traits, errors)
                         │
          ┌──────────────┼──────────────────────────┐
          │              │                           │
          ▼              ▼                           ▼
schemahub-storage  schemahub-plugin-protobuf   schemahub-plugin-flatbuffers
(StorageBackend    (ProtobufPlugin impl)        (FlatBuffersPlugin impl)
 trait + redb)
          │                                    schemahub-plugin-openapi
          │                                    (OpenApiPlugin impl)
          ▼
   schemahub-core
   (version control,
    mutation dispatch,
    GC, plugin registry)
          │
          ├─────────────────────┐
          ▼                     ▼
  schemahub-api          (all three plugins)
  (generated gRPC
   types from proto)
          │
    ┌─────┴──────┐
    ▼            ▼
schemahub-    schemahub-
server        cli
```

**Key constraint enforced by the graph:**

- The plugin crates depend **only** on `schemahub-types` — they know nothing about storage, the other plugins, or the core.
- `schemahub-core` depends on `schemahub-storage` and `schemahub-types` — it knows about the storage interface and the plugin trait, but not any specific plugin implementation.
- The plugins are injected into the core at startup by `schemahub-server`, which is the only crate that depends on all three plugins simultaneously.
- `schemahub-cli` depends on `schemahub-api` (for the generated gRPC client stubs) but **not** on `schemahub-core`, `schemahub-storage`, or any plugin — it is a pure network client.

---

## 3. What Lives Where

### 3.1 `schemahub-types`

The lowest-level crate. No business logic — only type definitions and trait interfaces.

```
schemahub-types/src/
  lib.rs
  blob.rs          # Blob (Vec<u8> newtype), Hash (SHA-256 newtype)
  schema_path.rs   # SchemaPath = (project, repo, schema_name)
  mutation.rs      # Mutation { schema_path, format_id, operation: Bytes }
  schema_change.rs # SchemaChange enum (DeclarationAdded/Removed/Modified)
  plugin.rs        # FormatPlugin trait (the core abstraction)
  auth.rs          # AuthnProvider trait, AuthzPolicy trait, Identity, Action
  compat.rs        # CompatibilityRules, CompatibilityDirection, CompatibilityViolation
  decl.rs          # DeclSummary, DeclDetail, DeclKind
  import.rs        # Import { path, resolved_commit, decl_name }
  language.rs      # Language enum (Rust, Go, TypeScript, ...)
  errors.rs        # ParseError, PrintError, DiffError, MutationError,
                   # ReadError, DescriptorError, CodegenError, AuthnError, AuthzError
```

Key types:

```rust
// blob.rs
pub struct Blob(pub Vec<u8>);
pub struct Hash([u8; 32]);  // SHA-256

// plugin.rs — the FormatPlugin trait (from design.md Section 2)
pub trait FormatPlugin: Send + Sync + 'static {
    fn format_id(&self) -> &'static str;
    fn parse(&self, source: &str) -> Result<Blob, ParseError>;
    fn print(&self, blob: &Blob) -> Result<String, PrintError>;
    fn diff(&self, old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError>;
    fn apply_mutation(&self, blob: &Blob, mutation: &Mutation) -> Result<Blob, MutationError>;
    fn apply_mutations(&self, blobs: &HashMap<SchemaPath, Blob>, mutations: &[Mutation])
        -> Result<HashMap<SchemaPath, Blob>, MutationError>;
    fn check_compatibility(&self, old: &Blob, new: &Blob, rules: &CompatibilityRules)
        -> Result<(), Vec<CompatibilityViolation>>;
    fn list_declarations(&self, blob: &Blob) -> Result<Vec<DeclSummary>, ReadError>;
    fn get_declaration(&self, blob: &Blob, name: &str) -> Result<DeclDetail, ReadError>;
    fn imports(&self, blob: &Blob) -> Result<Vec<Import>, ReadError>;
    fn generate_descriptors(&self, blobs: &HashMap<SchemaPath, Blob>)
        -> Result<Bytes, DescriptorError>;
    fn generate_code(&self, blobs: &HashMap<SchemaPath, Blob>, language: Language)
        -> Result<String, CodegenError>;
}

// auth.rs
pub trait AuthnProvider: Send + Sync + 'static {
    fn identify(&self, metadata: &RequestMetadata) -> Result<Identity, AuthnError>;
}
pub trait AuthzPolicy: Send + Sync + 'static {
    fn check(&self, caller: &Identity, action: Action, resource: &ResourcePath)
        -> Result<(), AuthzError>;
}
// No-op implementations ship here for getting-started convenience:
pub struct NoopAuthn;
pub struct NoopAuthz;
```

**Dependencies:** none beyond `std` and `bytes`.

### 3.2 `schemahub-storage`

The storage abstraction and its `redb` implementation.

```
schemahub-storage/src/
  lib.rs
  backend.rs     # StorageBackend trait
  redb.rs        # RedbBackend: StorageBackend
  keys.rs        # KV namespace key builders (all the key formatting logic lives here)
  objects.rs     # Object read/write helpers (Blob, Tree, Commit, Tag)
  refs.rs        # refs/ namespace read/write helpers
  index.rs       # index/ and deps/ namespace helpers
  search.rs      # search/ namespace helpers
  idempotency.rs # idempotency/ namespace helpers
  pending.rs     # pending/ namespace helpers
  roles.rs       # roles/ namespace helpers
```

The `StorageBackend` trait is deliberately narrow — it exposes typed operations rather than raw key-value primitives. This prevents the core from accidentally bypassing the key naming conventions.

```rust
// backend.rs
pub trait StorageBackend: Send + Sync + 'static {
    // Objects
    fn write_object(&self, hash: &Hash, data: &[u8]) -> Result<(), StorageError>;
    fn read_object(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StorageError>;
    fn object_exists(&self, hash: &Hash) -> Result<bool, StorageError>;

    // Refs
    fn set_ref(&self, project: &str, repo: &str, ref_name: &str, commit: &Hash)
        -> Result<(), StorageError>;
    fn get_ref(&self, project: &str, repo: &str, ref_name: &str)
        -> Result<Option<Hash>, StorageError>;
    fn list_refs(&self, project: &str, repo: &str, prefix: &str)
        -> Result<Vec<(String, Hash)>, StorageError>;
    fn delete_ref(&self, project: &str, repo: &str, ref_name: &str)
        -> Result<(), StorageError>;

    // Atomic CAS on a ref (used for branch advancement)
    fn compare_and_set_ref(
        &self, project: &str, repo: &str, ref_name: &str,
        expected: &Hash, new: &Hash,
    ) -> Result<bool, StorageError>;  // false = CAS failed (conflict)

    // Transactional write (all or nothing)
    fn write_transaction(&self, ops: Vec<StorageOp>) -> Result<(), StorageError>;

    // Prefix scan (for search/, deps/, index/)
    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StorageError>;
    fn delete_prefix(&self, prefix: &str) -> Result<u64, StorageError>;
    // ... additional operations for idempotency, pending, roles
}
```

**Dependencies:** `schemahub-types`, `redb`, `prost` (for serializing Tree/Commit/Tag objects).

### 3.3 `schemahub-core`

The business logic layer. Knows about the `FormatPlugin` trait and the `StorageBackend` trait, but not about any specific plugin or the redb backend.

```
schemahub-core/src/
  lib.rs
  plugin_registry.rs     # HashMap<format_id, Arc<dyn FormatPlugin>>
  version_control/
    mod.rs
    commit.rs            # commit creation, Tree building
    branch.rs            # create, delete, list branches
    tag.rs               # create, delete, list tags
    merge.rs             # fast-forward merge check and execution
    diff.rs              # diff between two VersionRefs
    log.rs               # commit history traversal
  mutation/
    mod.rs
    single.rs            # single-mutation flow (10 steps from design.md)
    transaction.rs       # transaction flow (13 steps from design.md)
    idempotency.rs       # idempotency key check and storage
    occ.rs               # base_revision CAS check
    compat.rs            # compatibility check orchestration
    bfs.rs               # transitive import closure (for GetDescriptors)
  gc.rs                  # RunGC implementation
  index.rs               # RebuildIndex implementation
  exploration.rs         # ListDeclarations, GetDeclaration, FollowType, Search
  codegen.rs             # GetDescriptors, PreviewCodegen (calls bfs.rs + plugin)
  auth/
    mod.rs
    middleware.rs        # AuthnProvider + AuthzPolicy call in request flows
```

This crate does NOT have a `main.rs` — it is a library only. The gRPC handler glue lives in `schemahub-server`.

**Dependencies:** `schemahub-types`, `schemahub-storage`.

### 3.4 `schemahub-api`

Generated Rust types from the `.proto` files. Contains all the gRPC server traits, client stubs, and request/response message types.

```
schemahub-api/
  Cargo.toml
  build.rs               # calls tonic_build::configure().compile_protos(...)
  src/
    lib.rs               # tonic::include_proto!("schemahub.v1") calls
  proto/                 # symlink or copy of proto/ at workspace root
                         # (or build.rs points to workspace-root proto/)
```

The `build.rs` generates code at compile time. No generated files are checked into the repository — they land in `OUT_DIR`.

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                "proto/schemahub/v1/schema_service.proto",
                "proto/schemahub/v1/ref_service.proto",
                "proto/schemahub/v1/exploration_service.proto",
                "proto/schemahub/v1/codegen_service.proto",
                "proto/schemahub/v1/project_service.proto",
                "proto/schemahub/v1/admin_service.proto",
            ],
            &["proto/", "vendor/googleapis/"],  // googleapis for google.rpc.Status
        )?;
    Ok(())
}
```

**Dependencies:** `tonic`, `prost`, `prost-types`. No schemahub crates — the generated types are self-contained.

### 3.5 Format Plugin Crates

Each plugin follows the same internal structure. Using Protobuf as the example:

```
schemahub-plugin-protobuf/src/
  lib.rs               # ProtobufPlugin struct; FormatPlugin impl entry point
  parser/
    mod.rs             # source text → ProtoAst
    lexer.rs
    grammar.rs
  ast/
    mod.rs
    message.rs         # MessageBlob, FieldDef (with prost::Message derives)
    enum_.rs           # EnumBlob, EnumValueDef
    service.rs         # ServiceBlob, RpcDef
    file.rs            # FileMetadataBlob
    types.rs           # FieldType enum, scalar types
  printer.rs           # ProtoAst → .proto source text
  diff.rs              # diff(old_blob, new_blob) → Vec<SchemaChange>
  compat.rs            # check_compatibility per the allowlist in design.md 4.2
  mutations/
    mod.rs
    field.rs           # AddField, RemoveField, RenameField, ChangeFieldType, ...
    message.rs         # AddMessage, RemoveMessage, RenameMessage
    enum_.rs           # AddEnum, AddEnumValue, ...
    service.rs         # AddService, AddRpc, ...
    import.rs          # UpdateImport
  codegen/
    descriptors.rs     # generate_descriptors → FileDescriptorSet
    rust.rs            # generate_code for Rust
    go.rs              # generate_code for Go
    # ... other languages
  migrations.rs        # static MIGRATIONS chain (blob_version upgrades)
  blob.rs              # encode/decode helpers: Blob ↔ MessageBlob (via prost)
```

The AST types (`MessageBlob`, `EnumBlob`, etc.) use `#[derive(prost::Message)]` directly in Rust — no separate `.proto` file is needed for the internal blob encoding:

```rust
// ast/message.rs
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessageBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "3")]
    pub fields: Vec<FieldDef>,
    #[prost(message, repeated, tag = "4")]
    pub reserved_numbers: Vec<ReservedRange>,
    #[prost(string, repeated, tag = "5")]
    pub reserved_names: Vec<String>,
    #[prost(string, optional, tag = "6")]
    pub doc_comment: Option<String>,
}
```

**`schemahub-plugin-flatbuffers`** and **`schemahub-plugin-openapi`** have the same structure with format-specific parsers and AST types. The OpenAPI plugin's AST types match the definitions in `openapi-ast.md`.

**Dependencies (all plugin crates):** `schemahub-types`, `prost`, `bytes`. Plus format-specific parsing libraries (see Section 6).

### 3.6 `schemahub-server`

The gRPC server binary. The only crate that wires all layers together.

```
schemahub-server/src/
  main.rs              # startup: parse config, build registry, start tonic server
  config.rs            # schemahub.toml parsing (serde + toml)
  wire.rs              # conversion between schemahub-api types and schemahub-core types
  services/
    schema.rs          # SchemaServiceServer impl
    ref_.rs            # RefServiceServer impl
    exploration.rs     # ExplorationServiceServer impl
    codegen.rs         # CodegenServiceServer impl
    project.rs         # ProjectServiceServer impl
    admin.rs           # AdminServiceServer impl
  error.rs             # map internal errors → tonic::Status (with rich detail encoding)
```

The handler implementations are thin: they call `wire.rs` to convert request types, then call into `schemahub-core`, then convert results back.

```rust
// main.rs — startup sketch
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_file("schemahub.toml")?;

    let storage = Arc::new(RedbBackend::open(&config.storage.path)?);

    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(ProtobufPlugin::new()));
    registry.register(Arc::new(FlatBuffersPlugin::new()));
    registry.register(Arc::new(OpenApiPlugin::new()));

    let authn = Arc::new(NoopAuthn);   // or load configured impl
    let authz = Arc::new(NoopAuthz);

    let core = Arc::new(Core::new(storage, registry, authn, authz));

    Server::builder()
        .add_service(SchemaServiceServer::new(SchemaHandler::new(core.clone())))
        .add_service(RefServiceServer::new(RefHandler::new(core.clone())))
        .add_service(ExplorationServiceServer::new(ExplorationHandler::new(core.clone())))
        .add_service(CodegenServiceServer::new(CodegenHandler::new(core.clone())))
        .add_service(ProjectServiceServer::new(ProjectHandler::new(core.clone())))
        .add_service(AdminServiceServer::new(AdminHandler::new(core.clone())))
        .serve(config.listen_addr)
        .await?;

    Ok(())
}
```

**Dependencies:** `schemahub-core`, `schemahub-storage` (for `RedbBackend`), `schemahub-api`, all three plugin crates, `tonic`, `tokio`, `serde`, `toml`, `anyhow`.

### 3.7 `schemahub-cli`

The CLI binary. A pure gRPC client — depends on `schemahub-api` for the generated client stubs but nothing from the server side.

```
schemahub-cli/src/
  main.rs
  config.rs            # ~/.schemahub/config and .schemahub project file parsing
  format.rs            # file extension → SchemaFormat inference
  output.rs            # DeclDetail bytes → human-readable text for each format
  client.rs            # gRPC channel setup, auth header injection
  commands/
    schema.rs          # schema create / update / pull / delete
    field.rs           # field add / remove / rename
    message.rs         # message rename
    branch.rs          # branch create / list / delete
    tag.rs             # tag create / list / delete
    log.rs             # log
    diff.rs            # diff
    merge.rs           # merge
    import_.rs         # import update
    codegen.rs         # codegen get / preview
    search.rs          # search
    project.rs         # project / repo / member management
```

**Dependencies:** `schemahub-api`, `clap`, `tonic`, `tokio`, `serde`, `toml`, `anyhow`.

---

## 4. Workspace `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
    "crates/schemahub-types",
    "crates/schemahub-storage",
    "crates/schemahub-core",
    "crates/schemahub-api",
    "crates/schemahub-plugin-protobuf",
    "crates/schemahub-plugin-flatbuffers",
    "crates/schemahub-plugin-openapi",
    "crates/schemahub-server",
    "crates/schemahub-cli",
]

# Shared dependency versions across the workspace.
[workspace.dependencies]
# Async runtime
tokio       = { version = "1", features = ["full"] }
# gRPC
tonic       = "0.12"
tonic-build = "0.12"
# Protobuf encoding (both API types and internal blob encoding)
prost       = "0.13"
prost-types = "0.13"
# Storage
redb        = "2"
# Byte buffers
bytes       = "1"
# Serialization (config files)
serde       = { version = "1", features = ["derive"] }
toml        = "0.8"
# CLI argument parsing
clap        = { version = "4", features = ["derive"] }
# Error handling
anyhow      = "1"
thiserror   = "1"
# SHA-256 for content addressing
sha2        = "0.10"
```

All crates reference versions from `[workspace.dependencies]` using `{ workspace = true }` to ensure consistency.

---

## 5. Directory Layout

```
schemahub/
  Cargo.toml                    ← workspace root
  Cargo.lock
  proto/
    schemahub/v1/
      common.proto
      mutations.proto
      schema_service.proto
      ref_service.proto
      exploration_service.proto
      codegen_service.proto
      project_service.proto
      admin_service.proto
  vendor/
    googleapis/                 ← google.rpc.Status proto (git subtree or submodule)
  crates/
    schemahub-types/
    schemahub-storage/
    schemahub-core/
    schemahub-api/
    schemahub-plugin-protobuf/
    schemahub-plugin-flatbuffers/
    schemahub-plugin-openapi/
    schemahub-server/
    schemahub-cli/
  docs/
    requirements.md
    design.md
    open-questions.md
    openapi-ast.md
    grpc-api.md
    crate-structure.md          ← this file
```

---

## 6. Third-Party Parsing Libraries

Each plugin needs to parse its format's source text. Options:

| Format | Library | Notes |
|--------|---------|-------|
| Protobuf | [`protox`](https://crates.io/crates/protox) or hand-rolled | `protox` is a pure-Rust proto3 compiler; alternatively write a PEG parser with `pest` |
| FlatBuffers | [`flatc-rust`](https://crates.io/crates/flatc-rust) or hand-rolled | No mature pure-Rust parser exists; a `pest` grammar is the realistic v1 path |
| OpenAPI | [`oas3`](https://crates.io/crates/oas3) or [`openapiv3`](https://crates.io/crates/openapiv3) | `openapiv3` parses into serde structs; plugin converts to schemahub AST |

**Decision rule:** Prefer a library for parsing (turning text into a structured representation), but always convert from the library's types into schemahub's own AST types. The plugin never exposes library types across its boundary — it exposes only `Blob` (serialized schemahub AST). This insulates the core from library version churn and ensures the AST is under schemahub's control for blob versioning and migration.

---

## 7. Testing Strategy

### Unit tests (inside each crate)

- `schemahub-types`: trait object safety tests (can `dyn FormatPlugin` be used as expected)
- `schemahub-storage`: KV round-trip tests, CAS correctness, prefix scan
- `schemahub-core`: mutation flow steps tested with a mock `StorageBackend` and mock `FormatPlugin`
- `schemahub-plugin-*`: parser round-trip (parse → print → parse must produce identical AST), diff correctness, compatibility checker (one test per row of the compat table), migration chain (golden file tests per migration step)

### Integration tests (`crates/schemahub-server/tests/`)

Full-stack tests that spin up a real server (in-process, ephemeral redb store) and drive it via gRPC client calls. Cover the end-to-end mutation flows, idempotency, OCC conflicts, and GC.

### Architectural test

A `#[test]` in `schemahub-core` that asserts the crate has no direct dependency on any plugin crate — enforced by attempting to use a format-specific type and expecting a compile error. In practice, this is enforced by the dependency graph itself; `cargo deny` can be used to catch accidental dependency additions.

---

## 8. Key Design Decisions

**Why `schemahub-types` is separate from `schemahub-core`:**
Without a separate types crate, the plugin crates would need to depend on the core — creating a cycle (`core → plugin → core`). The types crate breaks the cycle: both `core` and `plugins` depend on `types`, but not on each other. This is the standard Rust pattern for shared trait boundaries.

**Why `schemahub-api` has no schemahub dependencies:**
The generated gRPC types (`prost::Message` structs, tonic service traits) are independent of schemahub's internal model. Keeping `schemahub-api` free of schemahub dependencies means the CLI can depend on just `schemahub-api` without pulling in the entire server-side stack. It also makes the generated code easier to publish as a standalone client library later.

**Why the server wires everything in `main.rs` (not in core):**
`schemahub-core` takes `Arc<dyn FormatPlugin>` values — it doesn't know which plugins exist. The server is the composition root: it creates the concrete plugin instances and injects them. This is dependency injection via the type system, and it means that tests of `schemahub-core` can inject a minimal mock plugin without involving any real format-specific code.

**Why there is no `schemahub-common` or `schemahub-utils`:**
Generic utility crates tend to accumulate unrelated code over time, making them a coupling point. Each crate owns the utilities it needs. If a utility is genuinely needed in multiple places, it should be added to `schemahub-types` with an explicit justification.

**Why the CLI does not depend on `schemahub-core`:**
The CLI is a network client, not an embedded library. It should be possible to build and ship the CLI binary against a running server without compiling the server-side logic. This also means the CLI binary stays small — it does not link in redb, the format parsers, or the business logic.
