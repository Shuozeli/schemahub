# schemahub — gRPC API (v2: Compilers + Jujutsu-style JJ)

> This document specifies the gRPC API surface **as implemented**: all services, RPCs, request/response shapes, and mutation types. The `.proto` files described here are the external contract that the CLI, AI agents, and any future client libraries depend on. The server (`schemahub-server`) translates these wire types into the internal `Mutation` envelope and `Core` calls described in `design.md`.
>
> It supersedes the v1 git-style API design (preserved in git history). Two project-level decisions drive the v2 surface (see `design.md`, `requirements.md`):
>
> 1. **Jujutsu-style JJ.** Writes no longer reject on a base-revision CAS mismatch. Every write returns a stable **`change_id`** (durable identity that survives rewrite/rebase) alongside the new commit, and may report **`conflicted_decls`** — declarations that landed as first-class conflicts rather than hard-failing. A new **`HistoryService`** exposes the operation log, `Undo`, and conflict render/resolve.
> 2. **Compilers, not plugins.** "branches" map to jj **bookmarks** (the server keeps the `RefService`/branch names as an alias over bookmarks); per-declaration storage; the format-specific work is owned by a `Compiler` (`design.md` §2).
>
> **Source-of-truth note.** Where this doc and `design.md` disagree, the protos + server win — they are the implementation. Divergences are flagged inline with **AS-BUILT**.

---

## 1. File Structure and Package Conventions

The `.proto` files live in `crates/schemahub-api/proto/schemahub/v1/`. The v2 layout split the old monolithic `common.proto` into focused files and added `history_service.proto`:

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
  exploration_service.proto# ExplorationService (read API)
  codegen_service.proto    # CodegenService    (descriptors, preview)
  project_service.proto    # ProjectService    (projects, repos, members/ACL)
  admin_service.proto      # AdminService      (GC, index rebuild, server config)
