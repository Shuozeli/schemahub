<!-- agent-updated: 2026-07-30T04:16:42Z -->
# schemahub — gRPC API (v2: Compilers + Jujutsu-style JJ)

> This document specifies the gRPC API surface **as implemented**: all services, RPCs, request/response shapes, and mutation types. The `.proto` files described here are the external contract that the CLI, AI agents, and any future client libraries depend on. The server (`schemahub-server`) translates these wire types into the internal `Mutation` envelope and `Core` calls described in `design.md`.
>
> It supersedes the v1 git-style API design (preserved in git history). Two project-level decisions drive the v2 surface (see `design.md`, `requirements.md`):
>
> 1. **Jujutsu-style JJ.** Writes no longer reject on a base-revision CAS mismatch. Every successful schema write returns a stable **`change_id`** (durable identity that survives rewrite/rebase) alongside the new commit. Unprotected bookmarks may report **`conflicted_decls`** as first-class conflicts; protected bookmarks reject a conflicted exact final tree before publication. A new **`HistoryService`** exposes the operation log, `Undo`, and conflict render/resolve.
> 2. **Compilers, not plugins.** "branches" map to jj **bookmarks** (the server keeps the `RefService`/branch names as an alias over bookmarks); per-declaration storage; the format-specific work is owned by a `Compiler` (`design.md` §2).
>
> **Source-of-truth note.** Where this doc and `design.md` disagree, the protos + server win — they are the implementation. Divergences are flagged inline with **AS-BUILT**.
>
> **1.0 compatibility boundary.** The `schemahub.v1` protobuf package is the
> designated public API for humans, agents, automation, and generated clients.
> The server's unversioned `/api/*` HTTP routes are a same-release GUI BFF, not
> REST bindings for this package and not part of the public API promise. See
> ADR 0002 and `http-api.md`.

---

## 1. File Structure and Package Conventions

The `.proto` files live in `crates/schemahub-api/proto/schemahub/v1/`. The v2 layout split the old monolithic `common.proto` into focused files and added dedicated history and change-control services:

```
proto/schemahub/v1/
  enums.proto              # SchemaFormat, CompatibilityDirection, DeclKind, Role, Language
  resources.proto          # CommitInfo, DeclSummary, SchemaInfo, BranchInfo, TagInfo
  refs.proto               # VersionRef (oneof branch|tag|commit)
  errors.proto             # ConflictDetail, CompatibilityViolation/Error, MutationValidationError, MergeConflictDetail
  mutations.proto          # ProtobufMutation, FlatBuffersMutation, OpenApiMutation
  schema_service.proto     # SchemaService     (lifecycle + mutations)
  ref_service.proto        # RefService        (commits, diff, branches==bookmarks, tags, merge)
  history_service.proto    # HistoryService    (NEW: log, op log, undo, render/resolve conflict)
  change_service.proto     # ChangeService     (durable human/agent change notes)
  exploration_service.proto# ExplorationService (read API)
  codegen_service.proto    # CodegenService    (descriptors, preview)
  serving_service.proto    # ServingService    (immutable revisions + artifacts)
  project_service.proto    # ProjectService    (projects, repos, members/ACL)
  admin_service.proto      # AdminService      (GC, index rebuild, server config, capabilities)
```

**Package:** `schemahub.v1` · **Go option:** `option go_package = "github.com/shuozeli/schemahub/gen/go/schemahub/v1";` · **Rust:** generated via `tonic-build`; all types land in `schemahub_api::schemahub_v1`. All files are `syntax = "proto3"`.

> **AS-BUILT — `refs.proto` vs `resources.proto`:** the design assumed a single `common.proto`. The implementation split shared types: `VersionRef` lives in `refs.proto`; `CommitInfo`/`DeclSummary`/`SchemaInfo`/`BranchInfo`/`TagInfo` in `resources.proto`; the structured error details in `errors.proto`. `ref_service.proto` defines the version-control RPCs (the file `refs.proto` holds only `VersionRef`).

---

## 2. The Jujutsu Concurrency Model on the Wire

This is the single biggest change from v1 and shapes every write RPC.

### 2.1 No base CAS rejection — final publication policy still applies

In v1 a write carried a `base_revision` and the server **rejected** with `FAILED_PRECONDITION` + `ConflictDetail` if it did not match the branch HEAD. In v2 the JJ layer is jj-style:

- Two writes may start from the same immutable state; the backend serializes
  their final load/merge/validate/commit boundary.
- Edits to **different** declarations merge automatically (different per-declaration files in the tree).
- Edits to the **same** declaration produce a **first-class conflict** on an
  unprotected bookmark. On a protected bookmark, the exact conflicted final
  tree returns `FAILED_PRECONDITION` before a commit/operation is published;
  `force` cannot bypass this invariant.

`base_revision` is therefore a causal input rather than a HEAD gate. When
supplied, it must be a retained commit belonging to the target repository;
foreign and unknown commits return `FAILED_PRECONDITION`, while stale retained
commits are accepted. The durable identity of a write is the returned
**`change_id`**, and the durable record of concurrency is the **operation log**
plus the **`conflicted_decls`** field.

> **AS-BUILT — `ConflictDetail` / `MergeConflictDetail` are vestigial.** They remain defined in `errors.proto` for wire compatibility, but the implemented write path does not return either detail. Protected final-tree rejection is a plain `FAILED_PRECONDITION`; unprotected conflicts are persisted as JJ tree state. Treat both detail messages as deprecated.

### 2.2 Durable idempotency receipts

Every direct schema write observes its receipt only after authentication and
authorization. Keys are scoped by operation kind plus project/repository and
bound to the semantic request fingerprint. A literal retry returns the original
commit; changing any bound field while reusing the key returns
`FAILED_PRECONDITION`. Receipts persist in the selected ObjectDb, are bounded to
1,024 entries with a 24-hour completed TTL, and carry correlation attributes on
the JJ operation so post-publication crashes reconcile without a second commit.
See `idempotency.md` for the lease and cleanup protocol.

### 2.2 Common write-response fields

Every mutating RPC on `SchemaService` (and `ResolveConflict` on `HistoryService`) returns:

| Field | Type | Meaning |
|-------|------|---------|
| `new_commit` | `string` | hash of the commit created by this write |
| `change_id` | `string` | the stable jj **change ID** — durable identity of the edit across rewrite/rebase (`design.md` §5.1) |
| `conflicted_decls` | `repeated string` | declaration names that landed **conflicted** on an unprotected bookmark (empty on a clean or any successful protected write). Resolve with `HistoryService.ResolveConflict`. |

