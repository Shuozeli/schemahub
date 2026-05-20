# schemahub — Design

> This document specifies *how* schemahub is built. It is informed by `requirements.md` and resolves all open questions documented in `open-questions.md` (OQ-01 through OQ-20). Where a design decision traces to a specific OQ, it is noted inline.

---

## 1. Architecture Overview

schemahub is split into two layers with a clean trait boundary between them.

```
┌─────────────────────────────────────────────────────────┐
│                     schemahub core                      │
│                                                         │
│  version control  │  mutation API  │  storage engine    │
│  (commits, refs,  │  (idempotency, │  (KV object store, │
│   branches, tags) │   OCC, txns)   │   GC, namespaces)  │
│                                                         │
│  project/repo/schema namespacing  │  auth traits        │
└────────────────────┬────────────────────────────────────┘
                     │  FormatPlugin trait
┌────────────────────▼────────────────────────────────────┐
│                format-specific layer                    │
│                                                         │
│   ┌─────────────┐  ┌─────────────┐  ┌───────────────┐  │
│   │  Protobuf   │  │ FlatBuffers │  │    OpenAPI    │  │
│   │             │  │             │  │               │  │
│   │ parser      │  │ parser      │  │ parser        │  │
│   │ AST model   │  │ AST model   │  │ AST model     │  │
│   │ diff engine │  │ diff engine │  │ diff engine   │  │
│   │ compat chkr │  │ compat chkr │  │ compat chkr   │  │
│   │ printer     │  │ printer     │  │ printer       │  │
│   │ mut. valid. │  │ mut. valid. │  │ mut. valid.   │  │
│   └─────────────┘  └─────────────┘  └───────────────┘  │
└─────────────────────────────────────────────────────────┘
```

**The core layer knows nothing about Protobuf, FlatBuffers, or OpenAPI.** It operates on opaque blobs and delegates all format-specific work to the plugin via the `FormatPlugin` trait. This allows new formats (e.g. SQL DDL, Thrift) to be added without touching the core.

---

## 2. The `FormatPlugin` Trait

This is the boundary between the two layers. Every format implements this trait.

```rust
trait FormatPlugin: Send + Sync + 'static {
    /// Unique identifier for this format, e.g. "protobuf", "flatbuffers", "openapi"
    fn format_id(&self) -> &'static str;

    /// Parse source text into a format-specific AST blob.
    /// The blob is an opaque byte sequence to the core; only the plugin can interpret it.
    fn parse(&self, source: &str) -> Result<Blob, ParseError>;

    /// Reconstruct canonical source text from an AST blob (deterministic round-trip).
    fn print(&self, blob: &Blob) -> Result<String, PrintError>;

    /// Compute a semantic diff between two AST blobs.
    /// Returns a list of typed changes (field added, message renamed, etc.)
    fn diff(&self, old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError>;

    /// Validate and apply a single mutation against the current AST blob.
    /// Returns the new AST blob if valid, or a typed error if not.
    fn apply_mutation(
        &self,
        blob: &Blob,
        mutation: &Mutation,
    ) -> Result<Blob, MutationError>;

    /// Apply a sequence of mutations across one or more schemas.
    /// Called by the core for transactions. Intermediate states may be inconsistent;
    /// only the final state of each blob is validated.
    ///
    /// Input: all schemas that may be touched, keyed by schema path.
    /// Output: only the schemas that were actually changed (unchanged schemas are omitted).
    ///
    /// v1 constraint: all schemas in one transaction must belong to the same format.
    /// The core validates this before calling; the plugin may assume it.
    fn apply_mutations(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        mutations: &[Mutation],
    ) -> Result<HashMap<SchemaPath, Blob>, MutationError>;

    /// Check compatibility between old and new AST blobs under the given rules.
    /// Called once per changed schema after apply_mutations completes.
    fn check_compatibility(
        &self,
        old: &Blob,
        new: &Blob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>>;

    // ── Read / exploration methods (OQ-05) ──────────────────────────────────

    /// List all top-level declarations in a blob with summary info (name, kind, comment).
    /// Used by the ListDeclarations RPC. Does not require loading full field detail.
    fn list_declarations(&self, blob: &Blob) -> Result<Vec<DeclSummary>, ReadError>;

    /// Return full detail for one named declaration (all fields, options, comments).
    /// The returned bytes are opaque to the core and forwarded to the client.
    fn get_declaration(&self, blob: &Blob, name: &str) -> Result<DeclDetail, ReadError>;

    /// Extract all import paths from a blob (for transitive closure BFS).
    fn imports(&self, blob: &Blob) -> Result<Vec<Import>, ReadError>;

    // ── Codegen methods (OQ-07, OQ-09) ──────────────────────────────────────

    /// Produce a self-contained descriptor artifact from a transitive closure of blobs.
    ///   Protobuf  → serialized FileDescriptorSet (binary proto)
    ///   FlatBuffers → bundle of reconstructed .fbs source files
    ///   OpenAPI   → resolved YAML with all $ref inlined
    /// The core pre-computes the transitive closure and passes it here.
    fn generate_descriptors(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
    ) -> Result<Bytes, DescriptorError>;

    /// Render generated code for a given language.
    /// Uses the same transitive closure as generate_descriptors.
    /// Returns CodegenError::UnsupportedLanguage for languages not supported by this plugin.
    fn generate_code(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        language: Language,
    ) -> Result<String, CodegenError>;
}
```