```

**Package:** `schemahub.v1` · **Go option:** `option go_package = "github.com/shuozeli/schemahub/gen/go/schemahub/v1";` · **Rust:** generated via `tonic-build`; all types land in `schemahub_api::schemahub_v1`. All files are `syntax = "proto3"`.

> **AS-BUILT — `refs.proto` vs `resources.proto`:** the design assumed a single `common.proto`. The implementation split shared types: `VersionRef` lives in `refs.proto`; `CommitInfo`/`DeclSummary`/`SchemaInfo`/`BranchInfo`/`TagInfo` in `resources.proto`; the structured error details in `errors.proto`. `ref_service.proto` defines the version-control RPCs (the file `refs.proto` holds only `VersionRef`).

---

## 2. The Jujutsu Concurrency Model on the Wire

This is the single biggest change from v1 and shapes every write RPC.

### 2.1 No CAS rejection — concurrency yields conflicts

In v1 a write carried a `base_revision` and the server **rejected** with `FAILED_PRECONDITION` + `ConflictDetail` if it did not match the branch HEAD. In v2 the JJ layer is jj-style:

- Two writes starting from the same state **both commit**. jj records concurrent operations and merges their views on next load.
- Edits to **different** declarations merge automatically (different per-declaration files in the tree).
- Edits to the **same** declaration produce a **first-class conflict** on that one declaration — the second writer is *not* rejected; the conflict is recorded for later resolution.

`base_revision` is therefore advisory in v2 (kept for wire compatibility and as an optimistic hint); the durable identity of a write is the returned **`change_id`**, and the durable record of concurrency is the **operation log** plus the **`conflicted_decls`** field.

> **AS-BUILT — `ConflictDetail` / `MergeConflictDetail` are vestigial.** They remain defined in `errors.proto` for wire compatibility, but the implemented write path does not return `ConflictDetail` (no CAS reject). `Merge` is a real jj-style merge that surfaces same-decl divergence as entries in `conflicted_decls` on the response (see §5.4), so `MergeConflictDetail` is never raised either. Treat them as deprecated.

### 2.2 Common write-response fields

Every mutating RPC on `SchemaService` (and `ResolveConflict` on `HistoryService`) returns:

| Field | Type | Meaning |
|-------|------|---------|
| `new_commit` | `string` | hash of the commit created by this write |
| `change_id` | `string` | the stable jj **change ID** — durable identity of the edit across rewrite/rebase (`design.md` §5.1) |
| `conflicted_decls` | `repeated string` | declaration names that landed **conflicted** because of concurrency (empty on a clean write). Resolve with `HistoryService.ResolveConflict`. |

`DeleteSchemaResponse` carries `new_commit` + `change_id` only (a delete cannot itself produce a conflicted decl).

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

> **AS-BUILT** — `CommitInfo` is reconstructed from the operation-log-derived history (see §5.1); the server currently fills `hash`, `parent_hashes`, `author`, `message` and leaves `timestamp`/`force`/`format_id` empty when deriving from the log.

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

> **AS-BUILT — read-path resolution.** `Core`'s read/exploration/history/codegen paths resolve a `VersionRef` to a **bookmark name** (branch); unset defaults to `"main"`. Tag/commit refs are honored only where the bookmark namespace resolves them by name. Pinned-commit reads are not yet fully wired through exploration. (`server/src/services/exploration.rs`, `wire::version_ref_bookmark`.)

### 3.4 `errors.proto` (structured `status.details`)

`CompatibilityViolation` / `CompatibilityError` (compat failure → `FAILED_PRECONDITION`) and `MutationValidationError` (validator reject → `INVALID_ARGUMENT`) are the actively-used detail types. `ConflictDetail` and `MergeConflictDetail` are retained but vestigial under the jj model (§2.1).

---

## 4. `mutations.proto` — Mutation Operation Types

Granular, typed operations on one schema file. The compiler validates each against the real AST.

### 4.1 `ProtobufMutation` (oneof `operation`)

Field ops: `ProtoAddField`, `ProtoRemoveField` (auto-reserves number+name), `ProtoRenameField`, `ProtoChangeFieldType` (wire-type-compatible allowlist), `ProtoChangeFieldLabel`, `ProtoReorderFields`. Message ops: `ProtoAddMessage`, `ProtoRemoveMessage`, `ProtoRenameMessage`. Enum ops: `ProtoAddEnum`, `ProtoRemoveEnum`, `ProtoAddEnumValue`, `ProtoRemoveEnumValue`, `ProtoRenameEnumValue`. Service ops: `ProtoAddService`, `ProtoRemoveService`, `ProtoAddRpc`, `ProtoRemoveRpc`, `ProtoRenameRpc`. Import: `ProtoUpdateImport`. (Shapes unchanged from v1; see the proto for fields.)

### 4.2 `FlatBuffersMutation` (oneof `operation`)

Field ops: `FbsAddField` (always appended), `FbsDeprecateField`, `FbsRenameField` (rename is safe — wire identity is the slot index). `RemoveField`/`ReorderFields` are intentionally absent (always rejected). Table ops: `FbsAddTable`, `FbsRemoveTable`, `FbsRenameTable`. Enum: `FbsAddEnum`, `FbsAddEnumValue`. Union: `FbsAddUnion`, **`FbsAddUnionMember`**, **`FbsRemoveUnionMember`**. Import: `FbsUpdateImport`.

> **AS-BUILT** — `FbsAddUnionMember` / `FbsRemoveUnionMember` are present in the implemented proto (not in the v1 design's union set).

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

> **AS-BUILT** — the design (`design.md` §3.3, `openapi-ast.md`) says OpenAPI is whole-document-only in v1 with granular ops deferred. The implementation already ships these six granular ops (`compiler-openapi/src/operations.rs`, `lib.rs::apply_one`); any other granular op returns `MutationError::UnsupportedInV1`. They are reachable via `ApplyMutation` (`openapi_op`) — **but not via `ApplyTransaction`**, whose `TransactionOp` oneof carries only `protobuf_op` / `fbs_op`.

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

### 5.1 Commits & Diff (AS-BUILT — derived from the op log)

- **`ListCommits`** streams `CommitInfo` newest-first. **AS-BUILT:** the implementation derives the commit list from the operation-log-based `Core::log`, then reverses; `from`/`stop_at_commit`/`schema_path` filters in the request are accepted but not yet applied.
- **`GetCommit`** finds the matching entry in that derived log; `NOT_FOUND` if absent.
- **`Diff`** computes a per-declaration semantic diff between `base` and `head` refs. Returns `DeclarationChange { change_type: "added"|"removed"|"modified", decl_name, detail }`; `detail` carries format-specific bytes only for `"modified"` (empty for add/remove). When `schema_path` is empty, it diffs the union of schema files present on either side.

### 5.2 Branches (bookmarks)

- **`CreateBranch`** creates a bookmark from `from` (default `"main"`); returns `BranchInfo` (`protected` is always reported `false` — protection lives in repo config, §8).
- **`ListBranches`** / **`GetBranch`** enumerate bookmarks (optional `name_prefix`), reporting each bookmark's first target as `head_commit`.
- **`DeleteBranch`** removes the bookmark via `Core::delete_bookmark` (delegating to `Jj::delete_bookmark`).

### 5.3 Tags

- **`CreateTag`** points a tag at `target` (default `"main"`); `annotated` is set when `message` is non-empty.
- **`ListTags`** enumerates tags (optional `name_prefix`).
- **`DeleteTag`** requires `force=true` (else `FAILED_PRECONDITION` — tags are immutable pins by contract); when set, removes the tag via `Core::delete_tag`.

### 5.4 Merge

`Merge(source_branch → target_branch)` returns the commit `target_branch` points to afterward. **AS-BUILT:** a real jj merge via `Core::merge` → `Jj::merge` (`jj_lib::rewrite::merge_commit_trees`), producing a two-parent merge commit whose tree is jj's 3-way merge over the merge base. Same-declaration divergence becomes a stored jj first-class conflict, surfaced to the caller in `WriteResult::conflicted_decls` (the `MergeResponse` does not currently carry that field — callers can re-read the resulting commit to see them). `base_revision`/`idempotency_key`/`message` are accepted.

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

`LogRequest { project, repo, VersionRef at, uint32 limit }` → `LogResponse { repeated LogEntry }`, where:

```proto
message LogEntry { string commit_id=1; string change_id=2; repeated string parents=3;
                   string author=4; string message=5; string timestamp=6; }