`DeleteSchemaResponse` also carries `conflicted_decls`; deleting from an
unprotected stale planning snapshot can conflict with a concurrent edit just as
another schema write can.

---

## 3. `enums.proto`, `resources.proto`, `refs.proto`, `errors.proto` — Shared Types

### 3.1 `enums.proto`

```proto
enum SchemaFormat            { UNSPECIFIED=0; PROTOBUF=1; FLATBUFFERS=2; OPENAPI=3; }   // names prefixed SCHEMA_FORMAT_
enum CompatibilityDirection  { UNSPECIFIED=0; BACKWARD=1; FORWARD=2; FULL=3; DISABLED=4; }
enum Role                    { UNSPECIFIED=0; READER=1; WRITER=2; MAINTAINER=3; OWNER=4; }
enum Language                { UNSPECIFIED=0; RUST=1; GO=2; TYPESCRIPT=3; PYTHON=4; JAVA=5; }

enum DeclKind {
  DECL_KIND_UNSPECIFIED            = 0;
  DECL_KIND_MESSAGE               = 1;   DECL_KIND_ENUM = 2;   DECL_KIND_SERVICE = 3;          // Protobuf
  DECL_KIND_TABLE                 = 4;   DECL_KIND_STRUCT = 5; DECL_KIND_FBS_ENUM = 6; DECL_KIND_UNION = 7;  // FlatBuffers
  DECL_KIND_PATH_ITEM             = 8;   DECL_KIND_COMPONENT_SCHEMA = 9;
  DECL_KIND_COMPONENT_PARAMETER   = 10;  DECL_KIND_COMPONENT_RESPONSE = 11;
  DECL_KIND_COMPONENT_REQUEST_BODY = 12; DECL_KIND_DOCUMENT_METADATA  = 13;                    // OpenAPI
}
```

### 3.2 `resources.proto`

```proto
message CommitInfo {
  string hash                          = 1;
  repeated string parent_hashes        = 2;
  google.protobuf.Timestamp timestamp  = 3;
  string author                        = 4;
  string message                       = 5;
  bool   force                         = 6;
  string format_id                     = 7;   // "protobuf" | "flatbuffers" | "openapi"
}

message DeclSummary { string name = 1; DeclKind kind = 2; string doc_comment = 3; }
// name is the per-declaration key: "UserRequest" (proto/fbs) or "path:/users", "schema:User" (openapi).

message SchemaInfo  { string name = 1; SchemaFormat format = 2; string head_blob = 3; }
message BranchInfo  { string project=1; string repo=2; string name=3; string head_commit=4; bool protected=5; }
message TagInfo     { string project=1; string repo=2; string name=3; string commit_hash=4;
                      bool annotated=5; string tagger=6; string message=7; google.protobuf.Timestamp timestamp=8; }
```

> **AS-BUILT** — `CommitInfo` is reconstructed from the real JJ commit graph
> (see §5.1). The server fills `hash`, `parent_hashes`, `author`, and `message`;
> `timestamp`, `force`, and `format_id` remain unset on this compatibility-shaped
> resource. `HistoryService.Log` is the richer commit/change view and includes
> stable `change_id` plus the stored timestamp.

### 3.3 `refs.proto`

```proto
message VersionRef {
  oneof ref {                 // exactly one set
    string branch = 1;        // HEAD of the bookmark at request time
    string tag    = 2;        // the tagged commit
    string commit = 3;        // pinned commit hash
  }
}
```

> **AS-BUILT — read-path resolution.** Exploration, history, diff, codegen, and
> serving preserve an explicit branch/tag/commit, resolve it exactly once, and
> read the complete response from that immutable snapshot. Omission selects the
> repository's configured default bookmark. Raw commits are accepted only when
> retained by the named repository; the JJ boundary applies the same ownership
> proof to reads and ref publication despite globally deduplicated objects.

### 3.4 `errors.proto` (structured `status.details`)

`CompatibilityViolation` / `CompatibilityError` (compat failure → `FAILED_PRECONDITION`) and `MutationValidationError` (validator reject → `INVALID_ARGUMENT`) are the actively-used detail types. `ConflictDetail` and `MergeConflictDetail` are retained but vestigial under the jj model (§2.1).

### 3.5 `change_service.proto` — durable intent resources

`ChangeRecord` is separate from a JJ commit. It can begin as a note and stores
the authenticated actor, mutable target/base/title/description/external
references/edits, lifecycle state, validation/review/application outputs, ETag,
and timestamps.

```proto
service ChangeService {
  rpc CreateChange(CreateChangeRequest)   returns (ChangeRecord);
  rpc GetChange(GetChangeRequest)         returns (ChangeRecord);
  rpc ListChanges(ListChangesRequest)     returns (ListChangesResponse);
  rpc UpdateChange(UpdateChangeRequest)   returns (ChangeRecord);
  rpc ValidateChange(ValidateChangeRequest) returns (ChangeRecord);
  rpc MarkChangeReady(MarkChangeReadyRequest) returns (ChangeRecord);
  rpc ApproveChange(ApproveChangeRequest) returns (ChangeRecord);
  rpc RejectChange(RejectChangeRequest) returns (ChangeRecord);
  rpc ApplyChange(ApplyChangeRequest) returns (ChangeRecord);
  rpc DeleteChange(DeleteChangeRequest)   returns (google.protobuf.Empty);
  rpc AbandonChange(AbandonChangeRequest) returns (ChangeRecord);
}
```

- Resource names are
  `projects/{project}/repos/{repo}/changes/{change}`; Create/List use the
  `projects/{project}/repos/{repo}` parent.
- Actor fields are output-only and server-derived. `ActorKind` distinguishes
  `HUMAN`, `AGENT`, `SERVICE`, and `ANONYMOUS`; `delegated_by` preserves an
  agent credential's delegating identity without changing authorization.
- `CreateChange` accepts a note-only draft. An empty target defaults to the
  repository's configured default bookmark.
- `external_references` accepts at most 32 ordered, unique, trimmed opaque
  values of at most 2,048 bytes each. This supports URLs and organization-local
  issue, incident, design, or automation IDs without trusting them as actors.
- `ListChanges` is creation-time/name ordered, status-filterable, and paginated
  with parent/filter-bound opaque cursors. Page size defaults to 50 and is
  capped at 200. Each page traverses the matching repository/status index
  through one bounded `ObjectDb` range and validates every returned target;
  existing v1 cursor encoding and observable ordering are unchanged.
