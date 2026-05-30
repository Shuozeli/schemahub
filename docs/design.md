# schemahub — Design (v2: Compilers + Jujutsu-style VCS)

> This document specifies *how* schemahub is built. It supersedes the v1 git-style design (preserved in git history). Two project-level decisions drive this revision:
>
> 1. **Compilers, not bespoke parsers.** Protobuf and FlatBuffers are fronted by the sibling compiler projects `protobuf-rs` and `flatbuffers-rs`; OpenAPI is in-tree. schemahub does not hand-roll format parsers or ASTs.
> 2. **Jujutsu-style VCS over a database.** The version-control layer uses the `jj-lib` model — content-addressed commits, stable change IDs, first-class conflicts, and an operation log with undo — with **custom backends that persist to a database** (not jj's on-disk git/file layout).
>
> Where this document names a `jj-lib` type or method, it follows jj-lib's model; exact signatures are pinned to the vendored jj-lib version during implementation (see §4.6 and Open Question 11 in `requirements.md`).

---

## 1. Architecture Overview

schemahub is two layers with a single trait boundary between them.

```
┌──────────────────────────── VCS layer (format-agnostic) ────────────────────────────┐
│                                                                                      │
│   jj-lib model:  commits + change IDs · operation log + undo · first-class conflicts │
│                  bookmarks · merge/rebase · auto-rebase of descendants               │
│                                                                                      │
│   custom Backend  (objects: commits, trees, per-declaration blobs, conflicts)        │
│   custom OpStore  (operation log + views)            ──────►  database (redb | pg)   │
│                                                                                      │
│   project/repo/schema namespacing · auth traits · compatibility orchestration        │
└────────────────────────────────────────┬─────────────────────────────────────────────┘
                                          │  Compiler trait
        ┌─────────────────────────────────┼─────────────────────────────────┐
   ┌────▼─────────────┐          ┌─────────▼──────────┐          ┌───────────▼─────────┐
   │ protobuf compiler │          │ flatbuffers compiler│          │  openapi compiler   │
   │  wraps protobuf-rs │          │  wraps flatbuffers-rs│          │   (in-tree AST)     │
   │                    │          │                      │          │                     │
   │  parse (reuse)     │          │  parse (reuse)       │          │  parse              │
   │  AST = FileDescr…  │          │  AST = Schema        │          │  AST (openapi-ast)  │
   │  print  (NEW)      │          │  print  (NEW)        │          │  print              │
   │  split/merge decls │          │  split/merge decls   │          │  split/merge        │
   │  diff · compat     │          │  diff · compat       │          │  diff · compat      │
   │  mutate · conflict │          │  mutate · conflict   │          │  (whole-doc) compat │
   │  codegen (reuse)   │          │  codegen (reuse)     │          │  descriptors        │
   └────────────────────┘          └──────────────────────┘          └─────────────────────┘
```

**The VCS layer knows nothing about Protobuf, FlatBuffers, or OpenAPI.** It stores opaque per-declaration blobs and delegates all format-specific work to a `Compiler` via the trait below. Adding a format (SQL DDL, Thrift) is a new compiler crate; the VCS layer is untouched.

**Key difference from v1:** v1 reduced this to a single `__schema__` blob per file fronted by hand-rolled mini-parsers. v2 fixes both: real compiler ASTs, and genuine per-declaration objects (§4.2) so jj's content-addressing, dedup, and first-class conflicts operate at declaration granularity.

---

## 2. The `Compiler` Trait

This is the boundary between the two layers (v1 called it `FormatPlugin`; renamed to reflect that each implementation *is* a compiler front-end). A `DeclBlob` is the serialized AST of **one top-level declaration**; a `MetaBlob` is the file-level metadata (package, imports, syntax/edition). Both are opaque `Vec<u8>` to the VCS layer.

```rust
pub trait Compiler: Send + Sync + 'static {
    fn format_id(&self) -> &'static str;          // "protobuf" | "flatbuffers" | "openapi"

    // ── Ingest: source text → per-declaration objects ───────────────────────────
    /// Parse source (reusing the sibling compiler), then SPLIT the resulting AST into
    /// one DeclBlob per top-level declaration plus one MetaBlob for the file.
    /// This split is what makes per-declaration storage (§4.2) possible.
    fn parse(&self, source: &str) -> Result<ParsedSchema, ParseError>;

    // ── Egress: per-declaration objects → canonical source (deterministic) ───────
    /// Reassemble decls + meta into the compiler AST and PRINT canonical source.
    /// NOTE: the sibling compilers ship no printer; schemahub owns this per format.
    fn print(&self, schema: &SchemaObjects) -> Result<String, PrintError>;

    // ── Diff ─────────────────────────────────────────────────────────────────────
    fn diff_decl(&self, old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError>;

    // ── Granular mutation (validated against the AST) ────────────────────────────
    /// Apply one typed op to one declaration (and possibly emit edits to others,
    /// e.g. a rename that touches referencing declarations).
    fn apply_mutation(&self, schema: &SchemaObjects, op: &Mutation)
        -> Result<MutationEffect, MutationError>;
    /// Transaction path: apply an ordered batch; only the final state is validated.
    fn apply_mutations(&self, schema: &SchemaObjects, ops: &[Mutation])
        -> Result<MutationEffect, MutationError>;

    // ── Compatibility (per changed declaration) ──────────────────────────────────
    fn check_compatibility(&self, old: &DeclBlob, new: &DeclBlob, rules: &CompatibilityRules)
        -> Result<(), Vec<CompatibilityViolation>>;

    // ── First-class conflicts (§6) ───────────────────────────────────────────────
    /// Render a conflicted declaration (a merge of N sides) for human/agent display.
    fn render_conflict(&self, sides: &ConflictSides) -> Result<String, ConflictError>;
    /// Validate a proposed resolution blob against the conflict (must be a valid decl).
    fn validate_resolution(&self, resolved: &DeclBlob) -> Result<(), ConflictError>;

    // ── Read / exploration ───────────────────────────────────────────────────────
    fn summarize_decl(&self, blob: &DeclBlob) -> Result<DeclSummary, ReadError>;
    fn decl_detail(&self, blob: &DeclBlob) -> Result<DeclDetail, ReadError>;
    fn imports(&self, meta: &MetaBlob) -> Result<Vec<Import>, ReadError>;
    /// The type names a declaration references (for FollowType / rename propagation).
    fn type_refs(&self, blob: &DeclBlob) -> Result<Vec<TypeRef>, ReadError>;

    // ── Codegen (reuse sibling codegen) ──────────────────────────────────────────
    /// Reassemble the transitive closure into the native descriptor artifact.
    fn generate_descriptors(&self, closure: &SchemaClosure) -> Result<Bytes, DescriptorError>;
    fn generate_code(&self, closure: &SchemaClosure, lang: Language)
        -> Result<String, CodegenError>;
}
```

Supporting types (owned by `schemahub-types`):

```rust
/// Output of parse(): the file split into addressable objects.
pub struct ParsedSchema {
    pub meta:  MetaBlob,                       // package, imports, syntax/edition
    pub decls: Vec<(String, DeclBlob)>,        // (declaration name, serialized AST node)
}

/// A schema loaded from storage for mutation/printing: meta + named decls.
pub struct SchemaObjects {
    pub meta:  MetaBlob,
    pub decls: BTreeMap<String, DeclBlob>,     // ordered for deterministic printing
}

/// What a mutation produced: changed/added/removed decls + possibly new meta.
pub struct MutationEffect {
    pub meta:    Option<MetaBlob>,
    pub upserts: Vec<(String, DeclBlob)>,
    pub removes: Vec<String>,
}

pub struct DeclBlob(pub Vec<u8>);  // serialized compiler AST node
pub struct MetaBlob(pub Vec<u8>);
```

### 2.1 Blob encoding

Each compiler serializes its AST nodes with a stable, versioned encoding. Because Protobuf and FlatBuffers ASTs come from the sibling crates, the encoding is `prost`/`serde` over those types (whichever the sibling crate already supports), wrapped with a `blob_version: u32` for migration. The VCS layer never inspects these bytes — it only content-addresses and stores them.

---

## 3. Compiler Implementations

### 3.1 Protobuf compiler (`schemahub-compiler-protobuf`)

Wraps **`protobuf-rs`**:

| Need | Source | Notes |
|------|--------|-------|
| Parse | `protoc-rs-parser::parse(src) -> FileDescriptorProto` (`parser/src/lib.rs:6`) | Use `parse_collecting` to surface all errors. |
| AST | `protoc-rs-schema::FileDescriptorProto` (`schema/src/descriptor.rs:27`) | Complete: nested types, `FieldLabel` (Optional/Required/Repeated), `proto3_optional`, `OneofDescriptorProto`, all `*Options`, reserved ranges/names, `source_code_info` for comments, editions. |
| Codegen | `protoc-rs-codegen::generate_rust(&FileDescriptorSet)` (`codegen/src/lib.rs:7`) | Rust + runtime. FileDescriptorSet assembled from the AST. |
| Print | **in-tree (NEW)** | `protobuf-rs` ships no printer. schemahub renders `FileDescriptorProto` (+ `source_code_info`) → canonical `.proto`. |

**Decl split (`parse`):** a `FileDescriptorProto` splits into one `DeclBlob` per `message_type` / `enum_type` / `service` (each serialized as its `DescriptorProto` / `EnumDescriptorProto` / `ServiceDescriptorProto`), plus a `MetaBlob` holding `package`, `dependency`, `syntax`/`edition`, file `options`, and the slice of `source_code_info` that is not attributable to a single declaration. Nested types stay inside their parent's `DescriptorProto` (they are not separate top-level objects).

**Mutation validator** (now able to target the *real* AST — these were impossible in v1):
- `ChangeCardinality`: sets `FieldLabel` / `proto3_optional` correctly.
- `AddField`/`RemoveField`: field-number + reserved-set rules; `RemoveField` auto-reserves number and name.
- `oneof` operations: operate on `OneofDescriptorProto` + `oneof_index`, preserving order.
- nested-type and options edits: directly on the descriptor.
- `ChangeFieldNumber`: rejected (always breaking).

**Compatibility checker:** same rule tables as v1 §4.2 (add optional field, remove-with-reservation, enum value add/remove, RPC add/remove, the wire-type-compatible type-change allowlist) — but evaluated against the real descriptor, so labels/options participate correctly.

### 3.2 FlatBuffers compiler (`schemahub-compiler-flatbuffers`)

Wraps **`flatbuffers-rs`**:

| Need | Source | Notes |
|------|--------|-------|
| Parse | `flatc-rs-parser::FbsParser::new(src).parse() -> ParseOutput { schema, state }` (`parser/src/lib.rs:6`) | |
| AST | `flatc-rs-schema::Schema` (`schema/src/lib.rs:534`) | `Object` (table/struct via `is_struct`), `Field` (`is_required`/`is_optional`/`is_deprecated`, `id`, attributes), `Enum` (incl. unions), `Service`/`RpcCall`, `Attributes`, `Documentation`, `Namespace`, `Span`. |
| Codegen | `flatc-rs-codegen::generate_rust/typescript/dart(&ResolvedSchema, opts)` (`codegen/src/lib.rs:177`) | Needs the resolved schema. |
| Print | **in-tree (NEW)** | `flatbuffers-rs` ships no printer; schemahub renders `Schema` → canonical `.fbs`. |

**Decl split:** one `DeclBlob` per `Object` (table/struct) and per `Enum` (enum/union); `Service` too. `MetaBlob` holds `file_ident`, `file_ext`, namespace context, includes, root-type designation. Slot indices (`Field.id`) are preserved as wire identity.

**Mutation validator:** `AddField` appends at end only; `RemoveField` rejected (use `DeprecateField`); any struct mutation rejected; reorder rejected — same constraints as v1 §4.3, now against the real `Schema`.

### 3.3 OpenAPI compiler (`schemahub-compiler-openapi`)

No sibling compiler exists, so this stays in-tree. The existing in-tree AST (`docs/openapi-ast.md`) is kept but must satisfy the per-declaration split: one `DeclBlob` per path-group and per component (`schemas`, `parameters`, `responses`, `requestBodies`), with stable addressable paths (the v1 path model in `openapi-ast.md` already targets this). v1 mutation surface remains whole-document via `UpdateSchema`; granular mutations deferred to v2.

---

## 4. VCS Layer (jj-lib over a database)

### 4.1 What the jj model gives us

`jj-lib` provides, as a library:
- **Commits** — immutable, content-addressed (`CommitId`), each carrying a **`ChangeId`** that is stable across rewrite/rebase/squash.
- **Trees & files** — content-addressed directory trees (`TreeId`) whose entries are `TreeValue`s (`File`, `Tree`, `Conflict`, …).
- **First-class conflicts** — a tree entry (or a commit's root tree) can *be* a conflict: a merge of N states, stored rather than rejected.
- **Operation log** — every repository mutation is an `Operation` over a `View` (bookmarks, heads, working-copy pointers). Undo/restore operate on this log.
- **Merge/rebase with auto-rebase of descendants**, bookmarks, and a revset query model.
- **Pluggable persistence** via the `Backend` and `OpStore` traits.

We adopt the model and implement the persistence ourselves.

### 4.2 Object model — per-declaration files in a two-level tree

We map schemahub's namespacing onto jj trees so that **each top-level declaration is its own content-addressed file**:

```
commit.root_tree
  └─ <schema-file>/                         ← one subtree per schema file (jj Tree entry)
       ├─ __meta__         → MetaBlob        ← file-level metadata (jj File entry)
       ├─ UserRequest      → DeclBlob        ← one message  (jj File entry)
       ├─ UserResponse     → DeclBlob        ← one message
       └─ UserStatus       → DeclBlob        ← one enum
```

A repo's root tree contains one subtree per schema file (`user.proto`, `order.proto`, …). Each subtree contains one file per declaration plus `__meta__`. This is the v1 two-level tree **made real**: editing one field rewrites exactly one declaration file and the two trees on its path; every other declaration's content hash is unchanged and dedups. Project/repo namespacing is a layer above (one jj repo per `project/repo`, or a tree prefix — see §4.5).

**Conflict granularity falls out for free:** two clients editing different messages touch different files → automatic merge. Two clients editing the *same* message → a conflict on that one file, stored as a jj conflict (§6).

### 4.3 Custom `Backend` over the database

We implement `jj_lib::backend::Backend` so that all object reads/writes hit the database instead of a git repo or local files:

```rust
// sketch — method set follows jj-lib's Backend trait
impl Backend for DbBackend {
    fn read_file(&self, id: &FileId)  -> BackendResult<Box<dyn Read>>;   // DeclBlob / MetaBlob bytes
    fn write_file(&self, contents: &mut dyn Read) -> BackendResult<FileId>;
    fn read_tree(&self, id: &TreeId)  -> BackendResult<Tree>;
    fn write_tree(&self, tree: &Tree) -> BackendResult<TreeId>;
    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit>;
    fn write_commit(&self, commit: Commit) -> BackendResult<(CommitId, Commit)>;
    fn read_conflict(&self, id: &ConflictId) -> BackendResult<Conflict>;
    fn write_conflict(&self, conflict: &Conflict) -> BackendResult<ConflictId>;
    // gc(), concurrency token, etc.
}
```

Objects are content-addressed (the backend hashes content → id), so dedup is inherent and cross-repo. The DB schema is essentially `objects(kind, id BLOB PRIMARY KEY, bytes BLOB)` plus the op-log tables below.

### 4.4 Custom `OpStore` over the database — the operation log

We implement `jj_lib::op_store::OpStore` to persist operations and views:

```rust
impl OpStore for DbOpStore {
    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation>;  // parents + view + metadata
    fn write_operation(&self, op: &Operation)  -> OpStoreResult<OperationId>;
    fn read_view(&self, id: &ViewId)  -> OpStoreResult<View>;                // bookmarks, heads, wc pointers
    fn write_view(&self, view: &View) -> OpStoreResult<ViewId>;
}
```

**Every schemahub write is one jj operation.** A mutation, a transaction, a bookmark move, a GC run, a role change — each is a `Transaction` that commits to a new `Operation`. This is the audit log (`who/when/what`) and the substrate for `undo` (restore the repo to a prior `OperationId`). It replaces v1's bespoke `pending/` GC roots and idempotency-as-durability machinery.

### 4.5 Database choice

The backend/op-store are written against a small internal `ObjectDb` trait so the concrete database is swappable:

- **`redb` (default, embedded):** single-file, MVCC, zero-ops — ideal for self-hosted/dev and the open-source default.
- **`postgres` (server):** matches the team's standard infra for multi-instance deployments; objects and op-log in tables, content-addressed ids as primary keys.

Project/repo namespacing: each `project/repo` is an independent jj repo (its own op-log and bookmark set), keyed by a `(project, repo)` prefix in the `ObjectDb`. Content objects dedup globally; op-logs are per-repo. (Alternative — one global repo with a project/repo tree prefix — is rejected: per-repo op-logs give per-repo undo and isolation.)

### 4.6 What we use from jj-lib, and what we bypass

**Use:** `Backend`, `OpStore`, `RepoLoader`/`ReadonlyRepo`/`MutableRepo`, `Transaction`, commits/trees/conflicts, change IDs, bookmarks, merge/rebase + auto-rebase, revsets.

**Bypass:** jj's **filesystem working copy** (`LocalWorkingCopy`) and **git interop** (`GitBackend`). There is no checked-out tree on disk; the "working-copy commit" is a *logical* editable tip created directly in a transaction. The CLI/agent edits via RPC, not by writing files.

> Implementation note: jj-lib's API evolves and some types (notably the exact conflict representation — older `Conflict` objects vs. inline merged-tree ids) differ by version. We pin one jj-lib version, vendor it, and adapt these sketches to its concrete signatures. This is Open Question 11.

---

## 5. Mutation API

### 5.1 Single mutation flow

```
1. Idempotency (RPC edge): if this idempotency_key was seen, return stored result. STOP.
2. AuthN: identify caller from request metadata.
3. AuthZ: caller has Write (or Force, if --force) on <project>/<repo>.
4. Load repo at current op:  let repo = repo_loader.load_at_head();
5. Resolve target bookmark → commit → root tree → <schema-file> subtree.
6. Load SchemaObjects (meta + the touched DeclBlobs) from the subtree.
7. effect = compiler.apply_mutation(&schema, &op);
8. If target bookmark is protected and !force: for each changed decl,
      compiler.check_compatibility(old, new, rules)  → CompatibilityError on violation.
9. Start a jj Transaction:
      a. write changed/added DeclBlobs and (if any) __meta__ as files
      b. rewrite the <schema-file> subtree and the root tree
      c. create a new commit (parent = current tip) — carries a ChangeId
      d. move the bookmark to the new commit
10. tx.commit("<op description>")  → writes a new Operation (the audit record).
11. Store idempotency result keyed by idempotency_key.
12. Return { commit_id, change_id }.
```

**Concurrency — no CAS rejection.** Two transactions starting from the same operation both commit; jj records concurrent operations and merges their views on next load. If they touched different declarations, the merge is clean. If they touched the same declaration, the bookmark/tree becomes **conflicted** (§6) — the second writer is not rejected; the conflict is recorded for resolution. The `idempotency_key` only dedupes literal network retries; durable identity is the `ChangeId`.

### 5.2 Transaction flow

Identical, except step 7 calls `compiler.apply_mutations(&schema, &ops)` (final-state validation only), step 8 checks every changed declaration, and step 9 writes all changes under **one** commit / one operation. Limits (≤ ops, ≤ schemas, timeout) are validated before step 7. Atomicity is inherent: a jj transaction either commits one operation or none.

---

## 6. First-Class Conflicts

When a merge/rebase/concurrent-edit cannot cleanly combine a declaration, jj stores the entry as a **conflict** — a merge of N sides (e.g. `base`, `ours`, `theirs`). schemahub surfaces this rather than failing:

- **Storage:** the conflicted declaration file becomes a jj `Conflict` (or merged-tree conflict) over the sides' `DeclBlob`s. The commit is valid and reachable; the bookmark may be marked conflicted.
- **Inspection:** `compiler.render_conflict(sides)` renders a human/agent-readable view of the competing declarations (e.g. both versions of `message UserRequest`). Exposed via a read RPC.
- **Resolution:** the client submits a resolved `DeclBlob` (a single valid declaration). `compiler.validate_resolution` checks it; a transaction replaces the conflict with the resolved file and records the resolution as an operation.
- **Policy gate:** publishing a conflicted state to a **protected** bookmark is refused; feature/working bookmarks may carry conflicts freely. This preserves "main is always clean" without ever forcing a lossy auto-merge.

This directly serves the agents-and-humans-editing-concurrently goal: an agent's racing edit produces a resolvable conflict, not a hard error to retry against a moving target.

---

## 7. Compatibility

Unchanged in spirit from v1 §5, re-anchored on bookmarks:

```rust
struct CompatibilityRules { direction: CompatibilityDirection, disabled: bool }
```

- **Per-repo direction**, default **FULL**; teams opt down consciously.
- **Protected bookmarks** (exact names + globs, e.g. `["main", "release/*"]`) are the only places compatibility is enforced. Mutations on unprotected bookmarks skip the check (step 8). This mirrors GitHub/GitLab branch protection.
- `--force` (requires `Maintainer`+) skips the check and records `force: true` in the commit.

---

## 8. Reference Integrity & Rename Propagation

- **Dependency index** is a derived index over the import statements in `__meta__` blobs and the `type_refs` of declarations; rebuildable by scanning objects reachable from bookmarks (an admin operation).
- **In-repo rename:** `compiler.apply_mutation` for a rename returns a `MutationEffect` that includes edits to all referencing declarations, applied in the same commit (atomic).
- **Across descendant commits:** rewriting a base declaration auto-rebases descendant commits (jj). Where a descendant can't absorb the rename cleanly, the result is a **conflict** on the affected declaration — surfaced, not silently broken.
- **Cross-repo:** v1 limitation stands — the server reports downstream repos that import the affected declaration (via the dependency index); the caller issues `UpdateImport` in those repos. Automated cross-repo propagation is v2.

---

## 9. Schema Exploration API (Read)

Per-declaration storage makes each read a direct object lookup (no whole-file parse):

```proto
rpc ListSchemas(...)        // root-tree subtree names
rpc ListDeclarations(...)   // names in a <schema-file> subtree + summaries
rpc GetDeclaration(...)     // one DeclBlob → DeclDetail
rpc FollowType(...)         // resolve a field's type via type_refs + dependency index
rpc ListDependencies(...)   // imports from __meta__, at pinned resolved_commit
rpc Search(...)             // by declaration name across schemas/repos
rpc GetSchemaSource(...)    // compiler.print(SchemaObjects) — reconstructed, never stored
```

`FollowType` and `Search` use the dependency / name index; both can cross repo boundaries via the full `project/repo/schema` path in each `Import`.

---

## 10. Codegen API

Reuses the sibling compilers' codegen; the VCS layer pre-computes the transitive import closure (BFS over `imports`, resolving each import's pinned commit, with cycle detection) and hands a `SchemaClosure` to the compiler:

```proto
rpc GetDescriptors(...)   // protobuf → FileDescriptorSet (via protoc-rs-codegen path);
                          // flatbuffers → reconstructed .fbs bundle; openapi → resolved YAML
rpc PreviewCodegen(...)   // compiler.generate_code(closure, lang) → rendered text, no files
```

The Protobuf compiler builds a `FileDescriptorSet` from the AST and calls `protoc-rs-codegen::generate_rust` — no `protoc` binary on the server. FlatBuffers resolves the `Schema` and calls `flatc-rs-codegen`. Imports are pinned by `resolved_commit`, so codegen is reproducible.

---

## 11. Auth Model

Unchanged from v1 §6:

```rust
trait AuthnProvider { fn identify(&self, md: &RequestMetadata) -> Result<Identity, AuthnError>; }
trait AuthzPolicy   { fn check(&self, who: &Identity, a: Action, r: &ResourcePath) -> Result<(), AuthzError>; }
enum Action { Read, Write, Force, ManageProject, ManageRepo }
```

- No-op implementations (`Identity::Anonymous`, `Ok(())`) ship in-tree as the getting-started default.
- Four project-scoped roles: `Owner` / `Maintainer` / `Writer` / `Reader`. `--force` requires `Maintainer`+.
- Auth runs after the idempotency check, before the transaction (steps 2–3). Public projects: reads open, writes always authenticated.
- Default role store: a `roles/<project>/<identity>` table, bootstrapped from `schemahub.toml`.

---

## 12. CLI Design

Resource-first, with jj-flavored history/recovery commands:

```bash
# Schema lifecycle
schemahub schema create user.proto
schemahub schema update user.proto
schemahub schema pull   payments/core-api/user.proto      # print reconstructed source
schemahub schema delete payments/core-api/user.proto

# Granular mutations
schemahub field add    payments/core-api/user.proto UserRequest email:string:3
schemahub field rename payments/core-api/user.proto UserRequest email email_address
schemahub message rename payments/core-api/user.proto UserRequest CreateUserRequest

# History & recovery (jj-style)
schemahub log     payments/core-api                       # commit/change graph
schemahub op log  payments/core-api                       # operation log (audit)
schemahub undo    payments/core-api                       # undo the last operation
schemahub diff    payments/core-api main..feature/xyz
schemahub resolve payments/core-api UserRequest           # resolve a conflicted declaration

# Bookmarks (branches)
schemahub bookmark create feature/xyz --from main
schemahub bookmark move   main --to <commit-or-change>
schemahub merge    feature/xyz --into main                # conflicts become objects, not errors
schemahub tag      create v1.0.0 --commit <id>

# Imports / codegen
schemahub import  update order.proto user.proto --to-tag v1.0.0
schemahub codegen get     payments/core-api/user.proto --lang rust --out ./gen/
schemahub codegen preview payments/core-api/user.proto --lang rust
```

Config: `~/.schemahub/config` (TOML) with `SCHEMAHUB_SERVER` / `SCHEMAHUB_TOKEN` overrides and `--profile`. Format inferred from extension and set in the RPC.

---

## 13. Migration From the v1 Implementation

The current code compiles and its mechanics (idempotency, auth traits, compatibility, transactions) are reusable, but three things change structurally:

1. **Replace hand-rolled parsers/ASTs** (`schemahub-plugin-*/src/parser.rs`, `ast/mod.rs`) with compiler wrappers over `protobuf-rs` / `flatbuffers-rs`. Keep the OpenAPI AST. **Write the printers** (the sibling compilers have none).
2. **Replace the `__schema__` single-blob storage** with the per-declaration split (§4.2). The `Compiler::parse` now returns `ParsedSchema { meta, decls }`; the VCS layer stores each decl as its own file.
3. **Replace the bespoke git-style object store + refs + GC** (`schemahub-storage`, `schemahub-core/version_control`, mutation CAS) with the jj-lib model: `DbBackend` + `DbOpStore` over `ObjectDb`, transactions, bookmarks, op-log/undo, first-class conflicts. The mutation/transaction *flows* (§5) keep their shape; the primitives underneath change.

Net effect: `schemahub-types`, `schemahub-api`, `schemahub-server`, `schemahub-cli` largely survive (with the `FormatPlugin`→`Compiler` rename and conflict/op-log RPCs added); `schemahub-storage` and `schemahub-core/version_control` are reworked onto jj-lib; the plugin crates become compiler crates. See `crate-structure.md`.