```

Each entry exposes both the **`commit_id`** and the stable **`change_id`**. The walk is over the real commit/change graph (`Jj::commit_log`) newest→oldest from `at` (defaults to the repo's configured default bookmark when unset). `limit = 0` means "use Core's default", currently 100.

### 6.2 `OpLog` — the audit record

`OpLogRequest { project, repo, uint32 limit }` → `OpLogResponse { repeated OperationRecord }`:

```proto
message OperationRecord { string op_id=1; repeated string parents=2;
                          string description=3; string author=4; string timestamp=5; }
```

Every schemahub write (mutation, transaction, bookmark move, tag, undo, …) is one operation. The JJ returns the chain oldest→newest along the head's parent chain; `limit = 0` returns the full log, `limit = n` trims the front (oldest entries) and keeps the most recent `n` operations.

### 6.3 `Undo`

`UndoRequest { project, repo, author }` → `UndoResponse { undone_op_id }`. Restores the repo's view to one step further back through the content-op chain. **AS-BUILT:** undo is a **linear monotonic walk-back stack**, not jj's bare op-toggle — consecutive `undo` calls step further back (skipping leading `undo` ops at the head) rather than redoing the previous undo. Undo is itself an append-only operation. `author` defaults to `"schemahub"` when empty. `undone_op_id` is the id of the content operation whose effect was rolled past.

### 6.4 `RenderConflict` — inspect competing sides

`RenderConflictRequest { project, repo, schema_path, declaration_name, VersionRef at }` → `RenderConflictResponse { rendered }`. `rendered` is a human/agent-readable view of the conflicting sides (e.g. `base` / `side 0` / `side 1` fragments via `Compiler::render_conflict`). Returns `FAILED_PRECONDITION` if the declaration is not conflicted. `at` is resolved to a bookmark for the read (default `"main"`).

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

**AS-BUILT:** the client submits the **full source** of the schema file; the server parses it with the compiler selected by file extension, extracts the named declaration's blob, validates it (`Compiler::validate_resolution`), and commits the resolution as one operation. `INVALID_ARGUMENT` if the resolved source does not define `declaration_name`.

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

All requests carry `project`, `repo`, `branch` (the bookmark), plus `base_revision` (advisory, §2.1) and `idempotency_key` (RPC-edge dedupe). Handler: `server/src/services/schema.rs`.

### 7.1 Lifecycle (`CreateSchema` / `UpdateSchema` / `DeleteSchema`)

All three are **format-agnostic, whole-document** and share one mechanism: the server picks the compiler from the schema-file extension, `parse`s `source` into per-declaration objects, diffs against the current content of that file, and commits a `MutationEffect` (upsert every new decl + meta, remove dropped decls). `DeleteSchema` empties the file's subtree.

- `CreateSchemaRequest`: `project, repo, branch, schema_name, SchemaFormat format, source, base_revision, idempotency_key`.
- `UpdateSchemaRequest`: adds `force` (skip the compatibility gate on a protected bookmark; requires Maintainer+).
- `DeleteSchemaRequest`: adds `force` (delete even if dependents exist).
- Responses: `Create`/`Update` → `{ new_commit, change_id, conflicted_decls }`; `Delete` → `{ new_commit, change_id }`.

> **AS-BUILT** — the create path tolerates a missing base bookmark (first write may create the bookmark): it loads the base schema with `unwrap_or_default()`. There is no separate "branch already exists" pre-check on create.

### 7.2 `ApplyMutation` — one granular edit

`ApplyMutationRequest { project, repo, branch, base_revision, idempotency_key, force, oneof { protobuf_op | fbs_op | openapi_op } }` → `{ new_commit, change_id, conflicted_decls }`. The op is decoded to the internal `Mutation` and run through `Core::apply_mutation` (auth + compatibility gate, then a jj transaction).

### 7.3 `ApplyTransaction` — atomic batch

`ApplyTransactionRequest { …, repeated TransactionOp operations }` applies an ordered batch in **one** commit / one operation; compatibility + reference integrity are checked on the **final** state. `TransactionOp` carries only `protobuf_op` | `fbs_op` (OpenAPI granular ops are not transactionable — §4.3). Empty `operations` → `INVALID_ARGUMENT`. All ops in a transaction must share one `format_id` (transactions never mix formats) and one `(project, repo)`; ops may target several schema files within that repo, in which case the core groups them by `schema_path`, applies each file's ops through the compiler, and commits every effect atomically via `Jj::commit_write_multi` (one commit, one operation across all touched files).

> **AS-BUILT — limits.** The proto comment says ≤500 ops / ≤20 schemas / 30 s. `AdminService.GetServerConfig` actually reports `max_ops_per_transaction = 100`, `max_schemas_per_transaction = 20`, `transaction_timeout_secs = 30`. The schema count now matches the proto's claim; the op count is still half. Treat the served config as authoritative.

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
  rpc Search(...)            returns (...);
}
```

