# schemahub — Open Questions

> Each entry provides the context, the relevant requirement, the design goal, the specific unresolved question, and a suggested approach. Entries are ordered roughly by implementation dependency — questions that block other work appear first.

---

## OQ-01: Multi-Schema Tree Structure

### Context
The design uses a git-style object model where a `Commit` points to a `Tree`, and a `Tree` maps names to blob hashes. In git, a tree represents one directory — entries are either file blobs or subtree hashes. schemahub's design doc currently defines a Tree as mapping "declaration name → blob hash", implying one Tree per schema file. But a repo contains multiple schema files (`user.proto`, `order.proto`, `payment.proto`), and a branch must cover the state of the entire repo — not just one file.

### Requirement
> "Resources are namespaced as `project / repo / schema`."
> "Git-style semantics: commits, branches, tags, history, diff, merge."

A branch in git covers an entire repository. Consistency requires that a branch in schemahub covers an entire repo (all its schemas), so a single commit represents the full snapshot of the repo at a point in time.

### Goal
Define the exact Tree structure so that one commit captures the state of all schemas in a repo, while preserving the content-addressable deduplication property (unchanged schemas share blob hashes across commits).

### Open Question
Should the Tree object be a **single-level structure** (one Tree per schema file, with the repo level being an additional Tree-of-Trees), or a **flat structure** (all declarations from all schemas in one Tree with composite keys)?

### Suggested Approach
Adopt the **two-level tree structure**, mirroring git's directory model exactly.

```
Commit → root_tree_hash
root_tree  → { "user.proto":  schema_tree_hash_A,
               "order.proto": schema_tree_hash_B }
schema_tree_A → { "UserRequest":  blob_hash_1,
                  "UserResponse": blob_hash_2 }
schema_tree_B → { "Order": blob_hash_3 }
```

The `Tree` object gains an `entry_type` field distinguishing `SubTree` entries (pointing to another tree hash) from `Blob` entries (pointing to a declaration blob hash):

```rust
enum TreeEntry {
    SubTree { name: String, tree_hash: Hash },
    Blob    { name: String, blob_hash: Hash },
}
```

The root tree always contains only `SubTree` entries (one per schema file). Schema-level trees always contain only `Blob` entries (one per top-level declaration). This two-level invariant is enforced by the core — no deeper nesting is permitted in v1.

When a single field in `UserRequest` changes, only the `UserRequest` blob and the `user.proto` schema tree are rewritten. The `order.proto` schema tree and its blobs are reused unchanged across the commit. This preserves the deduplication property.

The `index/` and `deps/` KV namespaces use the schema file name (the root tree entry name) as the `<schema>` component — consistent with the two-level structure.

---

## OQ-02: KV Namespace Scoping by Project/Repo

### Context
The design doc defines KV namespaces as:
```
objects/<hash>
refs/heads/<branch>
deps/<schema>/<decl>@<commit>
index/<schema>/<decl>/<field>
```
None of these include a project or repo prefix. schemahub is a multi-tenant registry serving many projects and repos simultaneously. Two repos — `payments/core-api` and `inventory/catalog` — could both have a schema named `user.proto` with a message named `UserRequest`. In the current namespace layout, their blobs, refs, and index entries would be indistinguishable.

### Requirement
> "Resources are namespaced as `project / repo / schema`."
> "Per-project ACLs and visibility (public / private)."

### Goal
Ensure complete data isolation between repos while retaining the storage efficiency benefit of content-addressed deduplication for blobs (identical declarations across repos naturally share one blob object).

### Open Question
Two options:

**Option A — Key prefixing (shared KV store):**
All namespace-sensitive keys are prefixed with `<project>/<repo>/`. `objects/<hash>` stays unprefixed.

**Option B — Per-repo KV store instances:**
Each repo gets its own isolated KV store file. No cross-repo deduplication.

### Suggested Approach
Use **Option A — key prefixing with a shared KV store**.

```
objects/<hash>                                    → global, unprefixed (content-addressed)
refs/<project>/<repo>/heads/<branch>              → commit hash
refs/<project>/<repo>/tags/<tag>                  → tag hash
deps/<project>/<repo>/<schema>/<decl>@<commit>    → dependency entries
index/<project>/<repo>/<schema>/<decl>/<field>    → field index entries
idempotency/<project>/<repo>/<key>                → idempotency result
pending/<mutation_id>                             → blob hash (global, short-lived)
```

`objects/` is global and unprefixed. Two repos that happen to store the same message declaration (e.g. a common `Timestamp` message) share one blob object — no duplication. All other namespaces are fully scoped, providing complete isolation.

