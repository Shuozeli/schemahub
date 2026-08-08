<!-- agent-updated: 2026-07-30T04:16:42Z -->
# schemahub — Design (v2: Compilers + Jujutsu-style JJ)

> This document specifies *how* schemahub is built. It supersedes the v1 git-style design (preserved in git history). Two project-level decisions drive this revision:
>
> 1. **Compilers, not bespoke parsers.** Protobuf and FlatBuffers are fronted by the sibling compiler projects `protobuf-rs` and `flatbuffers-rs`; OpenAPI is in-tree. schemahub does not hand-roll format parsers or ASTs.
> 2. **JJ over a database.** The storage layer uses the `jj-lib` model — content-addressed commits, stable change IDs, first-class conflicts, and an operation log with undo — with **custom backends that persist to a database** (not jj's on-disk git/file layout).
>
> Where this document names a `jj-lib` type or method, it follows jj-lib's model; exact signatures are pinned to the vendored jj-lib version during implementation (see §4.6 and Open Question 11 in `requirements.md`).

> **Product extension (2026-07-21):** The compiler/JJ architecture in this
> document remains the versioned storage foundation. SchemaHub will add a
> durable change control plane and immutable schema serving plane over it. See
> `product.md` and `ADR/0001-change-records-and-serving-plane.md`. Their storage
> and API design must preserve the `Compiler` boundary and must make applying a
> change record atomic and recoverable.

> **As built (2026-07-21):** `schemahub-core::change_record` now owns the
> format-agnostic resource/lifecycle and a compare-and-set store. Mutable
> control-plane records use a separate stable-key namespace in `ObjectDb`, with
> memory, redb, and PostgreSQL implementations; JJ's content-addressed object
> namespace remains unchanged. The server injects this durable ledger and
> exposes the draft-note workflow through `ChangeService`.
>
> **Indexed ledger increment (2026-07-30):** `ChangeRecord` creates and
> lifecycle transitions now atomically maintain repository-scoped
> creation-order and status indexes. `ListChanges` reads only a bounded ordered
> range and validates every returned target; a one-time durable-marker
> migration backfills records written before the indexes existed.

> **Durable resource increment (2026-07-21):** Project, membership, and
> repository control-plane state now uses those ObjectDb resource records too.
> Project creation and its initial Owner are one transaction; mutable resources
> use ETags; archive retains JJ history. See `resources-and-policy.md`.
>
> **Indexed resource-catalog increment (2026-07-30):** Project and
> per-project repository creates/archive transitions atomically maintain
> active/all name indexes. `ListProjects` and `ListRepos` use bounded prefix
> ranges with existing v1 tokens and fail-closed target validation; durable
> one-time markers backfill resources created before the indexes.
>
> **Bounded membership increment (2026-07-30):** `ListMembers` pages the
> existing project-prefixed, hex-identity role primary keys instead of loading
> the global role collection. Project-bound cursors advance across inactive
> tombstones; the CLI follows every page and GUI summaries fetch only the
> caller's role.
>
> **Bounded repository-ref increment (2026-07-30):** `ListBranches` and
> `ListTags` expose stable lexicographical pages with opaque cursors bound to
> ref kind, project, repository, and prefix. JJ's operation view remains one
> repository-scoped immutable object containing ordered ref maps; a page walks
> that view lazily and materializes at most the requested limit plus one
> lookahead entry. `GetBranch` uses the map's direct named lookup, and both CLI
> list commands consume continuations to completion.

> **Bounded GUI-catalog increment (2026-07-30):** The unversioned BFF now
> adapts Core's indexed project/repository pages into `ProjectPageDto` and
> `RepoPageDto` rather than returning complete arrays. Its opaque cursor binds
> catalog kind, project scope, and name prefix. The React client keeps pages in
> an infinite query and requests continuation explicitly; repository deep links
> use a size-one exact-prefix read. Project summaries no longer enumerate every
> repository to calculate a count.
>
> **Bounded GUI-aggregate increment (2026-07-30):** Repository dashboards now
> return bounded schema, branch, and tag pages. One opaque continuation binds
> project, repository, and ref expression, advances all three component
> cursors, and carries the immutable commit resolved by the first page so a
> mutable bookmark cannot produce a mixed schema inventory. Schema conflict
> counts scan that immutable tree without collecting every conflict path.
> The selected schema page and the repository-local schema-name inventory load
> together in one additional JJ tree traversal; Core validates each selected
> declaration through its format compiler and counts unique compiler-reported
> direct imports without an N-per-schema tree scan or dependency traversal.
> Browser ChangeRecord pages adapt the repository/status index already used by
> `schemahub.v1`, with parent/status-bound tokens and source-redacted list
> records. TanStack infinite queries retain only explicitly requested pages.

> **Control-plane audit increment (2026-07-29):** Runtime project, membership,
> and repository mutations append typed immutable audit events in the same
> ObjectDb transaction as the resource create/CAS. Events are partitioned by
> project and exposed newest-first to Owners through an immutable order index
> and bounded backend range reads. Every index target and typed transition is
> validated fail-closed. JJ operations remain the separate undoable
> schema/repository history. A project-keyed distributed publication guard
> spans authorization, last-Owner validation, and state/event/index commit so
> concurrent administrators cannot violate membership invariants. See
> `resources-and-policy.md`.

> **Immutable artifact increment (2026-07-21):** `schemahub-core::serving`
> stores the first successful artifact bytes in a versioned ObjectDb record
> before response. Atomic create gives redb/PostgreSQL and mixed-renderer
> instances one winner; later reads validate and return that winner before
> compiler lookup. Request identity includes every renderer input, stored
> dependencies are reauthorized, corrupt records fail closed, and JJ GC does
> not sweep the artifact collection. See `serving.md`.

> **API boundary increment (2026-07-21):** `schemahub.v1` gRPC/protobuf is the
> designated public 1.0 integration contract. The co-located unversioned
> `/api/*` HTTP surface is a GUI-only BFF, labeled in responses and generated
> OpenAPI and excluded from the public API compatibility promise. Operational
> routes retain their separate support contract. See ADR 0002.

---

## 1. Architecture Overview

schemahub is two layers with a single trait boundary between them.

```
┌──────────────────────────── JJ layer (format-agnostic) ────────────────────────────┐
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
   │  mutate · conflict │          │  mutate · conflict   │          │  mutate · conflict  │
   │  codegen (reuse)   │          │  codegen (reuse)     │          │  descriptors        │
   └────────────────────┘          └──────────────────────┘          └─────────────────────┘
```

**The JJ layer knows nothing about Protobuf, FlatBuffers, or OpenAPI.** It stores opaque per-declaration blobs and delegates all format-specific work to a `Compiler` via the trait below. Adding a format (SQL DDL, Thrift) is a new compiler crate; the JJ layer is untouched.

**Key difference from v1:** v1 reduced this to a single `__schema__` blob per file fronted by hand-rolled mini-parsers. v2 fixes both: real compiler ASTs, and genuine per-declaration objects (§4.2) so jj's content-addressing, dedup, and first-class conflicts operate at declaration granularity.

---

## 2. The `Compiler` Trait

This is the boundary between the two layers (v1 called it `FormatPlugin`; renamed to reflect that each implementation *is* a compiler front-end). A `DeclBlob` is the serialized AST of **one top-level declaration**; a `MetaBlob` is the file-level metadata (package, imports, syntax/edition). Both are opaque `Vec<u8>` to the JJ layer.

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
    /// Whole-schema input supports both metadata imports and declaration-level
    /// references such as OpenAPI external `$ref` values.
    fn imports(&self, schema: &SchemaObjects) -> Result<Vec<Import>, ReadError>;
    /// Every type reference in a declaration (dependency/ref-integrity helpers).
    fn type_refs(&self, blob: &DeclBlob) -> Result<Vec<TypeRef>, ReadError>;
    /// The exact requested field/property's one named type; scalar is None.
    fn field_type_ref(&self, blob: &DeclBlob, field_name: &str)
        -> Result<Option<TypeRef>, ReadError>;

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

Each compiler serializes its AST nodes with a stable, versioned encoding, wrapped with a `blob_version: u32` for migration. The JJ layer never inspects these bytes — it only content-addresses and stores them. The encoding used per compiler is determined by what the underlying AST types support:

| Compiler | Encoding | Why |
|----------|----------|-----|
| Protobuf | `prost` | `protoc-rs-schema::FileDescriptorProto` and its sub-descriptors implement `prost::Message`. |
| FlatBuffers | `serde_json` | `flatc-rs-schema` types implement `serde::Serialize`/`Deserialize` but NOT `prost::Message`. |
| OpenAPI | `serde_json` | In-tree AST; small, human-debuggable; serde is already a dependency. |

Determinism: each encoder writes fields in declaration order with no key reordering, so identical ASTs round-trip to identical bytes.

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

No sibling compiler exists, so this stays in-tree. The existing in-tree AST (`docs/openapi-ast.md`) is kept with a per-declaration split: one `DeclBlob` per path-group and per component (`schemas`, `parameters`, `responses`, `requestBodies`), with stable addressable paths. Parsing is recursively fallible: malformed object/array shapes, declaration keys, references, parameter locations, and JSON Schema types are rejected before an empty/default declaration can enter immutable history. The selected 1.0 mutation surface includes whole-document push plus path, operation, and component-schema add/remove operations; all are direct- and transaction-reachable.

---

## 4. JJ Layer (jj-lib over a database)

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

`Jj::undo` is a **linear monotonic walk-back stack**, not jj's bare op-toggle:
consecutive `undo` calls step further back through the content-op chain
(skipping leading `undo` ops at the head), rather than re-doing the previous
undo. Core uses the validated undo path to inspect every bookmark target before
the view changes; protected bookmarks cannot be restored to a conflicted or
reference-invalid exact tree. The newly recorded `undo` operation's view equals
the validated target state's view; the response carries the id of the operation
whose effect was rolled past.

Persisted operations, views, commits, and trees validate every required field
and exact object-ID length before constructing jj-lib values. Unsupported git
submodules and database faults are surfaced explicitly rather than converted to
absence. For audit reads with a limit, `Jj::list_operations_tail` walks only the
requested suffix of the normal linear operation history; a branch inside that
suffix deliberately falls back to the complete graph traversal so ordering and
deduplication stay compatible.

### 4.5 Database choice

The backend/op-store are written against a small internal `ObjectDb` trait so the concrete database is swappable:

- **`redb` (default, embedded):** single-file, MVCC, zero-ops — ideal for self-hosted/dev and the open-source default.
- **`postgres` (server):** matches the team's standard infra for multi-instance deployments; objects and op-log in tables, content-addressed ids as primary keys.

Project/repo namespacing: each `project/repo` is an independent jj repo (its own op-log and bookmark set), keyed by a `(project, repo)` prefix in the `ObjectDb`. Content objects dedup globally; op-logs are per-repo. (Alternative — one global repo with a project/repo tree prefix — is rejected: per-repo op-logs give per-repo undo and isolation.)

The PostgreSQL adapter bridges JJ's synchronous backend trait to SQLx with one
long-lived bounded executor; it never spawns a thread per query. Schema changes
use embedded, checksum-verified SQLx migrations. The adoption-safe baseline
registers databases created before migration tracking without rewriting their
tables or records.

Mutable ChangeRecords remain stable-key records outside JJ. Their public
creation-time/name and optional status pagination is backed by per-repository
ordered collections. Create atomically inserts the resource plus its all/status
entries; a lifecycle transition atomically compare-swaps the resource, removes
the old status entry, and inserts the new one. The first post-upgrade ledger
operation scans legacy records once, creates missing entries plus a completion
marker in one transaction, and fails closed on malformed or conflicting state.
Subsequent pages are bounded `ObjectDb::list_records_page` ranges. The marker
means old and new binaries cannot safely share one database during this
migration.

Because content objects deduplicate globally, GC is also global at the object
layer even when the authorized request names one repository. The mark phase
discovers repository keys directly from op/ref storage, retains every operation
view and commit ancestry across all repositories, then sweeps. All JJ mutations
hold a shared maintenance guard for their complete object-plus-operation
publication; GC holds the exclusive side across mark and sweep. PostgreSQL uses
a database advisory read/write lock so independent server instances share the
same fence; redb and memory use an RW lock. Operational migration,
backup/restore, and recovery procedures are in `codelab-operations.md`.

Every operation-head publisher additionally holds an exclusive repository
publication guard from loading the current head through merge, exact-final-tree
policy validation, and `Transaction::commit`. Memory/redb use a process-local
mutex (redb is the single-process embedded backend); PostgreSQL uses a stable
repository-keyed advisory lock shared by all server instances. Different
PostgreSQL repositories can still publish concurrently. This guard prevents
lost op-head updates and makes protected-conflict/reference decisions atomic
with the JJ operation they authorize.

### 4.6 What we use from jj-lib, and what we bypass

**Use:** `Backend`, `OpStore`, `RepoLoader`/`ReadonlyRepo`/`MutableRepo`, `Transaction`, commits/trees/conflicts, change IDs, bookmarks, merge/rebase + auto-rebase, revsets.

**Bypass:** jj's **filesystem working copy** (`LocalWorkingCopy`) and **git interop** (`GitBackend`). There is no checked-out tree on disk; the "working-copy commit" is a *logical* editable tip created directly in a transaction. The CLI/agent edits via RPC, not by writing files.

> Implementation note: jj-lib's API evolves and some types (notably the exact conflict representation — older `Conflict` objects vs. inline merged-tree ids) differ by version. We pin one jj-lib version, vendor it, and adapt these sketches to its concrete signatures. This is Open Question 11.

---

## 5. Mutation API

### 5.1 Single mutation flow

```
1. AuthN: identify caller from request metadata.
2. AuthZ: caller has Write (or Force, if --force) on <project>/<repo>.
3. Fingerprint the semantic request and observe its scoped receipt. If a
   completed response or correlated JJ operation exists, replay it. STOP.
4. Validate an optional base_revision belongs to retained target-repository
   history. A stale base is valid causal provenance, not a HEAD CAS gate.
5. Load repo at current op:  let repo = repo_loader.load_at_head();
6. Resolve the target bookmark exactly once to an immutable planning commit
   (or the repository root for a first write).
7. Load SchemaObjects (meta + the touched DeclBlobs) from that immutable tree.
8. effect = compiler.apply_mutation(&schema, &op);
9. If target bookmark is protected and !force: for each changed decl,
      compiler.check_compatibility(old, new, rules)  → CompatibilityError on violation.
10. Atomically claim a persistent receipt lease immediately before publication.
11. Start a jj Transaction and stamp receipt/attempt correlation attributes
    plus `schemahub.force=true` when an elevated override was used:
      a. write changed/added DeclBlobs and (if any) __meta__ as files
      b. rewrite the <schema-file> subtree and the root tree
      c. merge the writer tree from the immutable planning commit with a newer
         current tip when necessary
      d. while the backend repository publication guard is still held, validate
         that exact final tree: protected targets contain no unresolved
         conflicts, and no schema that disappeared leaves a live unpinned
         same-repository importer; force cannot bypass either invariant
      e. create a commit carrying a ChangeId and move the bookmark to it
12. tx.commit("<op description>")  → writes a new Operation (the audit record).
13. CAS the pending receipt to { commit_id, change_id, conflicted_decls }.
14. Return the receipt.
```

The observation in step 3 is read-only. Missing keys are claimed only after
validation, so invalid input does not poison the receipt namespace. If the
process stops between steps 12 and 13, a retry reconstructs the response from
the exact historical JJ operation and repairs the receipt. See
`idempotency.md`.

A known policy rejection in step 11d occurs before any JJ operation exists, so
SchemaHub deletes the pending direct-write receipt (or releases a ChangeRecord
Apply lease back to Ready) for immediate retry. Operational JJ errors remain
pending because their publication outcome may be ambiguous and correlation
recovery must remain possible.

**Concurrency — no base-revision CAS rejection.** Writers may plan from the
same immutable commit; repository publication is serialized only for the final
load/merge/validate/commit boundary. If they touched different declarations,
the merge is clean. If they touched the same declaration, an unprotected
bookmark stores a first-class conflict (§6). A protected bookmark rejects that
exact conflicted tree before commit; force does not bypass the protection. The
`idempotency_key` only dedupes literal network retries; durable identity is the
`ChangeId`.

### 5.2 Transaction flow

Identical to §5.1, except step 8 calls
`compiler.apply_mutations(&schema, &ops)` (final-state validation only), step 9
checks every changed declaration, and publication writes all changes under
**one** commit / one operation. This permits ordered migrations such as
deleting a referenced declaration before deleting or rewriting its consumer
later in the same batch, while rejecting any dangling reference in the final
state. The implemented bounds (≤100 ops, ≤20 schemas) are validated before
compiler work. `ApplyTransaction` starts a 30-second monotonic server deadline
before decoding/normalization, moves synchronous compiler and ObjectDb work to
the blocking executor, and shares cancellation with Core. Core checks the
deadline throughout planning and again inside the guarded final-tree callback;
expiry before publication aborts any pending idempotency receipt. A write that
already crossed the final atomic publication boundary remains covered by normal
idempotency reconciliation. Atomicity is inherent: a jj transaction either
commits one operation or none.

**Multi-file transactions.** A transaction may touch several schema files within one `(project, repo)`. The implementation (`schemahub-core/src/mutation/transaction.rs`) groups the ordered ops by `Mutation::schema_path` (preserving op order within each file and first-appearance file order), loads each touched file's base, applies that file's ops through the compiler to produce one `MutationEffect` per file, runs the compat gate per file when the bookmark is protected, and commits every effect atomically through `Jj::commit_schema_changes_validated` — one exact-final-tree policy decision, one commit, and one operation across all touched files. Every op in a transaction must share one `format_id` (transactions never mix formats) and one `(project, repo)`.

Default limits served by the core (`TransactionLimits::default`): `max_ops = 100`, `max_schemas = 20`. The protobuf comment and `AdminService.GetServerConfig` now report the same values.

---

## 6. First-Class Conflicts

When a merge/rebase/concurrent-edit cannot cleanly combine a declaration, jj stores the entry as a **conflict** — a merge of N sides (e.g. `base`, `ours`, `theirs`). schemahub surfaces this rather than failing:

- **Storage:** the conflicted declaration file becomes a jj `Conflict` (or merged-tree conflict) over the sides' `DeclBlob`s. The commit is valid and reachable; the bookmark may be marked conflicted.
- **Inspection:** `compiler.render_conflict(sides)` renders a human/agent-readable view of the competing declarations (e.g. both versions of `message UserRequest`). Exposed via a read RPC.
- **Resolution:** the client submits a resolved `DeclBlob` (a single valid declaration). `compiler.validate_resolution` checks it; a transaction replaces the conflict with the resolved file and records the resolution as an operation.
- **Policy gate (required before 1.0):** publishing a conflicted final tree to a
  **protected** bookmark must be refused atomically; feature/working bookmarks
  may carry conflicts freely. Immutable planning-base tests now prove conflicts
  are surfaced instead of overwritten, but the exact final-tree rejection is
  still an explicit release gate in `tasks.md`.

This directly serves the agents-and-humans-editing-concurrently goal: an agent's racing edit produces a resolvable conflict, not a hard error to retry against a moving target.

---

## 7. Compatibility

Unchanged in spirit from v1 §5, re-anchored on bookmarks:

```rust
struct CompatibilityRules { direction: CompatibilityDirection, disabled: bool }
```

- **Per-repo direction**, default **FULL**; teams opt down consciously.
- **Protected bookmarks** (exact names + globs, e.g. `["main", "release/*"]`) are the only places compatibility is enforced. Mutations on unprotected bookmarks skip the check (step 8). This mirrors GitHub/GitLab branch protection.
- `--force` (requires `Maintainer`+) skips compatibility only and records
  `schemahub.force=true` in the durable JJ operation. It does not bypass
  reference integrity or the protected-conflict invariant.

### 7.1 Executable capability contract

`AdminService.GetFormatCapabilities` is the runtime source of truth for format
features, operation status, and direct/transaction reachability. The CLI exposes
it as `schemahub capabilities [--json]`. Matrix version `1.0` describes the
current interpretation; operation additions do not by themselves change the
matrix version. See `format-capabilities.md`.

---

## 8. Reference Integrity & Rename Propagation

- **Dependency view:** Protobuf and FlatBuffers imports are read from `__meta__`
  blobs at immutable repository snapshots. Same-repository final-tree scans
  enforce whole-schema deletion safety. `ListDependents` performs a bounded
  on-demand reverse scan across repositories visible to the caller and returns
  the exact per-repository snapshot manifest; 1.0 deliberately does not persist
  a reverse index.
- **Immutable import pins:** `to_tag` is resolved at the API boundary and only its commit ID is stored. Explicit commits are checked against the named target repository and schema. Pinned imports therefore cannot drift after publication.
- **In-repo rename:** `compiler.apply_mutation` for a rename returns a `MutationEffect` that includes edits to same-file referencing declarations and relevant metadata, applied in the same commit (atomic). Protobuf covers messages/services/enum values, including extension extendees and proto2 defaults; FlatBuffers covers tables/enums/unions/enum values.
- **Deletion:** a declaration or OpenAPI component with a remaining same-file
  reference is rejected. Whole-schema deletion also rejects remaining
  same-repository live unpinned imports, even with force. ChangeRecord batches
  validate touched consumer/provider final state, enabling an atomic migration;
  the final merged tree is revalidated while the backend publication guard is
  held through JJ commit, closing both consumer-first and delete-first races.
- **Across descendant commits:** rewriting a base declaration auto-rebases descendant commits (jj). Where a descendant can't absorb the rename cleanly, the result is a **conflict** on the affected declaration — surfaced, not silently broken.
- **Cross-repo:** automated propagation is out of scope. Callers use
  `ListDependents`, retain its per-repository bookmark/commit manifest, and
  coordinate explicit `UpdateImport` ChangeRecords. This is a direct-edge,
  authorization-filtered advisory read: there is no global snapshot, transitive
  reverse traversal, or cross-repository transaction. See
  `dependency-discovery.md`.

---

## 9. Schema Exploration API (Read)

Per-declaration storage makes each read a direct object lookup (no whole-file parse):

```proto
rpc ListSchemas(...)        // root-tree subtree names
rpc ListDeclarations(...)   // names in a <schema-file> subtree + summaries
rpc GetDeclaration(...)     // one DeclBlob → DeclDetail
rpc FollowType(...)         // exact field/property type + local/imported declaration snapshots
rpc ListDependencies(...)   // normalized live/pinned imports with effective target commits
rpc ListDependents(...)     // direct visible-repository reverse scan + snapshot manifest
rpc Search(...)             // by declaration name across schemas in one repository
rpc GetSchemaSource(...)    // compiler.print(SchemaObjects) — reconstructed, never stored
```

Every repository-local read resolves its branch/tag/commit once and performs
the full operation at that repository-owned immutable commit. Responses expose
the resolved commit (or, for commit streams, initial metadata); omitted refs use
the repository's configured default bookmark. Raw commits are ownership-checked
at the JJ boundary for both reads and ref publication because content objects
deduplicate globally.

`Search` is repository-scoped and fails on unknown schema formats rather than
silently omitting files. `FollowType` asks the compiler for the exact named
field/property's type, resolves its matching local or imported declaration, and
returns populated summary/detail plus source/target commits, pin state, and the
stored import path. It never substitutes the first unrelated type reference;
scalar, missing, and ambiguous cases fail explicitly.

Forward `ListDependencies` traverses `(schema, immutable commit)` nodes so two
historical revisions are not collapsed. Same-repository live edges remain on
the importing snapshot, cross-repository live edges share one configured-
default snapshot per target repository, and pins retain their stored commit.
Unreadable/archived/absent external or builtin targets remain explicit
`resolved=false` edges and are not traversed; invalid pins, corruption, unknown
formats, and bounds fail the call.

`ListDependents` is the explicit cross-repository reverse-discovery method. It
resolves each readable repository's configured default bookmark once, performs
all reads at that immutable commit, and returns those commits with its results.
Scans are bounded to 1,000 visible repositories and 10,000 schemas and fail
closed rather than returning a silent partial result.

`Log` (the commit/change history) walks the **real** commit graph from a ref via `Jj::commit_log`, newest→oldest, surfacing each commit's content-addressed `commit_id`, its stable jj `change_id` (reverse hex), parents, author, message, and timestamp. This is distinct from `OpLog` (the operation log audit record); see §6 / §4.4.

---

## 10. Codegen API

Reuses the sibling compilers' codegen; the JJ layer pre-computes the transitive import closure (BFS over `imports`, resolving each import's pinned commit, with cycle detection) and hands a `SchemaClosure` to the compiler:

```proto
rpc GetDescriptors(...)   // protobuf → FileDescriptorSet (via protoc-rs-codegen path);
                          // flatbuffers → reconstructed .fbs bundle; openapi → resolved YAML
rpc PreviewCodegen(...)   // compiler.generate_code(closure, lang) → rendered text, no files
```

`SchemaClosure` carries the explicitly requested root path as well as its entry
map; compilers must not infer the root from hash-map or lexical iteration. The
Protobuf compiler builds a `FileDescriptorSet`, resolves parser-level named
message and enum references across the complete closure, and calls
`protoc-rs-codegen::generate_rust` — no `protoc` binary on the server.
FlatBuffers combines the closure but takes `root_type`, file identifier, and
file extension only from the requested root before calling
`flatc-rs-codegen`. Imports are pinned by `resolved_commit`, so codegen is
reproducible.

---

## 11. Auth Model

Two trait interfaces in `schemahub-types`:

```rust
trait AuthnProvider { fn identify(&self, token: Option<&str>) -> Result<Identity, AuthnError>; }
trait AuthzPolicy   { fn check(&self, who: &Identity, a: Action, r: &ResourcePath) -> Result<(), AuthzError>; }
enum Action { Read, Write, Force, ManageProject, ManageRepo }
```

### Default ("Noop") mode