Per-declaration storage makes each read a direct object lookup. Handler: `server/src/services/exploration.rs`. All take a `VersionRef at` resolved to a bookmark (default `"main"`).

- **`GetDeclaration`** returns `DeclSummary summary` + `bytes detail` (the compiler's `DeclDetail` rendering) + `at_commit`. `declaration_name` is the per-declaration key (`"UserRequest"`, `"path:/users"`, `"schema:User"`, …).
- **`GetSchemaSource`** (`get_schema_source`) returns the schema file's source as bytes + `at_commit`. **AS-BUILT** — this RPC exists in the implemented proto and maps to `Core::get_schema_source`; it was not in the v1 doc's exploration service.
- **`Search`** — **AS-BUILT:** repo-scoped. The server requires non-empty `project` and `repo`; cross-repo search returns `INVALID_ARGUMENT` ("cross-repo search is v2"). `SearchRequest.at` is honored (defaults to the `"main"` bookmark when unset) — `at` was previously hard-coded; the request field now flows through to `Core::search_detailed`.
- **`FollowType`** — **AS-BUILT, partial:** resolves a declaration's type refs against file imports and reports the first matching import's `resolved_schema_path` + `resolved_commit`, else echoes the request schema; `summary`/`detail` are currently left empty.
- **`ListDependencies`** — **AS-BUILT:** for OpenAPI, document-level imports are empty (external `$ref` imports are v2-modeled), so dependency lists come from the compiler's `imports(meta)`.

---

## 9. `codegen_service.proto` — Descriptors and Preview

```proto
service CodegenService {
  rpc GetDescriptors(GetDescriptorsRequest) returns (GetDescriptorsResponse);
  rpc PreviewCodegen(PreviewCodegenRequest) returns (PreviewCodegenResponse);
}
```

- **`GetDescriptors`** reconstructs the native descriptor artifact from the AST closure: Protobuf → `FileDescriptorSet` bytes; FlatBuffers → reconstructed `.fbs`; OpenAPI → resolved YAML (multi-document stream for multi-file closures). Response: `{ descriptor_bytes, SchemaFormat format, at_commit }`. **AS-BUILT** — `format` is derived from the schema-file extension; `at_commit` is currently left empty.
- **`PreviewCodegen`** renders generated source for a `Language`. Response `{ bytes content, bool is_archive, at_commit }`; `is_archive` is currently always `false`. Unsupported language/format → `UNIMPLEMENTED` (e.g. OpenAPI `generate_code` returns `UnsupportedLanguage`).

---

## 10. `project_service.proto` — Projects, Repos, Members

```proto
service ProjectService {
  rpc CreateProject / GetProject / ListProjects / DeleteProject
  rpc CreateRepo / GetRepo / UpdateRepo / ListRepos / DeleteRepo
  rpc AddMember / RemoveMember / UpdateMemberRole / ListMembers
}
```

**AS-BUILT — projects + member management are real; repo registry is still implicit.** Projects, members, and visibility are persisted (`schemahub-core/src/projects.rs` over `ProjectStore` + `RoleStore`; default file-backed JSON under `[auth].data_dir`). Repos still spring into existence on first write — a persisted repo registry has not landed yet.

| RPC | Behavior |
|-----|----------|
| `CreateProject` | **REAL** — wired to `Core::create_project`. Anonymous identities are rejected (`PERMISSION_DENIED`); the resolved caller becomes the project's Owner. |
| `GetProject` | **REAL** — `Core::get_project`; `NOT_FOUND` if absent; `PERMISSION_DENIED` if the caller can't `Read` it. |
| `ListProjects` | **REAL** — `Core::list_projects`; returns every project the caller can `Read` (public ∪ private-where-member), filtered by `name_prefix`. |
| `DeleteProject` | **UNIMPLEMENTED** — "not exposed by the JJ layer in v1". |
| `CreateRepo`, `GetRepo`, `UpdateRepo` | echo back a `RepoConfig` with defaults (`default_branch="main"`, `protected_branches=["main"]`, direction `FULL`). No persisted repo registry yet. |
| `ListRepos` | returns an empty list (no registry). |
| `DeleteRepo` | **UNIMPLEMENTED** — "not exposed by the JJ layer in v1". |
| `AddMember`, `RemoveMember`, `UpdateMemberRole` | **REAL** — wired to `Core::add_member` / `remove_member` / `update_member_role`. Owner-only (`Action::ManageProject`). The "last Owner" invariant is enforced fail-fast — these calls refuse to leave a project with zero Owners. |
| `ListMembers` | **REAL** — `Core::list_members`; gated by `Action::Read`. |

`RepoConfig` (`compatibility_direction`, `protected_branches`) is the home of the compatibility-protection policy (`design.md` §7) and IS honored at mutation time through `Config.repo_config_store()` + `config::RepoConfigStore` — but the values come from the `[repos.*]` section of `schemahub.toml`, not from `UpdateRepo` (which still only echoes).

---

## 11. `admin_service.proto` — Operational RPCs

```proto
service AdminService {
  rpc RunGC(RunGCRequest)                     returns (RunGCResponse);
  rpc RebuildIndex(RebuildIndexRequest)       returns (RebuildIndexResponse);
  rpc GetServerConfig(GetServerConfigRequest) returns (GetServerConfigResponse);
}
```

> **AS-BUILT** (`server/src/services/admin.rs`):
> - **`RunGC`** requires non-empty `project` + `repo` (global GC is v2 → `INVALID_ARGUMENT` otherwise). `dry_run` is honored by skipping the sweep. `RunGCResponse` reports `objects_scanned`/`objects_deleted` (both = swept count); `bytes_reclaimed` and the v1 `pending_*`/`idempotency_*` counters are `0` (those v1 GC roots no longer exist under the op-log model).
> - **`RebuildIndex`** calls `Core::rebuild_index`; the response counters are currently `0`.
> - **`GetServerConfig`** returns the live limits (see §7.3): `max_ops_per_transaction=100`, `max_schemas_per_transaction=20`, `transaction_timeout_secs=30`, `storage_backend="redb"` (hard-coded; the field doesn't yet reflect a `"postgres"` build), `server_version` from `CARGO_PKG_VERSION`; the v1 `pending_*`/`idempotency_*`/`gc_age_*` fields report `0`.

---

## 12. Authentication

Transport-level, not in the proto types. The server reads an `authorization` metadata header (`Bearer <token>`, lowercased key), strips the prefix, and passes the token to the `AuthnProvider`. There are no auth RPCs.

Two modes ship in-tree (`schemahub-server/src/lib.rs::build_core`):

- **Noop (default).** When `schemahub.toml` has no `[auth].tokens` (and no `[projects.*]` bootstrap), the server installs `NoopAuthn` + `NoopAuthz`: every request is `Identity::Anonymous`, every action allowed. Tokens are accepted but ignored. This is the getting-started default.
- **BearerToken + RBAC (configured).** When `[auth].tokens` is non-empty, the server installs `BearerTokenAuthn` (a static `token → Identity` table) + `RoleBasedAuthz` (project-scoped roles), both backed by `FileRoleStore` + `FileProjectStore` under `[auth].data_dir`. `[projects.<name>]` blocks seed the project + role registries at startup (idempotent — entries already in the on-disk stores are not overwritten). Four roles, descending: `Owner` / `Maintainer` / `Writer` / `Reader`. `--force` requires `Maintainer`+; `ManageProject` is `Owner`-only. The "last Owner" invariant is enforced fail-fast on member removal/role-change. See `design.md` §11 and `crates/schemahub-server/src/config.rs` for the toml shape.

---

## 13. Error Handling

| gRPC Status | When |
|-------------|------|
| `OK` | Success |
| `NOT_FOUND` | project/repo/schema/branch/tag/commit/declaration absent |
| `INVALID_ARGUMENT` | missing/empty required field; unknown file extension; no compiler for format; empty transaction; mutation validator reject (`MutationValidationError`); resolved source lacks the declaration; repo-scope-required (Search/GC) |
| `FAILED_PRECONDITION` | compatibility violation (`CompatibilityError`); `RenderConflict` on a non-conflicted decl |
| `PERMISSION_DENIED` / `UNAUTHENTICATED` | authz / authn failure (Noop default never raises these; BearerToken + RBAC mode raises them on missing/unknown token or insufficient role) |
| `UNIMPLEMENTED` | `DeleteProject`, `DeleteRepo`; unsupported `PreviewCodegen` language/format (e.g. OpenAPI). `DeleteBranch`, `DeleteTag`, `AddMember`/`RemoveMember`/`UpdateMemberRole` are all implemented now. |
| `INTERNAL` | server/JJ error |

Structured `status.details` (as `google.protobuf.Any`) carry `CompatibilityError` and `MutationValidationError` for programmatic inspection; the CLI unpacks and renders them.

---

## 14. CLI Surface (as implemented)

The CLI (`schemahub-cli`) is a pure gRPC client. Top-level commands (`main.rs`) and their actions:

```bash
schemahub repo init <project/repo> [--public] [--default-branch main]     # ProjectService (create project+repo)

schemahub project create <name> [--public]                                 # RBAC: caller becomes Owner
schemahub project member add <project> <identity_id> [--role Reader]       # Owner-only
schemahub project member remove <project> <identity_id>                    # Owner-only
schemahub project member set-role <project> <identity_id> --role <role>    # Owner-only

schemahub schema create <file> --project P --repo R [--branch main] [--name N] [--base-revision ""]
schemahub schema update <file> --project P --repo R [--branch] [--name] [--base-revision] [--force]
schemahub schema pull   <project/repo/schema> [--branch main]              # prints reconstructed source
schemahub schema delete <project/repo/schema> [--branch] [--base-revision] [--force]

schemahub field add    <project/repo/schema> <message> <name:type:number> [--branch] [--base-revision]
schemahub field remove <project/repo/schema> <message> <field>            # auto-reserves number+name
schemahub field rename <project/repo/schema> <message> <old> <new>

schemahub branch create <project/repo> <name> [--from main]
schemahub branch delete <project/repo> <name>
schemahub branch list   <project/repo> [--prefix ""]
schemahub branch merge  <project/repo> <source> [--into main] [--base-revision] [--message]

schemahub tag create <project/repo> <name> (--commit <id> | --branch <name>) [--message]
schemahub tag delete <project/repo> <name> [--force]                       # --force required
schemahub tag list   <project/repo> [--prefix ""]

schemahub log  <project/repo> [--branch main] [--limit 20]                 # commit/change history
schemahub op log <project/repo> [--limit 0]                                # operation log (audit; 0 = no limit)
schemahub undo <project/repo> [--author schemahub-cli]
schemahub resolve <project/repo/schema> <declaration> [--branch main] [--from <file>] [--author] [--message]
                                                                           # --from omitted → render the conflict
schemahub diff <project/repo> <base..head> [--schema-path ""]

schemahub codegen get     <project/repo/schema> [--branch] [--lang] [--out ./gen]
schemahub codegen preview <project/repo/schema> [--branch] [--lang]
```

> **AS-BUILT — CLI scope.** Granular mutations are exposed only for Protobuf **fields** (`field add/remove/rename`); there is no `message` / `enum` / `service` subcommand yet (those `ApplyMutation` ops exist on the wire but have no CLI). Top-level `diff` lives at the root (`schemahub diff …`), not under `branch`; `merge` lives under `branch merge`. `op log` is the only `op` subcommand. `project` ships subcommands for `create` and `member {add,remove,set-role}` (wired to the RBAC layer). Config: server/token via `--server`/`--token` flags, `SCHEMAHUB_SERVER`/`SCHEMAHUB_TOKEN` env (clap `env` feature), or `~/.schemahub` profile (`--profile`). CLI ref strings parse as `tag:<name>` → tag, `@<hex>` → commit, else branch.

---

## 15. Design Decisions (retained / revised)

- **One service per concern.** Seven services keep each `.proto` focused; `HistoryService` isolates the jj-specific surface so non-history clients ignore it.
- **`change_id` on every write.** Durable identity replaces v1's CAS-as-identity. A client answers "did my edit land, and where is it now" via the change ID + op log even after the bookmark advanced or history was rewritten.
- **`conflicted_decls`, not rejection.** Concurrency is surfaced as data the caller can resolve, never a hard error to retry against a moving target — the core agents-and-humans-editing-concurrently goal.
- **Branches == bookmarks.** `RefService` keeps git-flavored branch/tag/merge naming as a thin alias over jj bookmarks, easing migration; deletion is deferred (UNIMPLEMENTED) until the JJ layer exposes it.
- **`VersionRef` oneof.** Forces the caller to declare branch vs. tag vs. commit intent; read paths currently lean on the bookmark form.
- **Format always explicit on create.** No content sniffing; the CLI infers from extension and sets `SchemaFormat`.