The trade-off: all repos share one KV store file. This simplifies operations (one backup, one GC run) but means a single corrupted store affects all repos. For v1, this is acceptable. If per-repo isolation becomes a requirement (e.g. for compliance), Option B can be implemented behind the storage trait without changing the namespace design.

---

## OQ-03: The `Mutation` Type Definition

### Context
The `FormatPlugin` trait exposes `apply_mutation(blob, mutation: &Mutation)` and `apply_mutations(blobs, mutations: &[Mutation])`. The `Mutation` type is a boundary type between the core layer and the format-specific layer. It also determines the shape of the mutation gRPC RPCs that clients call. Yet `Mutation` is never defined anywhere in the design doc.

The challenge: mutations are inherently format-specific. `DeprecateField` is a FlatBuffers concept that doesn't exist in Protobuf. `ReserveFieldNumber` is a Protobuf concept that doesn't exist in FlatBuffers. Yet the core must be able to route a mutation to the right plugin without understanding its content.

### Requirement
> "Granular RPCs that mutate individual schema elements — not text patches, not whole-file replacements."

### Goal
Define a `Mutation` type that allows the core to route to the correct plugin without understanding the operation content, while preserving strong typing for clients at the gRPC API level.

### Open Question
Three candidate designs: opaque bytes envelope, per-format RPCs, or a typed oneof in the proto.

### Suggested Approach
**Split the concern into two layers**: the gRPC API layer uses a typed `oneof` for client ergonomics; the internal Rust core uses an opaque envelope.

**gRPC API (client-facing):** The mutation RPC uses a `oneof` so clients get compile-time type safety:
```proto
message ApplyMutationRequest {
  string project      = 1;
  string repo         = 2;
  string branch       = 3;
  string base_revision = 4;
  string idempotency_key = 5;
  oneof operation {
    ProtobufMutation  protobuf_op  = 6;
    FlatBuffersMutation fbs_op     = 7;
    OpenApiMutation   openapi_op   = 8;
  }
}
```

**Internal Rust core:** The gRPC handler converts the matched `oneof` branch into an opaque envelope before passing to the core:
```rust
struct Mutation {
    schema_path: SchemaPath,  // core uses this to load the right blob
    format_id:   String,      // core uses this to select the plugin
    operation:   Bytes,       // format-specific bytes, opaque to core
}
```

The core never inspects `operation`. It reads `format_id`, selects the plugin, and passes `operation` to `apply_mutation`. The plugin deserializes it.

This gives clients strong typing without the core needing to enumerate format-specific variants. Adding a new format in v2 requires adding a new `oneof` branch to the proto and a new plugin — the core dispatch loop is unchanged.

---

## OQ-04: The `SchemaChange` Type Definition