The `Blob` type is `Vec<u8>` to the core — it stores and retrieves it opaquely. Only the plugin that produced it can deserialize it.

---

## 3. Core Layer Design

### 3.1 Storage: Git-Style Object Model

The core storage model mirrors git's internal object model but stores AST blobs instead of file bytes. All objects are content-addressed by SHA-256.

**Storage backend:** `redb` (OQ-08). Pure Rust, MVCC (many concurrent readers, one writer), typed tables, single-file operational simplicity. The storage backend is a trait — `redb` is the v1 implementation.

#### Object types

```
Blob    → serialized AST for one top-level declaration
          (one message, one enum, one service, one table, etc.)
          key = sha256(content), value = raw bytes + created_at_unix timestamp

Tree    → snapshot of a directory level: entries are either SubTree or Blob pointers
          key = sha256(content), value = serialized tree entries

Commit  → { tree_hash, parent_hashes[], timestamp, author,
             message, force: bool, format_id: String }
          key = sha256(content), value = serialized commit

Tag     → { commit_hash, tagger, timestamp, message }  (annotated tags only)
          key = sha256(content), value = serialized tag
```

**Blob granularity:** One blob per top-level declaration (message / enum / service for Protobuf; table / struct / enum / union for FlatBuffers; schema object / path group for OpenAPI). Fields are stored inside the declaration's blob, not as separate blobs. This balances diff granularity against lookup complexity.

#### Two-level tree structure (OQ-01)

A repo contains multiple schema files. One commit must capture the state of the entire repo — not just one file. The `Tree` object uses a **two-level hierarchy** mirroring git's directory model:

```
Commit → root_tree_hash

root_tree  → { "user.proto":  schema_tree_hash_A,   ← SubTree entries
               "order.proto": schema_tree_hash_B }

schema_tree_A → { "UserRequest":  blob_hash_1,      ← Blob entries
                  "UserResponse": blob_hash_2 }

schema_tree_B → { "Order": blob_hash_3 }
```

```rust
enum TreeEntry {
    SubTree { name: String, tree_hash: Hash },
    Blob    { name: String, blob_hash: Hash },
}
```

**Invariant (enforced by core):** The root tree contains only `SubTree` entries (one per schema file). Schema-level trees contain only `Blob` entries (one per top-level declaration). No deeper nesting is permitted in v1.

**Deduplication:** When a single field in `UserRequest` changes, only the `UserRequest` blob and the `user.proto` schema tree are rewritten. The `order.proto` schema tree and its blobs are reused unchanged across the commit — their hashes remain the same, so no new objects are written for them.

#### KV namespaces (OQ-02)

All namespace-sensitive keys are prefixed with `<project>/<repo>/`. The `objects/` namespace is global and unprefixed — content-addressed deduplication is inherently cross-repo.

```
objects/<hash>                                        → Blob / Tree / Commit / Tag bytes
                                                        (global, unprefixed — content-addressed)

refs/<project>/<repo>/heads/<branch>                  → commit hash
refs/<project>/<repo>/tags/<tag>                      → tag hash (lightweight) or tag object hash

deps/<project>/<repo>/<schema>/<decl>@<commit>        → dependency entries
index/<project>/<repo>/<schema>/<decl>/<field>        → { blob_hash, field_position }
search/<name>/<project>/<repo>/<schema>               → declaration kind
                                                        (prefix-scanned for cross-repo search)

idempotency/<project>/<repo>/<key>                    → { commit_hash_or_error, expires_at_unix }
pending/<mutation_id>                                 → [blob_hash, ...]  (GC root for in-flight mutations)

roles/<project>/<identity>                            → role assignment
```

The `deps/` and `index/` namespaces use the schema file name (the root tree entry name) as the `<schema>` component — consistent with the two-level tree structure. The `search/` namespace enables fast prefix-scan by declaration name across all repos (see Section 8).

**The `deps/` and `index/` namespaces are derived indices.** Their canonical source of truth is the AST blobs themselves. A `RebuildIndex` admin RPC scans all blobs reachable from all refs and reconstructs both from scratch. This is the recovery path if an index diverges due to crash or bug.

### 3.2 Version Control

Branches and tags are named refs (pointers to commit hashes). Creating a branch is O(1) — it writes one entry to `refs/<project>/<repo>/heads/`. The commit DAG is append-only; objects are never mutated.

**Diff between commits:** Compare two Trees by walking their entries. Entries with identical blob hashes are unchanged (skip). Entries with differing hashes are passed to `FormatPlugin::diff` for semantic diffing. Entries present in only one tree are additions or deletions.