When `schemahub.toml` has no static tokens, JWT block, or `[projects.*]`
bootstrap, the server installs `NoopAuthn` (`Identity::Anonymous`) +
`NoopAuthz` (every action allowed). This is the getting-started default —
anonymous reads and writes, no enforcement.

### Durable RBAC with configured credentials

When either static development tokens or production JWT verification is
configured, the server installs the same durable RBAC layer:

- **`BearerTokenAuthn`** (`schemahub-core/src/auth_impls.rs`) — a static
  `token → Identity` table populated from `[auth].tokens` for development.
- **`JwtAuthn` + `JwtAuthRuntime`** (`schemahub-server/src/jwt_auth.rs`) — a
  synchronous verifier over a prevalidated rotating JWKS plus an asynchronous,
  supervised HTTPS/file loader. JWT time uses an injected `JwtClock`. Static
  tokens and `[auth.jwt]` are mutually exclusive.
- **`RoleBasedAuthz`** (`schemahub-core/src/auth_impls.rs`) — project-scoped role checks over a `RoleStore` and `ProjectStore`.
- **`ObjectDbRoleStore` + `ObjectDbProjectStore`** (`schemahub-core/src/auth_object_db.rs`) — transactional resource records, bounded project membership ranges, and active/all project catalogs in the selected redb/PostgreSQL database. The JSON stores remain one-time migration readers.