- `UpdateChange` requires `google.protobuf.FieldMask` and the current ETag.
  Mutable paths are `target_bookmark`, `base_revision`, `title`, `description`,
  `external_references`, and `edits`. Stale ETags return `ABORTED`.
- `ValidateChange` resolves an immutable JJ base, replays ordered edits through
  the selected compiler, and stores a deterministic digest plus findings.
  `MarkChangeReady` requires that current snapshot to pass.
- Approve/Reject require Maintainer+ and store the authenticated reviewer;
  authors cannot review their own records. Approval is advisory until D3 adds
  per-repository required-review policy.
- `ApplyChange` requires a stable `request_id`. A durable `APPLYING` lease and
  JJ operation attributes correlate the record to its commit. Same-request
  retries return the original receipt or recover it from JJ after a crash.
- Delete and Abandon are soft deletes: the durable record transitions to
  `ABANDONED` and remains readable. An author with Writer access may abandon
  their own record; abandoning another actor's record requires Maintainer+.

The selected `ObjectDb` persists these mutable resources outside JJ's
content-addressed object namespace. Memory, redb, and PostgreSQL implement the
same atomic create/get/list/compare-and-swap contract.

---

## 4. `mutations.proto` — Mutation Operation Types

Granular, typed operations on one schema file. The compiler validates each against the real AST.

### 4.1 `ProtobufMutation` (oneof `operation`)

Field ops: `ProtoAddField`, `ProtoRemoveField` (auto-reserves number+name), `ProtoRenameField`, `ProtoChangeFieldType` (wire-type-compatible allowlist), `ProtoChangeFieldLabel`, `ProtoReorderFields`. Message ops: `ProtoAddMessage`, `ProtoRemoveMessage`, `ProtoRenameMessage`. Enum ops: `ProtoAddEnum`, `ProtoRemoveEnum`, `ProtoAddEnumValue`, `ProtoRemoveEnumValue`, `ProtoRenameEnumValue`. Service ops: `ProtoAddService`, `ProtoRemoveService`, `ProtoRenameService`, `ProtoAddRpc`, `ProtoRemoveRpc`, `ProtoRenameRpc`, `ProtoChangeRpcType`. Import: `ProtoUpdateImport` (add/update/remove with an optional immutable commit/tag pin).

### 4.2 `FlatBuffersMutation` (oneof `operation`)

Field ops: `FbsAddField` (always appended), `FbsDeprecateField`, `FbsRenameField` (rename is safe — wire identity is the slot index), `FbsChangeFieldType`. `RemoveField`/`ReorderFields` are explicitly rejected. Table ops: `FbsAddTable`, `FbsRemoveTable`, `FbsRenameTable`. Enum: `FbsAddEnum`, `FbsRemoveEnum`, `FbsRenameEnum`, `FbsAddEnumValue`, `FbsRemoveEnumValue`, `FbsRenameEnumValue`. Union: `FbsAddUnion`, `FbsRemoveUnion`, `FbsRenameUnion`, `FbsAddUnionMember`, `FbsRemoveUnionMember`. Import: `FbsUpdateImport` (add/update/remove with an optional immutable commit/tag pin).

Union members are typed table references with stable discriminators. Removing a
member preserves the other discriminator values; `NONE` cannot be removed.

### 4.3 `OpenApiMutation` (oneof `operation`)

The implemented OpenAPI surface is **broader than v1's whole-document-only design** — the compiler implements a handful of granular ops:

```proto
message OpenApiMutation {
  string schema_path = 1;
  oneof operation {
    OpenApiPushDocument          push_document            = 2;   // whole-document replace (used by UpdateSchema)
    OpenApiAddPath               add_path                 = 10;
    OpenApiRemovePath            remove_path              = 11;
    OpenApiAddOperation          add_operation            = 20;
    OpenApiRemoveOperation       remove_operation         = 21;
    OpenApiAddComponentSchema    add_component_schema     = 30;
    OpenApiRemoveComponentSchema remove_component_schema  = 31;
  }
}
```

These seven operations are reachable via `ApplyMutation` and
`ApplyTransaction`; `TransactionOp` carries `protobuf_op`, `fbs_op`, or
`openapi_op`. Component removal rejects remaining local `$ref`s, and batch
reference validation runs against the final document.

---

## 5. `ref_service.proto` — Commits, Diff, Branches (== Bookmarks), Tags, Merge

```proto
service RefService {
  rpc GetCommit(GetCommitRequest)       returns (GetCommitResponse);
  rpc ListCommits(ListCommitsRequest)   returns (stream CommitInfo);   // newest first
  rpc Diff(DiffRequest)                 returns (DiffResponse);

  rpc CreateBranch(CreateBranchRequest) returns (CreateBranchResponse);
  rpc DeleteBranch(DeleteBranchRequest) returns (DeleteBranchResponse);
  rpc ListBranches(ListBranchesRequest) returns (ListBranchesResponse);
  rpc GetBranch(GetBranchRequest)       returns (GetBranchResponse);

  rpc CreateTag(CreateTagRequest)       returns (CreateTagResponse);
  rpc DeleteTag(DeleteTagRequest)       returns (DeleteTagResponse);    // requires force=true
  rpc ListTags(ListTagsRequest)         returns (ListTagsResponse);

  rpc Merge(MergeRequest)               returns (MergeResponse);        // real jj merge
}
```

A **branch is a jj bookmark**; the branch name == the bookmark name. `RefService` is the compatibility-shaped face of bookmark operations. Handler: `server/src/services/bookmark.rs`.

### 5.1 Commits & Diff

- **`ListCommits`** walks the real commit graph from `from` (or the configured
  default bookmark), newest-first. `stop_at_commit` is a repository-owned,
  retained exclusive stop; an unreachable stop fails instead of widening the
  requested range. `schema_path` retains only commits whose raw schema subtree
  differs from the first parent, including conflicted-tree changes. Initial
  response metadata contains `x-schemahub-at-commit`, the exact immutable
  traversal root. A scan over 10,000 commits returns `RESOURCE_EXHAUSTED`
  rather than a truncated success.
- **`GetCommit`** resolves the supplied raw hash within the named repository's
  retained history and returns that real commit; foreign/unknown hashes fail.
- **`Diff`** resolves `base` and `head` once, then computes a per-declaration
  semantic diff between those immutable commits. It returns
  `base_commit`, `head_commit`, and `DeclarationChange { change_type:
  "added"|"removed"|"modified", decl_name, detail }`; `detail` carries
  format-specific bytes only for `"modified"`. With no `schema_path`, the
  union of files on both sides is diffed, so whole-file additions/deletions are
  reported rather than skipped.