The `diff` method returns `Vec<SchemaChange>`. The `SchemaChange` type is a two-tier type (OQ-04):

```rust
enum SchemaChange {
    DeclarationAdded   { name: String },
    DeclarationRemoved { name: String },
    DeclarationModified {
        name:   String,
        detail: Bytes,  // format-specific; opaque to core, forwarded to client
    },
}
```

The core uses only `name` for merge conflict detection. The `detail` bytes are forwarded verbatim to the history/log RPC response for client-side display. The compatibility checker does **not** consume `SchemaChange` — it receives the old and new blobs directly via `check_compatibility`.

**Merge — fast-forward only in v1 (OQ-10):** The `Merge` RPC checks whether the source branch is a direct ancestor of the target:
- **Fast-forward possible:** update the target branch ref to the source HEAD. No new commit created. Returns the new commit hash.
- **Not fast-forward:** return a `MergeConflict` error listing the diverging commits. The client must rebase manually.

A complementary `Rebase` RPC re-applies commits from one branch onto another, one commit at a time. Each rebased commit goes through the normal mutation path. Full three-way merge with auto-conflict resolution is deferred to v2.

**Tag semantics:** Two kinds:
- **Lightweight:** ref entry only (`refs/<project>/<repo>/tags/<name>` → commit hash). Zero storage cost.
- **Annotated:** a full `Tag` object (tagger, timestamp, message) stored content-addressed; ref points to the tag object hash.

Tags are **immutable by default.** Moving or deleting a tag requires `--force`, recorded in the tag's audit field. This protects import pins (Section 4.1) from being silently invalidated.

### 3.3 Schema Lifecycle

**Creating a schema** (`CreateSchema`) ingests source text, parses it via the appropriate `FormatPlugin`, and writes the initial blob + Tree + Commit. No compatibility check runs — there is no prior version to compare against.

**If the schema name already exists on the target branch, `CreateSchema` returns `AlreadyExists`. There is no flag to override this (OQ-15).** Use `UpdateSchema` for intentional updates.

```proto
rpc CreateSchema(CreateSchemaRequest) returns (CreateSchemaResponse);
// Returns AlreadyExists if schema name exists on the branch.

rpc UpdateSchema(UpdateSchemaRequest) returns (UpdateSchemaResponse);
// Returns NotFound if schema does not exist. Runs compatibility check.
// For Protobuf/FlatBuffers: accepts full source text, parses, and diffs against HEAD.
// For OpenAPI: the only update path in v1 (no granular mutations).
```

Both RPCs require an explicit `format` field — the server never infers format from content. The CLI infers it from file extension and sets the field automatically.

**Format detection:** `.proto` → Protobuf, `.fbs` → FlatBuffers, `.yaml`/`.json`/`.yml` → OpenAPI. Format is stored permanently in the Commit object. A schema's format cannot change after creation.

**Deleting a schema** (`DeleteSchema`) removes the schema's entry from the root Tree on the current branch and produces a new commit. Before deletion, the server checks `deps/<project>/<repo>/<schema>/` for any schema that currently imports the target on the same branch. If dependents exist, the operation is rejected with a list of them unless `--force` is given. Blobs remain reachable via historical commits and are not GC'd while any ref points to a commit that included the schema.

**OpenAPI AST path-addressability (OQ-18):** Although granular OpenAPI mutations are deferred to v2, the v1 AST model must be designed so that every element is individually addressable by a stable path. The AST structure follows OpenAPI document structure:

```
/paths/{/users}/{GET}/parameters/{limit}
/paths/{/users}/{POST}/requestBody/content/{application/json}/schema/properties/{email}
/components/schemas/{User}/properties/{id}
```

These paths define the future granular mutation operation identifiers. Implementing the v1 AST to match this model ensures v2 mutations can target AST nodes without requiring a data migration.

### 3.4 Mutation API

#### Type definitions (OQ-03)

**gRPC API layer** — clients get compile-time type safety via `oneof`:

```proto
message ApplyMutationRequest {
  string project           = 1;
  string repo              = 2;
  string branch            = 3;
  string base_revision     = 4;
  string idempotency_key   = 5;
  bool   force             = 6;
  oneof operation {
    ProtobufMutation    protobuf_op  = 7;
    FlatBuffersMutation fbs_op       = 8;
    OpenApiMutation     openapi_op   = 9;
  }
}
```

**Internal Rust core** — the gRPC handler converts the matched `oneof` branch into an opaque envelope before passing to the core:

```rust
struct Mutation {
    schema_path: SchemaPath,  // core uses this to load the right blob
    format_id:   String,      // core uses this to select the plugin
    operation:   Bytes,       // format-specific bytes; opaque to core
}
```

The core never inspects `operation`. It reads `format_id`, selects the plugin, and passes `operation` to `apply_mutation`. The plugin deserializes it. Adding a new format requires adding a new `oneof` branch in the proto and a new plugin — the core dispatch loop is unchanged.