The production provider validates an explicit token type, asymmetric algorithm
allowlist, configured issuer/audience, signature, `kid`, expiration, optional
`nbf`/`iat`, bounded inputs, and key-cache freshness. It never follows a
token-provided key URL. A complete refresh is validated before atomic swap; the
last known-good keys remain available only through the configured staleness
window. Once stale, requests fail authentication and HTTP readiness returns
`503`. See `authentication.md` for the claims and rotation contract.

JWT subjects become durable role-store keys as
`identity_id_prefix + sub`. Trusted `schemahub_identity_kind` and
`schemahub_delegated_by` claims preserve human/agent/service audit attribution
without changing authorization privileges.

Four project-scoped roles, descending: `Owner` / `Maintainer` / `Writer` / `Reader`. `--force` requires `Maintainer`+. `ManageProject` (member CRUD) is `Owner`-only. Auth runs before receipt observation, so replay cannot disclose a prior write result to an unauthorized caller.

**Visibility:** projects carry `Visibility::Public | Private` in `ProjectMeta`. Public projects open reads to anonymous; private projects require a member identity.

**Invariants:**

- *Last Owner guard.* `remove_member` / `update_member_role` fail-fast if the change would leave a project with zero Owners.
- *Create-project authentication.* `CreateProject` requires a non-anonymous identity (the caller becomes the project's Owner).

### Bootstrap from `schemahub.toml`

`[projects.<name>]` blocks seed missing project records and reconcile configured
roles at startup. Existing project metadata is not overwritten:

```toml
[projects.acme]
visibility = "private"           # or "public"
owners     = ["alice"]
members    = { bob = "Writer", carol = "Reader" }
```

A `[projects.*].members` entry whose role string doesn't parse as one of `Reader`/`Writer`/`Maintainer`/`Owner` fails the server at startup (fail-closed). When the registries are not configured, `EmptyRoleStore` / `EmptyProjectStore` fallbacks are used: lookups are empty and project/member writes fail with `Unsupported` rather than claiming to persist data.

Project and repository resources use compare-and-swap ETags, ordered pagination,
and history-preserving archive. An archived project fails closed for all normal
descendant operations; only Owners may request its explicit audit view. Former
`[auth].data_dir` JSON records are imported project-plus-complete-ACL in one
transaction on first database-backed startup.

Active membership reads use a bounded exclusive range over
`projects/{project}/members/{hex(identity)}`. The key order is the public
identity-byte order, but the physical key and token encoding stay internal.
Inactive records remain as tombstones and may produce an empty continuable
page; scoped malformed records fail the request.

---

## 12. CLI Design

Resource-first, with jj-flavored history/recovery commands:

```bash
# Durable change intent (implemented; --json is agent/CI-safe)
schemahub change note payments/core-api --title "Add settlement currency"
schemahub change get projects/payments/repos/core-api/changes/<id> --json
schemahub change list payments/core-api --status draft --json
schemahub change update projects/payments/repos/core-api/changes/<id> --etag v1 --title "..."
schemahub change abandon projects/payments/repos/core-api/changes/<id> --etag v2

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

Config: `~/.schemahub/config` (TOML) with `SCHEMAHUB_SERVER` /
`SCHEMAHUB_TOKEN` overrides and `--profile`. A server coordinate is required;
there is no implicit loopback endpoint, and malformed/unreadable config fails
closed. Format is inferred from the extension and set in the RPC.

---

## 13. Migration From the v1 Implementation

The structural migration to v2 is **complete** (initial v2 commit landed on the `v2-rearchitecture` branch). Three things changed:

1. **Hand-rolled parsers/ASTs replaced** by compiler wrappers over `protobuf-rs` / `flatbuffers-rs` (`schemahub-compiler-protobuf` / `schemahub-compiler-flatbuffers`); the OpenAPI AST stayed in-tree (`schemahub-compiler-openapi`). All three compilers ship in-tree printers (the sibling crates have none).
2. **`__schema__` single-blob storage replaced** by the per-declaration split (§4.2). `Compiler::parse` returns `ParsedSchema { meta, decls }`; the JJ layer stores each decl at `<schema>/<decl>` with `__meta__` for the file metadata.
3. **Bespoke git-style object store + refs + GC replaced** by the jj-lib model: `DbBackend` + `DbOpStore` over an `ObjectDb`, transactions, bookmarks, op-log/undo, first-class conflicts. Mutation/transaction flows (§5) keep their shape; the primitives underneath are jj-native.

Net effect: `schemahub-types`, `schemahub-api`, `schemahub-server`, `schemahub-cli` largely survived (with the `FormatPlugin`→`Compiler` rename and conflict/op-log RPCs added); `schemahub-storage` + `schemahub-core/version_control` were reworked into `schemahub-jj`; the plugin crates became compiler crates. See `crate-structure.md`.

### Post-v2 increments

After the v2 cut, two extensions landed on the same branch (not architectural changes, just filling in what v2 left as Noop / stub):

- **Postgres `ObjectDb`** behind the `postgres` feature on `schemahub-jj` (and forwarded to `schemahub-server` via `--features postgres`). Redb remains the default.
- **Project-scoped RBAC and durable resources** — static development tokens or
  rotating external JWT/JWKS authentication plus `RoleBasedAuthz` over
  ObjectDb-backed project/role stores, persisted repository policy,
  `[projects.*]`/`[repos.*]` bootstrap, and one-time JSON access-store import.
  See §11, `authentication.md`, and `resources-and-policy.md`. Noop auth still
  ships for local evaluation when no credential or project configuration is
  present.

The project and repository lifecycle RPCs are persisted; update/archive use
ETags and retain schema history.