### 5.2 Branches (bookmarks)

- **`CreateBranch`** creates a bookmark from `from` (default: the repository's configured bookmark); returns `BranchInfo` (`protected` is always reported `false` — protection lives in repo config, §8).
- **`ListBranches`** accepts optional `name_prefix`, `page_size`, and opaque
  `page_token`, reporting each bookmark's first target as `head_commit` in
  stable lexicographical name order. The continuation is bound to branch kind,
  project, repository, and prefix; return it unchanged until
  `next_page_token` is empty. One page lazily materializes at most
  `page_size + 1` entries from the repository-local immutable JJ view.
- **`GetBranch`** performs a direct named lookup rather than enumerating the
  bookmark namespace.
- **`DeleteBranch`** removes the bookmark via `Core::delete_bookmark` (delegating to `Jj::delete_bookmark`).

### 5.3 Tags

- **`CreateTag`** points a tag at `target` (default: the repository's configured bookmark); `annotated` is set when `message` is non-empty.
- Tag names are immutable: creating an existing name returns `ALREADY_EXISTS`
  and leaves its original target unchanged.
- **`ListTags`** uses the same stable, bounded pagination contract as branches;
  its token is tag-kind/project/repository/prefix-bound and cannot be reused
  for a branch page.
- **`DeleteTag`** requires `force=true` (else `FAILED_PRECONDITION` — tags are immutable pins by contract); when set, removes the tag via `Core::delete_tag`.

### 5.4 Merge

`Merge(source_branch → target_branch)` returns the commit `target_branch` points to afterward. **AS-BUILT:** a real jj merge via `Core::merge_idempotent` → `Jj::merge_with_attributes_validated` (`jj_lib::rewrite::merge_commit_trees`), producing a two-parent merge commit whose tree is jj's 3-way merge over the merge base. Same-declaration divergence is stored on an unprotected target; a protected target instead returns `FAILED_PRECONDITION` before publication. `MergeResponse` currently carries only `new_commit`, so an unprotected caller re-reads the resulting commit to inspect conflicts. `message` becomes the merge commit/operation description, and `idempotency_key` uses the same durable correlation/replay contract as SchemaService writes; a known policy rejection releases the pending receipt for immediate retry.

---

## 6. `history_service.proto` — Operation Log, Undo, Conflicts (NEW in v2)

This service did not exist in v1; it is the wire surface for the jj operation log and first-class conflicts. Handler: `server/src/services/history.rs`.

```proto
service HistoryService {
  rpc Log(LogRequest)                     returns (LogResponse);             // commit/change graph
  rpc OpLog(OpLogRequest)                 returns (OpLogResponse);           // operation log (audit)
  rpc Undo(UndoRequest)                   returns (UndoResponse);            // undo last operation
  rpc RenderConflict(RenderConflictRequest)   returns (RenderConflictResponse);
  rpc ResolveConflict(ResolveConflictRequest) returns (ResolveConflictResponse);
}
```

### 6.1 `Log` — commit/change history

`LogRequest { project, repo, VersionRef at, uint32 limit }` →
`LogResponse { repeated LogEntry, string at_commit }`, where:

```proto
message LogEntry { string commit_id=1; string change_id=2; repeated string parents=3;
                   string author=4; string message=5; string timestamp=6; }
```

Each entry exposes both the **`commit_id`** and stable **`change_id`**. The
walk is over the real commit/change graph (`Jj::commit_log`) newest→oldest from
one resolved `at`; `at_commit` identifies that immutable root. An omitted ref
uses the repository's configured default bookmark. `limit = 0` means "use
Core's default", currently 100.

### 6.2 `OpLog` — the audit record

`OpLogRequest { project, repo, uint32 limit }` → `OpLogResponse { repeated OperationRecord }`:

```proto
message OperationRecord { string op_id=1; repeated string parents=2;
                          string description=3; string author=4; string timestamp=5; }
```

Every schemahub write (mutation, transaction, bookmark move, tag, undo, …) is one operation. The JJ returns the chain oldest→newest along the head's parent chain; `limit = 0` returns the full log, `limit = n` trims the front (oldest entries) and keeps the most recent `n` operations.

### 6.3 `Undo`

`UndoRequest { project, repo, author }` → `UndoResponse { undone_op_id }`.
Restores the repo's view to one step further back through the content-op chain.
**AS-BUILT:** undo is a **linear monotonic walk-back stack**, not jj's bare
op-toggle — consecutive `undo` calls step further back (skipping leading `undo`
ops at the head) rather than redoing the previous undo. Undo is itself an
append-only operation. Each restored bookmark is checked against the exact
target tree while the publication guard is held, so undo cannot reintroduce a
conflict or broken live import on a protected bookmark. The legacy wire
`author` field is ignored; the authenticated identity is the audit author
(`schemahub` only for an anonymous allowed caller). `undone_op_id` is the id of
the content operation whose effect was rolled past.

### 6.4 `RenderConflict` — inspect competing sides

`RenderConflictRequest { project, repo, schema_path, declaration_name, VersionRef at }` → `RenderConflictResponse { rendered }`. `rendered` is a human/agent-readable view of the conflicting sides (e.g. `base` / `side 0` / `side 1` fragments via `Compiler::render_conflict`). Returns `FAILED_PRECONDITION` if the declaration is not conflicted. Conflict rendering requires a branch ref because conflicts are mutable bookmark state; omission selects the configured default bookmark, while an explicit tag/commit returns `INVALID_ARGUMENT`.

### 6.5 `ResolveConflict` — submit a resolution

```proto
message ResolveConflictRequest {
  string project=1; string repo=2; string bookmark=3;
  string schema_path=4; string declaration_name=5;
  string resolved_source=6;   // full source of the schema FILE containing the resolved decl
  string author=7; string message=8;
}
message ResolveConflictResponse { string new_commit=1; string change_id=2; }
```

**AS-BUILT:** the client submits the **full source** of the schema file; the server parses it with the compiler selected by file extension, extracts the named declaration's blob, validates it (`Compiler::validate_resolution`), and commits the resolution as one operation. `INVALID_ARGUMENT` if the resolved source does not define `declaration_name`. The legacy request `author` is ignored; audit identity is server-derived.

---

## 7. `schema_service.proto` — Schema Lifecycle and Mutations

```proto
service SchemaService {
  rpc CreateSchema(CreateSchemaRequest)         returns (CreateSchemaResponse);
  rpc UpdateSchema(UpdateSchemaRequest)         returns (UpdateSchemaResponse);
  rpc DeleteSchema(DeleteSchemaRequest)         returns (DeleteSchemaResponse);
  rpc ApplyMutation(ApplyMutationRequest)       returns (ApplyMutationResponse);
  rpc ApplyTransaction(ApplyTransactionRequest) returns (ApplyTransactionResponse);
}
```

All requests carry `project`, `repo`, `branch` (the bookmark), plus
`base_revision` (repository-owned causal input, §2.1) and `idempotency_key`
(durable scoped receipt, §2.2). Handler: `server/src/services/schema.rs`.

### 7.1 Lifecycle (`CreateSchema` / `UpdateSchema` / `DeleteSchema`)

All three are **format-agnostic, whole-document** operations orchestrated by
Core rather than the gRPC handler. Core captures the bookmark as one immutable
planning commit, applies authorization/idempotency/existence/policy checks,
selects the compiler from the schema extension, parses replacement source into
per-declaration objects, and publishes against that same commit. `DeleteSchema`
removes the complete file subtree.

- `CreateSchemaRequest`: `project, repo, branch, schema_name, SchemaFormat format, source, base_revision, idempotency_key`. `format` is required and must match the file extension. Existing schemas return `ALREADY_EXISTS`; a missing bookmark is valid for the first create.
- `UpdateSchemaRequest`: adds `force` (skip the compatibility gate on a protected bookmark; requires Maintainer+). The schema must exist or the RPC returns `NOT_FOUND`; whole-source replacement runs the same compatibility gate as granular mutations.
- `DeleteSchemaRequest`: `force` skips protected-bookmark compatibility and requires Maintainer+. It never bypasses reference integrity: remaining same-repository live unpinned imports return `FAILED_PRECONDITION`; immutable pins remain valid.
- Create, Update, and Delete responses all return `{ new_commit, change_id, conflicted_decls }`.
- A true force override is retained as `schemahub.force=true` on the durable JJ operation.

### 7.2 `ApplyMutation` — one granular edit

`ApplyMutationRequest { project, repo, branch, base_revision, idempotency_key, force, oneof { protobuf_op | fbs_op | openapi_op } }` → `{ new_commit, change_id, conflicted_decls }`. The op is decoded to the internal `Mutation` and run through `Core::apply_mutation` (auth + compatibility gate, then a jj transaction). For Protobuf and FlatBuffers imports, the server resolves `to_tag` to a commit before mutation, validates explicit pins against the target repository/schema, and persists only the immutable commit ID.

### 7.3 `ApplyTransaction` — atomic batch

`ApplyTransactionRequest { …, repeated TransactionOp operations }` applies an ordered batch in **one** commit / one operation; compatibility + reference integrity are checked on the **final** state. `TransactionOp` carries `protobuf_op` | `fbs_op` | `openapi_op`. Empty `operations` → `INVALID_ARGUMENT`. All ops in a transaction must share one `format_id` (transactions never mix formats) and one `(project, repo)`; ops may target several schema files within that repo, in which case the core groups them by `schema_path`, applies each file's ops through the compiler, and commits every effect atomically via `Jj::commit_write_multi` (one commit, one operation across all touched files). Import pins are normalized before the batch reaches the compiler.

Limits are ≤100 operations and ≤20 schemas, matching
`AdminService.GetServerConfig`. The server starts the advertised 30-second
monotonic deadline before operation normalization, runs the synchronous Core
flow on the blocking executor, and returns `DEADLINE_EXCEEDED` when it expires.
The same cancellation token is checked throughout Core planning and inside the
final guarded publication callback. Expiry before publication removes a pending
idempotency receipt; a retry of work already inside atomic publication uses the
normal correlation/reconciliation contract. Clients may set a shorter deadline.

---

## 8. `exploration_service.proto` — Read API

```proto
service ExplorationService {
  rpc ListSchemas(...)       returns (...);   // schema files at a ref
  rpc ListDeclarations(...)  returns (...);   // per-declaration summaries in a file
  rpc GetDeclaration(...)    returns (...);   // one decl → summary + format-specific detail bytes
  rpc GetSchemaSource(...)   returns (...);   // AS-BUILT: source text for a schema file
  rpc FollowType(...)        returns (...);
  rpc ListDependencies(...)  returns (...);
  rpc ListDependents(...)    returns (...);
  rpc Search(...)            returns (...);
}
```

Per-declaration storage makes repository-local reads direct object lookups.
Handler: `server/src/services/exploration.rs`. Each repository-local method
resolves its `VersionRef at` once to a repository-owned immutable commit and
returns that commit (directly or in the resolution result); omission selects
the configured default bookmark. `ListDependents` instead captures the
configured default bookmark of every visible repository.

- **`ListSchemas` / `ListDeclarations`** return `at_commit` beside results from
  that exact snapshot. Declaration summaries are generated from the same
  immutable tree, not by re-resolving a mutable bookmark per declaration.
- **`GetDeclaration`** returns `DeclSummary summary` + `bytes detail` (the compiler's `DeclDetail` rendering) + `at_commit`. `declaration_name` is the per-declaration key (`"UserRequest"`, `"path:/users"`, `"schema:User"`, …).
- **`GetSchemaSource`** (`get_schema_source`) returns the schema file's source as bytes + `at_commit`. **AS-BUILT** — this RPC exists in the implemented proto and maps to `Core::get_schema_source`; it was not in the v1 doc's exploration service.
- **`Search`** is repository-scoped and fail-closed. It honors `at`, returns
  `at_commit`, and rejects an unknown schema format instead of silently
  omitting that file. Cross-repository declaration search is not part of this
  RPC.
- **`FollowType`** asks the selected compiler for the exact type of the named
  field/property, resolves the corresponding local or imported declaration,
  and returns its populated `summary`/`detail`. The response reports
  `source_commit`, target `resolved_commit`, `pinned`, and `import_path`.
  Same-repository live references stay on the captured source commit;
  cross-repository live references use one configured-default snapshot; stored
  pins stay immutable. Scalars/non-reference fields return `INVALID_ARGUMENT`,
  missing declarations return `NOT_FOUND`, and ambiguous matches return
  `FAILED_PRECONDITION` rather than choosing the first import.
- **`ListDependencies`** returns normalized direct or transitive edges. Every
  edge includes importing project/repo/schema/commit, the exact stored
  `import_path`, normalized target coordinates, stored `resolved_commit`,
  effective `target_commit`, `pinned`, and `resolved`. Same-repository live
  edges remain on their importing snapshot; each cross-repository live target
  is resolved once per repository per call; immutable pins are ownership
  checked. An unreadable, archived, absent, or builtin external target remains
  an explicit `resolved=false` edge and is not traversed. Invalid pins,
  corruption, unknown formats, and traversal-bound exhaustion fail the call.
  Protobuf imports, FlatBuffers includes, and external OpenAPI component `$ref`
  values in the selected 1.0 AST form the compiler-reported forward graph.
  OpenAPI refs use logical SchemaHub paths and are live/unpinned; network URLs,
  arbitrary fragments, and standalone reference shapes that AST cannot retain
  are rejected rather than surfaced as misleading registry edges. Other
  component categories are outside the 1.0 dependency guarantee.
- **`ListDependents`** — **AS-BUILT:** accepts a logical
  `(project, repo, schema_path)` target and returns direct import edges from all
  repositories readable by the authenticated identity. Each edge reports the
  importing bookmark and exact commit, the stored import, and whether it is
  pinned. The response also carries a sorted immutable snapshot manifest and
  total schemas scanned. Every repository is internally consistent at one
  resolved commit, but there is deliberately no cross-repository atomic
  instant, transitive reverse traversal, automatic source rewrite, or global
  transaction. Unreadable and archived repositories are omitted without
  disclosure. The authoritative scan fails as a whole after 1,000 visible
  repositories or 10,000 schemas; see `dependency-discovery.md`.

---

## 9. `codegen_service.proto` — Descriptors and Preview

```proto
service CodegenService {
  rpc GetDescriptors(GetDescriptorsRequest) returns (GetDescriptorsResponse);
  rpc PreviewCodegen(PreviewCodegenRequest) returns (PreviewCodegenResponse);
}
```

- **`GetDescriptors`** reconstructs the native descriptor artifact from the AST closure: Protobuf → `FileDescriptorSet` bytes; FlatBuffers → reconstructed `.fbs`; OpenAPI → resolved YAML (multi-document stream for multi-file closures). Response: `{ descriptor_bytes, SchemaFormat format, at_commit }`. **AS-BUILT** — `format` is derived from the schema-file extension and `at_commit` reports the resolved commit when ref resolution succeeds.
- **`PreviewCodegen`** renders generated source for a `Language`. The closure retains the explicitly requested root: Protobuf resolves imported and nested named types across all files, while FlatBuffers derives root helpers only from that root file. Request includes `rust_pluggable_buffer` (FlatBuffers Rust only) to generate `FlatBufferRead`-based readers and `root_as_<name>_in(&buffer)` helpers. Response `{ bytes content, bool is_archive, at_commit }`; `is_archive` is currently always `false`. Unsupported language/format → `UNIMPLEMENTED` (e.g. OpenAPI `generate_code` returns `UnsupportedLanguage`).

---

## 10. `serving_service.proto` — Immutable Revisions and Artifacts

```proto
service ServingService {
  rpc ResolveRevision(ResolveRevisionRequest) returns (SchemaRevision);
  rpc GetSchemaArtifact(GetSchemaArtifactRequest) returns (SchemaArtifact);
}
```

- `ResolveRevision` accepts the repository parent plus a bookmark, tag, or
  commit and returns
  `projects/{project}/repos/{repo}/revisions/{commit}`. The commit must be
  reachable from that repository's current or historical JJ operation views;
  global content deduplication cannot be used to cross repository boundaries.
- `GetSchemaArtifact` accepts only an immutable revision resource and returns
  canonical `SOURCE`, native `DESCRIPTORS`, or `GENERATED_CODE`. Every response
  carries media type, format, dependency schema names, exact payload SHA-256,
  deterministic closure SHA-256, and archive status.
- Before its first successful response, `GetSchemaArtifact` atomically stores
  the exact payload and verified metadata under a versioned request identity.
  Concurrent renderers converge on the first writer; later calls return that
  record across restart or compiler upgrade, and corruption fails closed.
- `if_none_match` equal to the payload digest returns metadata with empty
  content and `not_modified=true`. The payload digest is also emitted as the
  `x-schemahub-artifact-digest` gRPC response metadata field.
- Authorization is checked for the named repository and for every schema in
  the transitive import closure, including dependencies loaded from a stored
  artifact. See `serving.md` for persistence, versioned digest encoding, and
  the HTTP cache contract.

---

## 11. `project_service.proto` — Projects, Repos, Members

```proto
service ProjectService {
  rpc CreateProject / GetProject / UpdateProject / ListProjects / DeleteProject
  rpc CreateRepo / GetRepo / UpdateRepo / ListRepos / DeleteRepo
  rpc AddMember / RemoveMember / UpdateMemberRole / ListMembers
  rpc ListControlPlaneAuditEvents
}
```

**AS-BUILT — the resource hierarchy is durable.** Projects, memberships, and
repositories use ObjectDb resource records in the selected redb/PostgreSQL
database. Resources carry ETags and timestamps; updates use field masks and
compare-and-swap; deletion archives metadata while retaining repositories and
JJ history. See `resources-and-policy.md`.

| RPC | Behavior |
|-----|----------|
| `CreateProject` | **REAL** — wired to `Core::create_project`. Anonymous identities are rejected (`PERMISSION_DENIED`); the resolved caller becomes the project's Owner. |
| `GetProject` | Reads active projects; `include_archived=true` enables an Owner-only audit read. Archived records are hidden by default. |
| `UpdateProject` | Owner-only field-mask update of `is_public`; requires the current project ETag. |
| `ListProjects` | Returns readable active projects in stable name order with prefix filtering and opaque pagination over bounded catalog ranges. `include_archived` adds only archived projects owned by the caller. An authorization-filtered page can be empty and still carry `next_page_token`; continue until the token is empty. |
| `DeleteProject` | Owner-only soft archive. Requires an ETag and refuses a project containing repository records unless `force=true`; retained descendants become runtime-inert. |
| `CreateRepo`, `GetRepo`, `UpdateRepo` | Durable authorized repository CRUD. Updates select policy fields through a field mask and require the current ETag. |
| `ListRepos` | Stable name-ordered pagination with prefix and archive filters over one bounded per-project catalog range. |
| `DeleteRepo` | Soft archive that retains JJ history; requires `force=true` when refs exist. |
| `AddMember`, `RemoveMember`, `UpdateMemberRole` | **REAL** — wired to `Core::add_member` / `remove_member` / `update_member_role`. Owner-only (`Action::ManageProject`). The "last Owner" invariant is enforced fail-fast — these calls refuse to leave a project with zero Owners. |
| `ListMembers` | Gated by project `Action::Read`; stable identity-ordered pagination uses a project-bound opaque token and one bounded primary-key range. Inactive tombstones can yield an empty page with `next_page_token`; continue until the token is empty. |
| `ListControlPlaneAuditEvents` | Owner-only, newest-first immutable administrative history below `parent=projects/{project}`, with a parent-bound opaque cursor and bounded ordered-index range read. Events contain a server-generated ID, server-derived actor/time, action, target resource name, and typed project/member/repository snapshots before and after the mutation; malformed cursors or corrupt index/event relationships fail closed. |

`RepoConfig` owns compatibility direction, protected branches, required review,
ChangeRecord-only publication, and per-artifact serving flags. `[repos.*]`
seeds missing records; `UpdateRepo` changes the persisted runtime policy. The
durable record wins over the startup fallback.

Project, member, and repository state changes append their event in the same
ObjectDb transaction as the resource create/CAS. A failed precondition or
backend error therefore writes neither state nor event. This administrative
history is distinct from `HistoryService.OpLog`, which remains repository/JJ
history and supports undo.

Project/repository creates and archive transitions also maintain active/all
name catalogs in the resource transaction. Pre-index resources are backfilled
once behind durable markers. Page-token encodings and catalog keys are internal;
clients return opaque tokens unchanged. Malformed/filter-reused tokens and
missing, corrupt, or scope-mismatched catalog targets fail closed.

Membership pages use the already ordered
`projects/{project}/members/{hex(identity)}` role primary keys, so they require
no derived-index migration. A page reads and validates at most
`page_size + 1` scoped records. Malformed/cross-project tokens, invalid scoped
records, and key/content mismatches fail closed; records from another project
are neither returned nor decoded.

---

## 12. `admin_service.proto` — Operational RPCs

```proto
service AdminService {
  rpc RunGC(RunGCRequest)                     returns (RunGCResponse);
  rpc RebuildIndex(RebuildIndexRequest)       returns (RebuildIndexResponse);
  rpc GetServerConfig(GetServerConfigRequest) returns (GetServerConfigResponse);
  rpc GetFormatCapabilities(GetFormatCapabilitiesRequest)
      returns (GetFormatCapabilitiesResponse);
}
```

> **AS-BUILT** (`server/src/services/admin.rs`):
> - **`RunGC`** requires non-empty `project` + `repo` (global GC is v2 → `INVALID_ARGUMENT` otherwise). `dry_run` is honored by skipping both sweeps. A non-dry run performs JJ GC and bounded receipt cleanup; `idempotency_entries_cleaned` reports expired/evicted receipts. `objects_scanned`/`objects_deleted` both currently equal JJ's swept count; `bytes_reclaimed` and `pending_entries_cleaned` remain `0`.
> - **`RebuildIndex`** calls `Core::rebuild_index`; the response counters are currently `0`.
> - **`GetServerConfig`** returns the live limits (see §7.3 and §8): `max_ops_per_transaction=100`, `max_schemas_per_transaction=20`, `transaction_timeout_secs=30`, `idempotency_ttl_hours=24`, `max_dependency_scan_repositories=1000`, `max_dependency_scan_schemas=10000`, the resolved `storage_backend` supplied by the composition root, and `server_version` from `CARGO_PKG_VERSION`; `pending_cleanup_threshold_secs` and `gc_age_threshold_hours` remain `0`.
> - **`GetFormatCapabilities`** returns matrix version `1.0`, format-level parse/print, compatibility, conflict, descriptor, and codegen flags, plus a typed status and direct/transaction reachability for every advertised mutation. FlatBuffers unsafe field removal/reordering are explicit `REJECTED` entries. `schemahub capabilities --json` exposes the same contract; see `format-capabilities.md`.

---

## 13. Authentication

Transport-level, not in the proto types. The server reads an `authorization` metadata header (`Bearer <token>`, lowercased key), strips the prefix, and passes the token to the `AuthnProvider`. There are no auth RPCs.

Three modes ship in-tree:

- **Noop (default).** When `schemahub.toml` has no static tokens, JWT block,
  or `[projects.*]` bootstrap, the server installs `NoopAuthn` + `NoopAuthz`:
  every request is `Identity::Anonymous`, every action allowed. Tokens are
  accepted but ignored. This is the getting-started default.
- **Static BearerToken + RBAC (development).** When `[auth].tokens` is
  non-empty, `BearerTokenAuthn` resolves the configured token table.
- **JWT + RBAC (production).** `[auth.jwt]` validates externally issued JWTs
  against a startup-loaded and rotating HTTPS/file JWKS. Issuer, audience,
  asymmetric algorithm, token type, `kid`, expiration, optional `nbf`/`iat`,
  input bounds, and cache freshness are enforced. Static tokens and JWT mode
  are mutually exclusive.

Both configured modes use `RoleBasedAuthz` over ObjectDb project/role records
in the selected redb/PostgreSQL database. `[projects.<name>]` blocks seed
missing projects and reconcile configured roles. Former JSON stores under
`[auth].data_dir` are imported atomically on first database-backed startup.
Four roles, descending: `Owner` / `Maintainer` / `Writer` / `Reader`.
`--force` requires `Maintainer`+; `ManageProject` is `Owner`-only. Archived
projects fail closed for normal operations, and the "last Owner" invariant is
enforced on member changes. See `design.md` §11, `authentication.md`, and
`resources-and-policy.md`.

Token identities may set `kind = "human" | "agent" | "service"` (default:
`human`). Agent tokens may also set `delegated_by`. These values are audit
metadata only and do not elevate the identity's project role.

JWT mode uses required `iss`, `aud`, `sub`, and `exp` claims. The durable
identity is `identity_id_prefix + sub`; optional `name`,
`schemahub_identity_kind`, and `schemahub_delegated_by` claims supply the same
audit metadata. Missing credentials remain anonymous for public reads, while a
presented invalid JWT returns `UNAUTHENTICATED` instead of degrading to
anonymous.

---

## 14. Error Handling

| gRPC Status | When |
|-------------|------|
| `OK` | Success |
| `NOT_FOUND` | project/repo/schema/branch/tag/commit/declaration absent |
| `INVALID_ARGUMENT` | missing/empty required field; unknown file extension; no compiler for format; empty transaction; scalar/non-reference `FollowType` field; mutation validator reject (`MutationValidationError`); resolved source lacks the declaration; repo-scope-required (Search/GC); non-branch conflict-render ref |
| `FAILED_PRECONDITION` | compatibility/policy violation; archived resource; non-forced archive with retained descendants/refs; ambiguous type reference; unreachable history stop; foreign/unknown repository commit; `RenderConflict` on a non-conflicted decl |
| `ABORTED` | stale ChangeRecord, project, or repository ETag during optimistic concurrency |
| `DEADLINE_EXCEEDED` | `ApplyTransaction` exceeded the independent 30-second server execution deadline |
| `RESOURCE_EXHAUSTED` | transaction, request-body, idempotency, forward/reverse dependency traversal, or commit-history scan bound exceeded |
| `PERMISSION_DENIED` / `UNAUTHENTICATED` | authz / authn failure (Noop default never raises these; configured RBAC denies missing roles, and JWT mode returns `UNAUTHENTICATED` for malformed, expired, stale-key, or unverifiable credentials) |
| `UNIMPLEMENTED` | Unsupported compiler mutation or `PreviewCodegen` language/format (for example OpenAPI code generation). |
| `INTERNAL` | server/JJ error |

Structured `status.details` (as `google.protobuf.Any`) carry `CompatibilityError` and `MutationValidationError` for programmatic inspection; the CLI unpacks and renders them.

---

## 15. CLI Surface (as implemented)

The CLI (`schemahub-cli`) is a pure gRPC client. Top-level commands (`main.rs`) and their actions:

```bash
schemahub repo init <project/repo> [--public] [--default-branch main]     # ProjectService (create project+repo)

schemahub project create <name> [--public]                                 # RBAC: caller becomes Owner
schemahub project member list <project> [--page-size 50] [--json]          # Read access; follows all pages
schemahub project member add <project> <identity_id> [--role Reader]       # Owner-only
schemahub project member remove <project> <identity_id>                    # Owner-only
schemahub project member set-role <project> <identity_id> --role <role>    # Owner-only

schemahub change note <project/repo> --title T [--description D] [--reference R]... [--id ID]
schemahub change get <projects/P/repos/R/changes/C>
schemahub change list <project/repo> [--status draft] [--page-size 50] [--page-token TOKEN]
schemahub change update <projects/P/repos/R/changes/C> --etag E [--title T] [--description D] [--reference R]... [--clear-references]
schemahub change abandon <projects/P/repos/R/changes/C> --etag E
# All change commands accept --json for stable machine-readable output.

schemahub artifact resolve <project/repo> [--at main] [--json]
schemahub artifact fetch <projects/P/repos/R/revisions/H> --schema-path S \
  [--kind source|descriptors|generated-code] [--language rust] [--output FILE] \
  [--if-none-match sha256:HEX] [--json]
schemahub artifact verify <projects/P/repos/R/revisions/H> --schema-path S \
  [--kind source|descriptors|generated-code] [--language rust] --digest sha256:HEX

schemahub schema create <file> --project P --repo R [--branch main] [--name N] [--base-revision ""]
schemahub schema update <file> --project P --repo R [--branch] [--name] [--base-revision] [--force]
schemahub schema pull   <project/repo/schema> [--branch main]              # prints reconstructed source
schemahub schema delete <project/repo/schema> [--branch] [--base-revision] [--force]

schemahub field add    <project/repo/schema> <message> <name:type:number> [--branch] [--base-revision]
schemahub field remove <project/repo/schema> <message> <field>            # auto-reserves number+name
schemahub field rename <project/repo/schema> <message> <old> <new>

schemahub branch create <project/repo> <name> [--from main]
schemahub branch delete <project/repo> <name>
schemahub branch list   <project/repo> [--prefix ""] [--page-size 50]
schemahub branch merge  <project/repo> <source> [--into main] [--base-revision] [--message]

schemahub tag create <project/repo> <name> (--commit <id> | --branch <name>) [--message]
schemahub tag delete <project/repo> <name> [--force]                       # --force required
schemahub tag list   <project/repo> [--prefix ""] [--page-size 50]

schemahub log  <project/repo> [--branch main] [--limit 20]                 # commit/change history
schemahub op log <project/repo> [--limit 0]                                # operation log (audit; 0 = no limit)
schemahub undo <project/repo> [--author schemahub-cli]
schemahub resolve <project/repo/schema> <declaration> [--branch main] [--from <file>] [--author] [--message]
                                                                           # --from omitted → render the conflict
schemahub diff <project/repo> <base..head> [--schema-path ""]

schemahub codegen get     <project/repo/schema> [--branch] [--lang] [--out ./gen]
schemahub codegen preview <project/repo/schema> [--branch] [--lang] [--rust-pluggable-buffer]
```

> **AS-BUILT — CLI scope.** Change notes have note/get/list/update/abandon commands and stable `--json` output. Granular mutations are exposed only for Protobuf **fields** (`field add/remove/rename`); there is no `message` / `enum` / `service` subcommand yet (those `ApplyMutation` ops exist on the wire but have no CLI). Top-level `diff` lives at the root (`schemahub diff …`), not under `branch`; `merge` lives under `branch merge`. `op log` is the only `op` subcommand. `project` ships create/get/list/set-visibility/archive plus `member {list,add,remove,set-role}`; lifecycle mutations require ETags. Config: server/token via `--server`/`--token` flags, `SCHEMAHUB_SERVER`/`SCHEMAHUB_TOKEN` env (clap `env` feature), or `~/.schemahub` profile (`--profile`). The server coordinate is required and malformed/unreadable config fails closed. CLI ref strings parse as `tag:<name>` → tag, `@<hex>` → commit, else branch. `codegen preview --rust-pluggable-buffer` is honored only for FlatBuffers Rust output.

---

## 16. Design Decisions (retained / revised)

- **One service per concern.** Nine services keep each `.proto` focused;
  `ChangeService` isolates intent/review workflow, `ServingService` isolates
  immutable consumption, and `HistoryService` isolates the jj-specific surface.
- **`change_id` on every write.** Durable identity replaces v1's CAS-as-identity. A client answers "did my edit land, and where is it now" via the change ID + op log even after the bookmark advanced or history was rewritten.
- **Conflicts are data unless protected policy forbids publication.** Unprotected
  bookmarks retain concurrent sides for resolution. Protected bookmarks fail
  closed on the exact final tree without turning `base_revision` into a HEAD
  compare-and-swap gate.
- **Branches == bookmarks.** `RefService` keeps git-flavored branch/tag/merge
  naming as a thin alias over jj bookmarks, easing migration. Create, move,
  list, and delete are implemented; every mutation is serialized through the
  repository publication guard.
- **`VersionRef` oneof.** Forces the caller to declare branch vs. tag vs. commit intent; read paths currently lean on the bookmark form.
- **Format always explicit on create.** No content sniffing; the CLI infers from extension and sets `SchemaFormat`.