#### Transaction limits (OQ-12)

The following limits are server-wide defaults, configurable via server config. They are validated at step 3 of the transaction flow — if exceeded, return `InvalidArgument` before any processing begins.

| Limit | Default |
|-------|---------|
| Maximum operations per transaction | 500 |
| Maximum schemas touched per transaction | 20 |
| Server-side timeout (entire transaction) | 30 seconds |
| Timeout behavior | `DeadlineExceeded`, no partial commit |

#### Single-mutation flow

```
1. Check idempotency key → if known, return stored result (STOP)
2. AuthN: extract Identity from request metadata
3. AuthZ: check Identity has Write (or Force if --force) on <project>/<repo>
4. CAS check: refs/<project>/<repo>/heads/<branch> == base_revision? → ConflictError if not
5. Load current root Tree from HEAD commit; resolve target schema tree and blob
6. Call FormatPlugin::apply_mutation(blob, mutation)
   → plugin validates format-specific rules and returns new blob
7. If branch is protected: call FormatPlugin::check_compatibility(old_blob, new_blob, rules)
   → on violation: return CompatibilityError (unless --force, in which case auth requires Force role)
8. Write pending/<mutation_id> = [new_blob_hash]   (GC root while in-flight)
9. In one KV transaction:
   a. Write new blob to objects/
   b. Write updated schema Tree
   c. Write updated root Tree
   d. Write new Commit (parent = base_revision, force = flag)
   e. Update refs/<project>/<repo>/heads/<branch> (atomic CAS)
   f. Update deps/ and index/ entries
   g. Update search/ entries
   h. Delete pending/<mutation_id>
10. Store idempotency result (commit hash, 24h TTL)
11. Return new commit hash
```

#### Transaction flow

```
1. Check idempotency key → if known, return stored result (STOP)
2. AuthN: extract Identity from request metadata
3. AuthZ: check Identity has Write (or Force) on <project>/<repo>
4. Validate transaction limits (≤ 500 ops, ≤ 20 schemas) → InvalidArgument if exceeded
5. Validate all schemas in the transaction share the same format → InvalidArgument if not
6. CAS check: refs/<project>/<repo>/heads/<branch> == base_revision? → ConflictError if not
7. Load all relevant blobs into a HashMap<SchemaPath, Blob>
8. Call FormatPlugin::apply_mutations(blobs, mutations)
   → plugin applies all mutations in memory; intermediate states are NOT validated
   → returns only the changed schemas
9. If branch is protected: for each changed schema, call
   FormatPlugin::check_compatibility(old_blob, new_blob, rules)
   → on any violation: return CompatibilityError (unless --force)
10. Write pending/<mutation_id> = [all new blob hashes]
11. In one KV transaction:
    a. Write all new blobs to objects/
    b. Write new schema Trees (one per changed schema)
    c. Write new root Tree
    d. Write one Commit (parent = base_revision, force = flag)
    e. Update refs/<project>/<repo>/heads/<branch> (atomic CAS)
    f. Update deps/, index/, search/ entries for all affected schemas
    g. Delete pending/<mutation_id>
12. Store idempotency result (commit hash, 24h TTL)
13. Return new commit hash
```

**Requirements contradiction resolved:** Single mutations and transactions are separate code paths. Single mutations are validated by `apply_mutation` per-step. Transactions validate only the final state via `apply_mutations` + `check_compatibility`. There is no ambiguity about which rule applies.

#### Idempotency key ordering

The idempotency key check **always runs before** auth and the base-revision check. Rationale: if the first attempt succeeded (advancing the branch), a retry with the same key and now-stale base revision must return the original result, not a `ConflictError`. Running idempotency first achieves this — re-authorization is also skipped, preventing spurious rejections if the caller's token changed.

#### Conflict handling

On CAS failure, the server returns `ConflictError` immediately. The server does not retry internally. The client re-reads the latest state, decides whether the mutation still makes sense, and resubmits with a fresh `base_revision` and new idempotency key. This is the same model as `git pull --rebase && git push`.

### 3.5 Blob Encoding and Migration

**Format:** Each format plugin serializes its AST blobs using **Protobuf (prost)**. Every blob type carries a `blob_version: uint32` field as field number 1.

```proto
// Example: internal Protobuf plugin blob
message MessageBlob {
  uint32 blob_version = 1;   // always field 1, always present
  string name         = 2;
  repeated FieldDef fields = 3;
  // ...
}
```

Using Protobuf for internal blob encoding gives built-in forward compatibility: new optional fields added to `MessageBlob` in future schemahub versions deserialize as default values on old blobs.

**Lazy migration — additive changes:**
1. When a blob is read, check `blob_version`
2. If lower than the current version, migrate it in memory before use
3. Write the migrated blob back to the store only when the schema is next mutated
4. Old blobs remain in the store with their original hashes until GC

**Breaking AST changes — migration chain (OQ-19):** Each plugin maintains a static migration chain compiled into the binary:

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
2. A **golden file test**: a fixed binary blob at version N is migrated and compared byte-for-byte to a checked-in expected output.