### Context
`FormatPlugin::diff(old, new) -> Vec<SchemaChange>` returns a list of changes between two AST blobs. This type is consumed for the history/log API, merge conflict detection, and potentially the compatibility checker. The tension is between a format-specific type (rich detail, core can't use it for merge) and a generic core type (core can use it, but loses detail).

### Requirement
> "Git-style semantics: commits, branches, tags, history, diff, merge."

Diff output must be human/agent-readable and machine-usable for merge conflict detection.

### Goal
Define a `SchemaChange` type that gives the core enough information for merge while preserving format-specific detail for display.

### Open Question
Two candidates: two-tier type with opaque detail, or fully opaque type with core using blob-hash identity for merge.

### Suggested Approach
Use the **two-tier type** (Option A from the open question):

```rust
enum SchemaChange {
    DeclarationAdded   { name: String },
    DeclarationRemoved { name: String },
    DeclarationModified {
        name:   String,
        detail: Bytes,  // format-specific, opaque to core
    },
}
```

The core uses only `name` for merge conflict detection: if both branches produced a `DeclarationModified` for the same `name`, that is a conflict. The `detail` bytes are forwarded verbatim to the history/log RPC response, where the client-side plugin library or CLI deserializes them for display.

The compatibility checker does **not** consume `SchemaChange`. It receives the old and new blobs directly via `check_compatibility(old, new, rules)` and does its own analysis. This keeps the compatibility checker independent of the diff representation.

---

## OQ-05: Schema Exploration API (Read Path)

### Context
The requirements specify an entire read API in Section 4, described as the primary interface for AI agents. The design doc covers the write path in detail but contains no design for the read path. The `FormatPlugin` trait has no method for exposing individual AST nodes to the core for traversal.

### Requirement
> "Tree-walking RPCs designed for agents: resolve a message, list fields, follow a field's type, list dependencies, fetch a single node by path."
> "Searchable by name, type, path, and project."
> "Both humans and AI agents are first-class clients."

This is a v1 requirement and a core differentiator.

### Goal
Design a read API that allows incremental schema traversal without loading full source text into context, type reference following across import boundaries, and cross-registry search.

### Open Question
Three sub-questions: whether FormatPlugin needs read methods, how cross-import type following works, and how search is implemented.

### Suggested Approach
**Add two read methods to `FormatPlugin`:**

```rust
/// List all top-level declarations in a blob with summary info (name, kind, comment).
/// Used by ListDeclarations RPC — does not require loading full field detail.
fn list_declarations(&self, blob: &Blob) -> Result<Vec<DeclSummary>, ReadError>;

/// Return full detail for one named declaration (all fields, options, comments).
/// Used by GetDeclaration RPC.
fn get_declaration(&self, blob: &Blob, name: &str) -> Result<DeclDetail, ReadError>;
```

`DeclSummary` is a small core type (name, kind enum, one-line doc comment). `DeclDetail` is opaque bytes — the core forwards it to the client as-is, and the client-side plugin library deserializes it for display.

**Type reference following:** The core handles this at the request handler level, not in FormatPlugin. When `FollowType` is called for field `profile: UserProfile` in `order.proto`:
1. Core reads the field's type name (`UserProfile`) from the blob via `get_declaration`
2. Core looks up `deps/<project>/<repo>/order.proto/Order@<commit>` to find which schema declares `UserProfile`
3. Core resolves the `resolved_commit` for that import
4. Core loads the `UserProfile` blob from that commit
5. Core calls `get_declaration` on it and returns the result

**Search:** Add a dedicated `search/` KV namespace populated alongside `index/` during mutations:
```
search/<name>/<project>/<repo>/<schema> → declaration kind
```
This enables prefix-scan by declaration name across all repos. Cross-project search scans all entries under `search/<name>/`. The namespace is scoped so per-project/repo searches are cheap prefix scans.

**gRPC RPCs to add:**
```proto
rpc ListDeclarations(ListDeclarationsRequest) returns (ListDeclarationsResponse);
rpc GetDeclaration(GetDeclarationRequest)     returns (GetDeclarationResponse);
rpc FollowType(FollowTypeRequest)             returns (FollowTypeResponse);
rpc ListDependencies(ListDependenciesRequest) returns (ListDependenciesResponse);
rpc Search(SearchRequest)                     returns (SearchResponse);
```

---

## OQ-06: Auth Integration in the Mutation Flow

### Context
Neither the single-mutation nor the transaction flow includes an authentication or authorization check. The architecture diagram notes "auth traits" as a core component, but neither the trait interface nor its position in the request lifecycle is specified.

### Requirement
> "AuthN: trait-based, configurable. A no-op implementation ships in-tree for getting started."
> "AuthZ: required for project management. Trait-based, with a default implementation that enforces project-scoped roles."

### Goal
Define where auth runs in the request lifecycle, what the traits look like, and how the no-op implementation enables zero-config getting-started.

### Open Question
Three sub-questions: position in the flow, the AuthZ trait interface, and whether branch-level permissions are needed.

### Suggested Approach
**Position:** Auth runs after the idempotency check and before the CAS check. If the idempotency key matches in step 1, auth is skipped — the request was already authorized when it first succeeded.

Updated flow steps 1–2:
```
1. Check idempotency key → if known, return stored result (STOP)
2. AuthN: extract Identity from request metadata (token, cert, etc.)
3. AuthZ: check Identity has required Action on ResourcePath
4. CAS check: refs/...<branch> == base_revision?
...
```

**Trait interface:**
```rust
trait AuthnProvider: Send + Sync + 'static {
    fn identify(&self, metadata: &RequestMetadata) -> Result<Identity, AuthnError>;
}

trait AuthzPolicy: Send + Sync + 'static {
    fn check(&self, caller: &Identity, action: Action, resource: &ResourcePath) -> Result<(), AuthzError>;
}

enum Action { Read, Write, Force, ManageProject, ManageRepo }
```

`--force` maps to `Action::Force`. The no-op implementations return `Identity::Anonymous` and `Ok(())` respectively, allowing all operations without configuration.

**Branch-level permissions:** Not in v1. The protected-branch model (Section 5.2) handles the primary use case (restricting who can push to `main`) via the `Maintainer` role check at the AuthZ layer. Full branch-level ACLs are deferred to v2.

---

## OQ-07: `GetDescriptors` Transitive Import Resolution

### Context
`GetDescriptors` must return a self-contained descriptor artifact (e.g. a Protobuf `FileDescriptorSet`) that includes all transitive imports. The current design calls `FormatPlugin::print` on one blob, which produces one file's source text — insufficient for a multi-file descriptor bundle.

### Requirement
> "Local codegen via CLI: client pulls descriptors and generates code on the user's machine."

### Goal
`GetDescriptors` returns a descriptor artifact that codegen tools can consume without separately fetching imports.

### Open Question
Two sub-questions: the transitive closure algorithm, and whether FormatPlugin needs a multi-blob descriptor generation method.

### Suggested Approach
**Add a `generate_descriptors` method to `FormatPlugin`:**

```rust
/// Produce a self-contained descriptor artifact from a transitive closure of blobs.
/// For Protobuf: returns a serialized FileDescriptorSet (binary proto).
/// For FlatBuffers: returns a bundle of reconstructed .fbs source files.
/// For OpenAPI: returns the resolved YAML document with all $ref inlined.
fn generate_descriptors(
    &self,
    blobs: &HashMap<SchemaPath, Blob>,  // full transitive closure, pre-loaded by core
) -> Result<Bytes, DescriptorError>;
```

**The core performs the transitive closure** as a BFS before calling the plugin:
```
1. Start: { target_schema → blob at requested ref }
2. For each blob in the frontier:
   a. Extract imports from the blob's AST (using get_declaration or a new imports() method)
   b. For each import, resolve its resolved_commit
   c. Load the imported blob at that resolved_commit
   d. Add to the closure if not already visited (cycle detection via visited set)
3. Pass the full closure HashMap to plugin.generate_descriptors()
```

The plugin produces the descriptor artifact programmatically from the AST blobs — no external tools (`protoc`, `flatc`) are required on the server. The Protobuf plugin builds a `FileDescriptorSet` directly from the AST using `prost`. This is feasible because schemahub's AST captures all information needed to construct the descriptor.

---

## OQ-08: Storage Backend Choice

### Context
The requirements defer the storage backend decision. Two embedded Rust KV stores are candidates (`jammdb`, `redb`), plus a git-backed option. The choice must support ACID transactions with one writer / many concurrent readers, prefix scanning, and operational simplicity.

### Requirement
> "Storage backend is a trait. Choice between a KV-style backend (BoltDB-shaped, e.g. `jammdb` / `redb`) versus a real-git-backed backend is deferred to detailed design."

### Goal
Choose a backend for v1 with the right trade-offs, knowing the storage trait allows swapping later.

### Open Question
`jammdb` vs `redb` vs git-backed, with the OQ-02 scoping decision influencing the choice.

### Suggested Approach
Use **`redb`** for v1.

Reasons:
- Pure Rust, no C bindings, cross-platform
- MVCC: multiple concurrent readers without blocking, one writer at a time — matches schemahub's read-heavy access pattern
- Rust-native API: typed tables with compile-time key/value type checking, reducing serialization bugs
- Actively maintained with a stable v1 API
- Single file: simple to backup and deploy

`jammdb` is a close alternative but `redb`'s MVCC is a meaningful advantage for a read-heavy registry workload (many agents reading schemas concurrently while mutations are infrequent).

The **git-backed option is deferred**. Its main advantage (native git tooling works on the store) conflicts with schemahub's goal of storing AST blobs (not text files) — `git log` on binary AST blobs produces unreadable output. If a future requirement emerges for git-native storage, the trait allows implementing it without changing the rest of the system.

---

## OQ-09: `PreviewCodegen` Implementation Path

### Context
`PreviewCodegen` renders generated code server-side for inspection. The requirements say it reuses `codegen-infra`, `protobuf-rs`, and `flatbuffers-rs`. But the `FormatPlugin` trait has no codegen method — it's unclear whether codegen is a plugin responsibility or lives outside the plugin abstraction.

### Requirement
> "Server-side preview: an RPC that renders generated code on demand for inspection. No files written; response is the rendered text."

### Goal
Define where codegen logic lives and how `PreviewCodegen` is implemented without spawning external processes.

### Open Question
Option A: `generate_code` method on FormatPlugin. Option B: separate codegen layer outside the plugin.

### Suggested Approach
Use **Option A — add `generate_code` to `FormatPlugin`**:

```rust
fn generate_code(
    &self,
    blobs: &HashMap<SchemaPath, Blob>,  // transitive closure (same as generate_descriptors)
    language: Language,
) -> Result<String, CodegenError>;
```

The Protobuf plugin calls `protobuf-rs` internally. The FlatBuffers plugin calls `flatbuffers-rs`. No external process is spawned. The core calls the plugin once with the full transitive closure (already computed for `GetDescriptors` — the same traversal logic is reused).

`Language` is an enum: `Rust`, `Go`, `TypeScript`, `Python`, etc. Plugins only implement the languages they support; unsupported languages return `CodegenError::UnsupportedLanguage`.

OpenAPI codegen (generating HTTP clients) is **deferred to v2** along with the OpenAPI mutation API. The OpenAPI plugin's `generate_code` returns `UnsupportedLanguage` for all inputs in v1.

This keeps codegen collocated with format knowledge (the Protobuf plugin knows how to generate Rust from Protobuf, not the core) and avoids introducing a separate codegen layer that would need its own abstraction.

---

## OQ-10: Merge Conflict Resolution

### Context
Section 3.2 describes three-way merge but defers conflict resolution strategy. The design doesn't specify what the Merge RPC returns on conflict, how clients resolve it, or whether any conflicts are auto-resolvable.

### Requirement
> "Git-style semantics: commits, branches, tags, history, diff, merge."

### Goal
Define the merge experience for v1, keeping complexity manageable while providing a clear path to richer merge in v2.

### Open Question
Three sub-questions: auto-merge candidates, what the RPC returns on conflict, and whether merge is in v1 scope at all.

### Suggested Approach
**v1 supports fast-forward merges only.** Full three-way merge with conflict resolution is deferred to v2.

The `Merge` RPC first checks whether the source branch is a direct ancestor of the target branch (fast-forward check):
- **Fast-forward possible:** update the target branch ref to point to the source branch HEAD. No new commit created. Returns the new commit hash.
- **Not fast-forward:** return a `MergeConflict` error that lists the commits that diverged on each branch. The client must resolve manually — typically by rebasing their branch onto the target and creating a new commit.

This is the same model as `git merge --ff-only`. It forces teams to keep branches up to date before merging, which is compatible with the CAS-based mutation model.

A `Rebase` RPC (re-apply commits from one branch onto another, one commit at a time) can be added in v1 as a complementary operation without requiring full conflict resolution logic. Each rebased commit goes through the normal mutation path and can produce a `ConflictError` the client resolves commit-by-commit.

---

## OQ-11: Idempotency Key TTL and Scope

### Context
Every mutation carries an idempotency key. The design doesn't specify TTL, scope, or whether idempotency entries should be GC roots.

### Requirement
> "Re-sending the same request with the same key on the same branch is a no-op that returns the original result."

### Goal
Define a TTL and scope that covers realistic retry windows without indefinitely retaining entries or complicating GC.

### Open Question
TTL duration, key scope (global vs per-repo), and whether entries should be GC roots.

### Suggested Approach
- **TTL:** 24 hours, configurable per deployment. Covers even very slow network recovery scenarios (multi-hour outages). Stored as `expires_at_unix` in the idempotency entry.
- **Scope:** `idempotency/<project>/<repo>/<key>`. Per-repo scoping aligns with the OQ-02 namespace design and allows per-repo TTL configuration in the future.
- **Stored value:** Only the commit hash (for success) or the error code + message (for failures). Not the full response payload. If the client needs the schema state, it reads from the commit hash directly. This keeps idempotency entries small.
- **GC roots:** Idempotency entries are **not** GC roots. The commit hash they reference is already reachable via the branch ref. If the ref is deleted (branch deleted), the commit becomes unreachable regardless of the idempotency entry. GC expiry of the idempotency entry and GC of the blobs are independent events. This simplifies the GC root set.

Cleanup of expired entries happens lazily — a background sweep removes entries where `expires_at_unix < now()`. This sweep runs as part of the `RunGC` admin RPC.

---

## OQ-12: Transaction Size and Timeout Limits

### Context
Unbounded transactions risk memory exhaustion and KV backend limits. The design flags this as a required design decision but provides no numbers.

### Requirement
> "Transaction size limits and timeout behavior" is listed as a flagged design concern in the requirements.

### Goal
Define limits that prevent resource exhaustion without blocking legitimate large refactors.

### Open Question
Maximum operations per transaction, maximum schemas touched, server-side timeout, and behavior on timeout.

### Suggested Approach
- **Maximum operations per transaction:** 500
- **Maximum schemas touched per transaction:** 20
- **Server-side timeout:** 30 seconds for the entire transaction (steps 1–10 combined)
- **Timeout behavior:** return `DeadlineExceeded` immediately. No partial application (the KV transaction is not committed). No checkpoint mechanism. The client must retry with a smaller transaction.

These limits are **server-wide defaults**, configurable via server config at startup. They are validated at step 3 of the transaction flow — if exceeded, return `InvalidArgument` before any processing begins.

**Rationale for 500 operations:** A rename of a type used in 100 messages, each with 4 referencing fields, is ~400 operations. 500 provides headroom. For truly massive refactors (e.g. renaming a package used across 500 messages), the client should split the work across multiple transactions on a feature branch and merge.

Resolves OQ-20 (pending/ threshold) as a consequence: `pending/` cleanup threshold = 10 minutes = 30 seconds × 20 safety factor.

---

## OQ-13: Protobuf Field Type Change Compatibility

### Context
The compatibility table in Section 4.2 says `Change field type (compatible wire types) → depends` for all three directions. "Depends" is undefined, leaving the compatibility checker with no specified behavior for a critical mutation category.

### Requirement
> "Server enforces backwards-compatibility rules per format on push."

### Goal
Replace "depends" with explicit, enumerated rules in the compatibility checker.

### Open Question
Should the checker implement the wire-type compatibility matrix, or treat all type changes as breaking by default with an allowlist of safe pairs?

### Suggested Approach
Implement the **wire-type compatibility matrix with a conservative allowlist**.

**Rule:** type changes that cross wire-type boundaries are always breaking under all three directions, and are rejected by the compatibility checker regardless of `--force` being absent (they are also rejected by `apply_mutation` at the mutation validation layer, so they never reach the compatibility checker).

**Type changes within the same wire type** are evaluated by an explicit allowlist of known-safe pairs:

| Change | BACKWARD | FORWARD | FULL | Reason |
|--------|----------|---------|------|--------|
| `int32` → `int64` | ✓ | ✓ | ✓ | Same varint wire type; values fit |
| `uint32` → `uint64` | ✓ | ✓ | ✓ | Same varint wire type |
| `sint32` → `sint64` | ✓ | ✓ | ✓ | Same varint wire type (zigzag) |
| `string` → `bytes` | ✓ | ✓ | ✓ | Same length-delimited wire type |
| `enum` → `int32` | ✓ | ✓ | ✓ | Enums are varints |
| `int64` → `int32` | ✓ | ✗ | ✗ | Truncation risk for new data |
| All other same-wire-type pairs | ✗ | ✗ | ✗ | Conservative default |

Any type change not in the allowlist is treated as breaking under all directions. The allowlist can be extended in future versions as new safe pairs are identified.

---

## OQ-14: GC Race Condition — Age Threshold

### Context
The `pending/` mechanism protects in-flight mutation blobs from GC. However, a residual race exists: if GC's mark phase completes before a mutation's KV transaction commits, the sweep phase may delete a newly committed blob that the mark phase did not see.

### Requirement
> (Implicit) GC must not delete blobs reachable from any ref.

### Goal
Close the race without requiring complex coordination between GC and mutation writers.

### Open Question
Option A: age threshold (simple). Option B: coordinated GC epochs (tighter reclamation).

### Suggested Approach
Use **Option A — age threshold**.

GC never sweeps any object whose `created_at` timestamp is less than **1 hour** old. Any blob committed during a GC run falls within the age window and is protected even if GC's mark phase missed it.

Implementation: each object stored in `objects/<hash>` includes a `created_at_unix` timestamp in its value. The sweep phase skips objects where `now() - created_at_unix < age_threshold`.

The 1-hour threshold is configurable. The cost: objects that are written and immediately orphaned within 1 hour are not reclaimed until the next GC run after the window expires. For a registry workload (low write rate, large object sizes), this is acceptable — GC reclaims space on the next scheduled run without user impact.

Combined with the `pending/` mechanism, this closes the race completely. `pending/` protects blobs during the mutation's in-flight window. The age threshold protects blobs that committed between GC's mark and sweep phases.

---

## OQ-15: `CreateSchema` Name Collision Behavior

### Context
`CreateSchema` is the initial ingestion of a new schema. The design doesn't specify what happens if the schema name already exists on the target branch.

### Requirement
> (Implicit) Creating a schema should not silently overwrite an existing one.

### Goal
Prevent accidental data loss; provide a clean path for intentional updates.

### Open Question
Error on collision, upsert with compatibility check, or explicit `--replace` flag. Also: what is the update RPC for OpenAPI?

### Suggested Approach
**`CreateSchema` always returns `AlreadyExists` if the schema name exists on the target branch.** No flag to override this behavior.

For intentional updates, introduce a **`UpdateSchema` RPC** that is valid for all formats:
- For **Protobuf and FlatBuffers:** `UpdateSchema` accepts the full source text, parses it, and runs the compatibility checker against the current HEAD. This is the "push a whole document" path, complementing the granular mutation API.
- For **OpenAPI:** `UpdateSchema` is the only update path in v1 (no granular mutations).

```proto
rpc CreateSchema(CreateSchemaRequest) returns (CreateSchemaResponse);  // fails if exists
rpc UpdateSchema(UpdateSchemaRequest) returns (UpdateSchemaResponse);  // fails if not exists; runs compat check
```

This makes intent explicit. `CreateSchema` = "I know this is new." `UpdateSchema` = "I know this already exists and I want to update it." Accidental overwrites via `CreateSchema` are impossible. The separate `PushSchema` mentioned in the design doc is replaced by `UpdateSchema` for all formats.

---

## OQ-16: ACL and Role Model

### Context
The requirements specify per-project ACLs with public/private visibility. The design includes "auth traits" in the architecture but no role model is defined.

### Requirement
> "Per-project ACLs and visibility (public / private)."
> "AuthZ: required for project management. Trait-based, with a default implementation that enforces project-scoped roles."

### Goal
Define a minimal v1 role model implemented as a trait, with a shipped default that works without external systems.

### Open Question
Role granularity (project vs repo vs branch), `--force` permission elevation, and the default implementation storage.

### Suggested Approach
**Four roles, project-scoped in v1:**

| Role | Permissions |
|------|------------|
| `Owner` | Everything, including delete project, manage members, change visibility |
| `Maintainer` | Manage repos, push to protected branches, `--force`, change compatibility settings |
| `Writer` | Create/update schemas, push to unprotected branches |
| `Reader` | Read-only: schemas, history, codegen |

**`--force` requires `Maintainer` or above.** Writers cannot bypass compatibility checks. This makes the compatibility model meaningful — it cannot be silently subverted by a team member without elevated access.

**Public projects:** All read RPCs (`GetDescriptors`, `ListDeclarations`, `Search`, etc.) are accessible without authentication. Write RPCs always require authentication.

**Default implementation:** Roles are stored in the KV store under a `roles/<project>/<identity>` namespace, managed via `ManageProject` RPCs. Bootstrapped from a server config file (`schemahub.toml`) at startup that declares the initial Owner for each project. No external identity provider required.

**Repo-level roles** are deferred to v2.

---

## OQ-17: CLI UX

### Context
The CLI is the primary human interface. No command structure, flag conventions, or configuration model is specified.

### Requirement
> "gRPC server (tonic) + CLI client."
> "Local codegen via CLI: client pulls descriptors and generates code on the user's machine."

### Goal
An intuitive CLI for engineers familiar with git, mapping cleanly to the gRPC API.

### Open Question
Command structure, configuration model, and `update-import` UX.

### Suggested Approach
**Resource-first command structure** (`schemahub <resource> <verb>`), similar to `kubectl`:

```bash
# Schema lifecycle
schemahub schema create user.proto                        # CreateSchema (infers format from extension)
schemahub schema update user.proto                        # UpdateSchema (whole-document push)
schemahub schema pull  payments/core-api/user.proto       # print reconstructed source to stdout
schemahub schema delete payments/core-api/user.proto

# Granular mutations (Protobuf / FlatBuffers)
schemahub field add    payments/core-api/user.proto  UserRequest  email:string:3
schemahub field remove payments/core-api/user.proto  UserRequest  email
schemahub field rename payments/core-api/user.proto  UserRequest  email  email_address
schemahub message rename payments/core-api/user.proto  UserRequest  CreateUserRequest

# Version control
schemahub log    payments/core-api/user.proto
schemahub diff   payments/core-api/user.proto  main..feature/xyz
schemahub branch create feature/xyz --from main
schemahub branch list   payments/core-api
schemahub merge  feature/xyz --into main          # fast-forward only in v1
schemahub tag    create v1.0.0 --commit a3f9c2d

# Imports
schemahub import update order.proto user.proto    # re-pin to latest on default branch
schemahub import update order.proto user.proto --to-commit a3f9c2d
schemahub import update order.proto user.proto --to-tag v1.0.0

# Codegen
schemahub codegen get     payments/core-api/user.proto --lang rust --out ./gen/
schemahub codegen preview payments/core-api/user.proto --lang rust
```

**Configuration:** `~/.schemahub/config` (TOML), with env var overrides for CI:
```toml
[default]
server   = "https://registry.example.com"
token    = "..."

[staging]
server   = "https://staging-registry.example.com"
token    = "..."
```
`SCHEMAHUB_SERVER` and `SCHEMAHUB_TOKEN` override the active profile. `--profile staging` selects a non-default profile. All commands accept `--branch` and `--project` flags; defaults come from a `.schemahub` file in the working directory.

---

## OQ-18: OpenAPI Granular Mutation API (v2)

### Context
The OpenAPI mutation surface is deferred from v1. In v1, OpenAPI schemas are pushed as whole documents. The concern is whether the v1 AST design precludes a future granular API.

### Requirement
> "OpenAPI mutation surface is deferred to detailed design — the shape (paths, operations, components, schemas) differs enough from proto / fbs that it warrants its own RPC set."

### Goal
Ensure the v1 AST design does not require a data migration when granular OpenAPI mutations are added in v2.

### Open Question
Is `FormatPlugin::parse(source: &str) -> Blob` sufficient for v1 OpenAPI, and does the resulting AST match what future granular mutations would produce?

### Suggested Approach
Yes — `parse` is sufficient for v1, **but the OpenAPI AST model must be designed now**, before implementation.

The AST produced by `parse` in v1 must be **identical in structure** to what a hypothetical `AddEndpoint` or `RemoveParameter` mutation would produce in v2. If the AST is under-specified now and redesigned for v2, every existing OpenAPI blob requires migration.

Concrete constraint: every element in the OpenAPI AST must be individually addressable by a stable path, following the OpenAPI document structure:
```
/paths/{/users}/{GET}/parameters/{limit}
/paths/{/users}/{POST}/requestBody/content/{application/json}/schema/properties/{email}
/components/schemas/{User}/properties/{id}
```

These paths define the future granular mutation operation identifiers. Designing the AST to match this path model now ensures v2 mutations can target AST nodes without changing the storage format.

**Action:** The OpenAPI AST schema (the blob structure) must be fully specified as part of the v1 design work, even though the mutation API using it is deferred.

---

## OQ-19: Blob Migration for Breaking AST Changes

### Context
Section 3.5 defines lazy migration for additive AST changes. Breaking AST changes (splitting a field, restructuring a message) require explicit migration functions. The migration registry, testing strategy, and rollback policy are unspecified.

### Requirement
> (Implicit) schemahub must be upgradeable without losing historical schema data.

### Goal
Define how breaking AST changes are shipped and tested without requiring offline bulk migrations.

### Open Question
Migration function registry location, testing requirements, and rollback policy.

### Suggested Approach
**Each plugin maintains a static migration chain** compiled into the binary:

```rust
// In the Protobuf plugin crate
static MIGRATIONS: &[(u32, u32, fn(&[u8]) -> Vec<u8>)] = &[
    (1, 2, migrate_v1_to_v2),
    (2, 3, migrate_v2_to_v3),
];
```

When a blob is read with `blob_version = N` and the current version is `M`, the plugin applies `migrate_N_to_N+1`, then `migrate_N+1_to_N+2`, ..., up to `migrate_M-1_to_M` in sequence. Migrations are pure functions: `&[u8] → Vec<u8>`.

**Testing requirement:** Each migration function must have:
1. A **round-trip property test**: serialize a representative set of AST values at version N, migrate to N+1, and verify all fields are present and semantically equivalent.
2. A **golden file test**: a fixed binary blob at version N is migrated and compared byte-for-byte to a checked-in expected output. This prevents silent regressions.

**Rollback policy:** schemahub releases that introduce a new `blob_version` are **non-rollback-able** once any blob has been lazily migrated. This is documented explicitly in the release notes. The minimum supported `blob_version` per release is tracked in the changelog. Operators who need rollback capability must restore from a backup taken before the migration-introducing release.

To minimize migration exposure: require AST changes to be additive wherever possible. A PR introducing a new `blob_version` requires a second reviewer sign-off.

---

## OQ-20: `pending/` Entry Cleanup Threshold

### Context
Stale `pending/` entries (from crashed mutations) are cleaned up before GC runs. The threshold must exceed the longest possible mutation execution time, which depends on transaction limits (OQ-12).

### Requirement
> (Implicit) GC must not clean up pending entries from live in-flight mutations.

### Goal
Choose a threshold that safely covers all legitimate in-flight mutations while not accumulating stale entries indefinitely.

### Open Question
The threshold depends on OQ-12 (transaction timeout) and clock skew tolerance.

### Suggested Approach
Set the cleanup threshold to **10 minutes**, derived from OQ-12's 30-second transaction timeout:

```
threshold = transaction_timeout × safety_factor
10 min    = 30 sec              × 20
```

The safety factor of 20 covers: slow machines under load, KV write latency spikes, and minor clock skew between server restarts.

Document the constraint explicitly in the server configuration:
> `pending_cleanup_threshold` must be set to at least 3× `transaction_timeout`. The default is 10 minutes (20× the default 30-second transaction timeout).

If operators increase `transaction_timeout` for large-deployment workloads, `pending_cleanup_threshold` must be updated proportionally. The server validates this constraint at startup and refuses to start if violated.

This question is **resolved as a direct consequence of OQ-12** — the two must be set together.