**Rollback policy:** Releases that introduce a new `blob_version` are non-rollback-able once any blob has been lazily migrated. The minimum supported `blob_version` per release is tracked in the changelog. Operators who need rollback capability must restore from a pre-migration backup.

**Discipline required:** AST model changes must be additive where possible. A PR introducing a new `blob_version` requires a second reviewer sign-off.

### 3.6 GC Strategy

GC is triggered via a `RunGC` admin RPC. It does not run automatically and does not block reads or writes.

**GC roots:**
- All refs (`refs/<project>/<repo>/heads/*`, `refs/<project>/<repo>/tags/*`)
- All `pending/<mutation_id>` entries

**Idempotency entries are NOT GC roots (OQ-11).** The commit hash they reference is already reachable via the branch ref. GC expiry of the idempotency entry and GC of the blobs are independent events. Expired idempotency entries are cleaned up lazily as part of the `RunGC` sweep.

**Algorithm:** Mark-and-sweep.
1. Remove `pending/` entries older than **10 minutes** (the stale-entry cleanup threshold — see below)
2. Collect all GC roots
3. Walk all reachable objects (commit → root tree → schema trees → blobs; tag → commit)
4. Mark them live
5. Skip any object whose `created_at_unix` is less than **1 hour** old (age threshold — see below)
6. Delete all objects in `objects/` not marked live and not age-protected

**Race prevention — two mechanisms (OQ-14):**

*`pending/` entries:* The mutation flow writes `pending/<mutation_id>` before writing any blobs, and deletes it only after the KV transaction commits. A concurrent GC cannot delete a blob being written as part of an in-flight mutation.

*1-hour age threshold:* A residual race exists if GC's mark phase completes before a mutation's KV transaction commits. Any blob committed during a GC run falls within the 1-hour age window and is protected even if the mark phase missed it. The cost: orphaned blobs written and immediately abandoned within 1 hour are not reclaimed until the next GC run after the window expires. For a registry workload, this is acceptable.

**Stale pending entries:** If a mutation crashes between writing `pending/` and committing the KV transaction, the `pending/` entry is never deleted. The cleanup pass at the start of `RunGC` removes `pending/` entries older than the configured `pending_cleanup_threshold`.

**`pending_cleanup_threshold` must be at least 3× `transaction_timeout` (OQ-20).** Default: **10 minutes** (30-second transaction timeout × 20 safety factor, covering slow machines, KV write latency spikes, and clock skew). The server validates this constraint at startup and refuses to start if violated. If operators increase `transaction_timeout`, `pending_cleanup_threshold` must be updated proportionally.

**Idempotency entry TTL (OQ-11):** Idempotency entries expire after **24 hours** (configurable). The `expires_at_unix` field is checked by the cleanup sweep. Stored value: commit hash on success, or error code + message on failure. Not the full response payload — clients re-read schema state from the commit hash directly.

### 3.7 `--force` Semantics

`--force` applies at the **request level** (single mutation or transaction), not per-operation within a transaction. When `--force` is set:
- `check_compatibility` is skipped entirely
- `force: true` is recorded in the Commit object for audit purposes
- Reference integrity checks are also bypassed

**Authorization:** `--force` requires `Action::Force`, which maps to the `Maintainer` role or above. Writers cannot bypass compatibility checks (OQ-06, OQ-16).

`--force` does not bypass idempotency or base-revision checks — those are concurrency-control mechanisms, not compatibility checks.

---

## 4. Format-Specific Layer Design

### 4.1 Import Versioning (all formats)

Imports across schemas use a **hybrid lockfile model**. Every import in an AST blob stores two fields:

```
Import {
    path:             "payments/core-api/user.proto"  // logical path (project/repo/schema)
    resolved_commit:  "a3f9c2d..."                    // pinned commit hash (the lockfile)
}
```

- **Default behavior:** `resolved_commit` is set to the latest commit on the imported schema's default branch at the time the import is added.
- **`UpdateImport` mutation:** Re-resolves the import to the latest commit on the target branch (or a specified tag or commit hash) and updates `resolved_commit`.
- **Reproducibility:** Codegen always uses `resolved_commit`, not the live branch tip. Two codegens from the same commit produce identical output.

The `deps/` KV namespace stores versioned references:

```
deps/<project>/<repo>/<schema>/<decl>@<resolved_commit> → [(importing_schema, importing_decl), ...]
```

This allows the server to answer: "what declarations, at what import versions, reference this declaration?" — necessary for correct rename propagation and compatibility impact analysis.

### 4.2 Protobuf

**AST model:** Custom Rust structs (not `protoc` descriptors). Preserves field declaration order, field numbers, reserved ranges, reserved names, options, and leading/trailing comments. Field numbers are stored on each `FieldDef` and treated as the field's wire identity.

**Mutation validator (`apply_mutation`):**
- `RemoveField`: automatically adds a `reserved` entry for the removed field's number and name. The caller does not need to issue a separate reservation mutation.
- `ReorderFields`: permitted (field numbers, not positions, are the wire identity).
- `ChangeFieldNumber`: rejected unconditionally (always a breaking change).
- `AddField`: requires a field number not currently in use and not in the reserved set.
- `ChangeFieldType` across wire-type boundaries: rejected unconditionally (see compatibility table below).

**Compatibility checker:**

| Change | BACKWARD | FORWARD | FULL |
|--------|----------|---------|------|
| Add optional field | ✓ | ✓ | ✓ |
| Remove field (with auto-reservation) | ✓ | ✗ | ✗ |
| Add enum value | ✓ | ✗ | ✗ |
| Remove enum value | ✗ | ✓ | ✗ |
| Rename field | ✓ | ✓ | ✓ |
| Add RPC to service | ✓ | ✗ | ✗ |
| Remove RPC from service | ✗ | ✓ | ✗ |

**Protobuf field type changes — explicit allowlist (OQ-13):**

Type changes that cross wire-type boundaries are rejected by `apply_mutation` unconditionally (they never reach the compatibility checker). Type changes within the same wire type are evaluated against a conservative allowlist:

| Change | BACKWARD | FORWARD | FULL | Reason |
|--------|----------|---------|------|--------|
| `int32` → `int64` | ✓ | ✓ | ✓ | Same varint wire type; values fit |
| `uint32` → `uint64` | ✓ | ✓ | ✓ | Same varint wire type |
| `sint32` → `sint64` | ✓ | ✓ | ✓ | Same varint wire type (zigzag) |
| `string` → `bytes` | ✓ | ✓ | ✓ | Same length-delimited wire type |
| `enum` → `int32` | ✓ | ✓ | ✓ | Enums are varints |
| `int64` → `int32` | ✓ | ✗ | ✗ | Truncation risk for new data |
| All other same-wire-type pairs | ✗ | ✗ | ✗ | Conservative default |

Any type change not in the allowlist is treated as breaking under all directions. The allowlist can be extended in future versions.

### 4.3 FlatBuffers

**AST model:** Preserves declaration order within tables (slot indices are derived from declaration order and stored explicitly on each field). Structs, tables, enums, and unions are distinct node types. The `deprecated` flag is preserved. Slot indices are the wire identity, not names.

**Mutation validator (`apply_mutation`):**
- `ReorderFields` on a **table**: **rejected unconditionally.** Reordering changes slot indices, which is always a binary breaking change.
- `ReorderFields` on a **struct**: rejected (structs cannot evolve at all).
- `AddField` on a **table**: only permitted if the new field is appended at the end (highest slot index). The validator enforces this even if the caller provides a different position.
- `RemoveField` on a **table**: rejected. Fields must be marked `deprecated` instead via `DeprecateField`.
- Any structural mutation on a **struct**: rejected. Structs are immutable once defined.

**Compatibility checker:**

| Change | BACKWARD | FORWARD | FULL |
|--------|----------|---------|------|
| Add field at end of table | ✓ | ✓ | ✓ |
| Deprecate a field | ✓ | ✓ | ✓ |
| Reorder fields | ✗ | ✗ | ✗ (rejected at mutation layer) |
| Remove field (not deprecate) | ✗ | ✗ | ✗ (rejected at mutation layer) |
| Mutate a struct | ✗ | ✗ | ✗ (rejected at mutation layer) |
| Add enum value | ✓ | ✗ | ✗ |

### 4.4 OpenAPI

**AST model:** Preserves paths, operations (method + path), parameters, request bodies, response schemas, and component schemas. `$ref` references are resolved to their targets in the dependency graph but stored symbolically in the AST. Every element is individually addressable by a stable path (see Section 3.3 for the path model). This is required now to ensure v2 granular mutations do not require a blob migration.

**Mutation surface in v1:** Whole-document push via `CreateSchema` (initial) or `UpdateSchema` (updates). The compatibility checker runs on each `UpdateSchema` by diffing the previous and new AST. Granular mutations are deferred to v2.

**Compatibility checker:** OpenAPI breaking changes are semantic/contractual, not binary.

| Change | BACKWARD | FORWARD | FULL |
|--------|----------|---------|------|
| Add optional request parameter | ✓ | ✓ | ✓ |
| Add required request parameter | ✗ | ✓ | ✗ |
| Remove request parameter | ✓ | ✗ | ✗ |
| Add new endpoint | ✓ | ✗ | ✗ |
| Remove endpoint | ✗ | ✓ | ✗ |
| Remove response field | ✗ | ✓ | ✗ |
| Add optional response field | ✓ | ✓ | ✓ |
| Change response field type | ✗ | ✗ | ✗ |

---

## 5. Compatibility Rules Configuration

### 5.1 Per-repo direction

Compatibility direction is **configurable per repo**. The default for new repos is **FULL** — the strictest level — requiring teams to explicitly relax it.

```rust
struct CompatibilityRules {
    direction: CompatibilityDirection,  // Backward, Forward, or Full
    disabled: bool,                     // skip checks entirely (dev repos)
}
```

**Rationale for FULL as default:** BACKWARD-only is the most common source of subtle bugs (new enum values silently misread by old consumers). FULL forces teams to think about deployment order explicitly and opt down consciously.

### 5.2 Protected branches

Compatibility checks apply only to **protected branches**. Unprotected branches (feature branches, dev branches) have no compatibility checks.

Each repo has a configurable `protected_branches` list supporting exact names and glob patterns:

```toml
[repo."payments/core-api"]
compatibility_direction = "FULL"
protected_branches      = ["main", "release/*"]
```

Mutations on unprotected branches skip steps 7/9 (compatibility check) in both mutation flows. This replaces per-branch compatibility configuration — the mental model matches branch protection in GitHub/GitLab.

---

## 6. Auth Model

### 6.1 Position in request lifecycle (OQ-06)

Auth runs after the idempotency check and before the CAS check (steps 2–3 in both flows). If the idempotency key matches in step 1, auth is skipped — the request was already authorized when it first succeeded.

### 6.2 Trait interface

```rust
trait AuthnProvider: Send + Sync + 'static {
    fn identify(&self, metadata: &RequestMetadata) -> Result<Identity, AuthnError>;
}

trait AuthzPolicy: Send + Sync + 'static {
    fn check(&self, caller: &Identity, action: Action, resource: &ResourcePath) -> Result<(), AuthzError>;
}

enum Action {
    Read,
    Write,
    Force,          // required for --force; Maintainer+ only
    ManageProject,
    ManageRepo,
}
```

The **no-op implementations** return `Identity::Anonymous` and `Ok(())` respectively, allowing all operations without configuration. This is the default for getting-started deployments.

### 6.3 Role model (OQ-16)

Four roles, **project-scoped** in v1:

| Role | Permissions |
|------|------------|
| `Owner` | Everything: delete project, manage members, change visibility |
| `Maintainer` | Manage repos, push to protected branches, `--force` (`Action::Force`), change compatibility settings |
| `Writer` | Create/update schemas, push to unprotected branches |
| `Reader` | Read-only: schemas, history, codegen |

**`--force` requires `Maintainer` or above.** Writers cannot bypass compatibility checks. This makes the compatibility model meaningful — it cannot be silently subverted without elevated access.

**Public projects:** All read RPCs are accessible without authentication. Write RPCs always require authentication.

**Default implementation:** Roles are stored in the KV store under `roles/<project>/<identity>`, managed via `ManageProject` RPCs. Bootstrapped from `schemahub.toml` at startup (initial Owner per project). No external identity provider required.

**Repo-level and branch-level roles** are deferred to v2. The protected-branch model handles the primary use case (restricting who can push to `main`) via the `Maintainer` role check.

---

## 7. Reference Integrity and Rename Propagation

### 7.1 `deps/` index

The `deps/` index is a **derived index** — its canonical source of truth is the import statements within AST blobs. It is pre-computed and maintained for query performance, but always rebuildable via `RebuildIndex`.

### 7.2 Rename propagation flow

When a rename mutation is applied:
1. Look up `deps/<project>/<repo>/<schema>/<old_name>@<resolved_commit>` to find all referencing declarations
2. Include updates to all referencing blobs as part of the same transaction
3. Update `deps/` entries atomically with the rest of the commit

**Cross-repo propagation (v1 limitation):** In v1, rename propagation does not automatically cross repo boundaries. The server notifies the caller of all repos that import the affected declaration (via `deps/`). The caller issues `UpdateImport` mutations in downstream repos manually. Automated cross-repo propagation is a v2 concern.

---

## 8. Schema Exploration API (OQ-05)

### 8.1 FormatPlugin read methods

See Section 2 — `list_declarations`, `get_declaration`, and `imports` are defined on the `FormatPlugin` trait.

- `DeclSummary`: a small core type — name, kind enum (`Message`, `Enum`, `Service`, `Table`, `Struct`, `Union`, `Path`, `Schema`), one-line doc comment.
- `DeclDetail`: opaque `Bytes` — the core forwards it to the client as-is; the client-side plugin library deserializes for display.

### 8.2 gRPC read RPCs

```proto
// List all top-level declarations in a schema file.
rpc ListDeclarations(ListDeclarationsRequest) returns (ListDeclarationsResponse);

// Return full detail for one named declaration (all fields, options, comments).
rpc GetDeclaration(GetDeclarationRequest) returns (GetDeclarationResponse);

// Follow a field's type reference across import boundaries.
// Returns the full DeclDetail for the type declaration.
rpc FollowType(FollowTypeRequest) returns (FollowTypeResponse);

// List all schemas imported by a schema, at their pinned resolved_commit.
rpc ListDependencies(ListDependenciesRequest) returns (ListDependenciesResponse);

// Search declarations by name across schemas and repos.
rpc Search(SearchRequest) returns (SearchResponse);
```

### 8.3 Type reference following

When `FollowType` is called for a field `profile: UserProfile` in `order.proto`:
1. Core reads the field's type name (`UserProfile`) from the blob via `get_declaration`
2. Core looks up `deps/<project>/<repo>/order.proto/Order@<commit>` to find which schema declares `UserProfile`
3. Core resolves the `resolved_commit` for that import
4. Core loads the `UserProfile` blob from that commit
5. Core calls `get_declaration` and returns the result

Cross-repo type following works identically — the `SchemaPath` in the `Import` includes the full `project/repo/schema` path.

### 8.4 Search

The `search/<name>/<project>/<repo>/<schema>` namespace is populated alongside `index/` during every mutation. It enables:
- **Cross-repo search:** prefix-scan `search/<name>/` to find all repos that declare a type named `name`.
- **Per-project search:** prefix-scan `search/<name>/<project>/`.
- **Per-repo search:** prefix-scan `search/<name>/<project>/<repo>/`.

All prefix scans are O(matches) — the KV store supports ordered prefix scanning natively via `redb`'s range API.

---

## 9. Codegen API (OQ-07, OQ-09)

### 9.1 Transitive closure BFS

Before calling `generate_descriptors` or `generate_code`, the core performs a BFS to compute the full transitive import closure:

```
1. Start: { target_schema → blob at requested ref }
2. For each blob in the frontier:
   a. Call plugin.imports(blob) to extract import paths
   b. For each import, resolve its resolved_commit
   c. Load the imported blob at that resolved_commit
   d. Add to the closure if not already visited (cycle detection via visited set)
3. Pass the full closure HashMap<SchemaPath, Blob> to the plugin
```

### 9.2 gRPC RPCs

```proto
// Returns the schema in its native descriptor format, reconstructed from the AST.
//   Protobuf    → FileDescriptorSet (binary, compatible with protoc plugins)
//   FlatBuffers → bundle of reconstructed .fbs source files
//   OpenAPI     → resolved YAML document with all $ref inlined
rpc GetDescriptors(GetDescriptorsRequest) returns (GetDescriptorsResponse);

// Server renders generated code for a given language. No files written.
// Response contains the rendered source text for inspection.
rpc PreviewCodegen(PreviewCodegenRequest) returns (PreviewCodegenResponse);
```

Both RPCs accept a `ref` parameter that can be a branch name, tag name, or commit hash. When a branch name is given, the RPC resolves to the current HEAD at request time.

The Protobuf plugin builds a `FileDescriptorSet` directly from the AST using `prost` — no `protoc` binary required on the server. The FlatBuffers plugin reconstructs `.fbs` source via `print`. The OpenAPI plugin produces a resolved YAML document.

Codegen is not supported for external tools in `PreviewCodegen` — the server renders code using in-tree libraries (`protobuf-rs`, `flatbuffers-rs`). The CLI pipes `GetDescriptors` output to the appropriate toolchain when full codegen is needed.

---

## 10. CLI Design (OQ-17)

**Resource-first command structure** (`schemahub <resource> <verb>`):

```bash
# Schema lifecycle
schemahub schema create user.proto                         # CreateSchema
schemahub schema update user.proto                         # UpdateSchema
schemahub schema pull  payments/core-api/user.proto        # print reconstructed source to stdout
schemahub schema delete payments/core-api/user.proto

# Granular mutations (Protobuf / FlatBuffers)
schemahub field add    payments/core-api/user.proto  UserRequest  email:string:3
schemahub field remove payments/core-api/user.proto  UserRequest  email
schemahub field rename payments/core-api/user.proto  UserRequest  email  email_address
schemahub message rename payments/core-api/user.proto  UserRequest  CreateUserRequest

# Version control
schemahub log    payments/core-api
schemahub diff   payments/core-api  main..feature/xyz
schemahub branch create feature/xyz --from main
schemahub branch list   payments/core-api
schemahub merge  feature/xyz --into main           # fast-forward only in v1
schemahub tag    create v1.0.0 --commit a3f9c2d

# Imports
schemahub import update order.proto user.proto             # re-pin to latest on default branch
schemahub import update order.proto user.proto --to-commit a3f9c2d
schemahub import update order.proto user.proto --to-tag v1.0.0

# Codegen
schemahub codegen get     payments/core-api/user.proto --lang rust --out ./gen/
schemahub codegen preview payments/core-api/user.proto --lang rust
```

**Configuration:** `~/.schemahub/config` (TOML), with env var overrides for CI:

```toml
[default]
server = "https://registry.example.com"
token  = "..."

[staging]
server = "https://staging-registry.example.com"
token  = "..."
```

`SCHEMAHUB_SERVER` and `SCHEMAHUB_TOKEN` override the active profile. `--profile staging` selects a non-default profile. All commands accept `--branch` and `--project` flags; defaults come from a `.schemahub` file in the working directory (similar to `.git/config`).

Format is inferred from file extension (`.proto`, `.fbs`, `.yaml`/`.json`) and set automatically in the RPC.
