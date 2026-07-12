# schemahub

A general-purpose schema registry server. Manage Protobuf, FlatBuffers, and OpenAPI schemas via gRPC and a CLI — with **jj-style version control** (stable change IDs, first-class conflicts, operation log + undo), granular mutations, compatibility enforcement, and code generation built in.

## Contents

- [Overview](#overview)
- [Workspace structure](#workspace-structure)
- [Getting started](#getting-started)
- [CLI reference](#cli-reference)
- [gRPC API](#grpc-api)
- [Schema formats](#schema-formats)
- [Granular mutations](#granular-mutations)
- [Version control model](#version-control-model)
- [Compatibility checking](#compatibility-checking)
- [Auth](#auth)
- [Storage](#storage)
- [Configuration](#configuration)
- [Web console](#web-console)
- [Limitations in v1](#limitations-in-v1)

---

## Overview

schemahub stores schemas as structured, parsed representations rather than raw files. Every change is a commit with a stable jj **change ID**. Branches (bookmarks) and tags work like git, but concurrent edits to the same declaration produce **first-class conflicts** rather than rejections, and every write is recorded in an **operation log** with `undo`. Protobuf and FlatBuffers schemas support field-level mutations (add/remove/rename fields, messages, enums, services); OpenAPI supports a handful of granular path/operation/component mutations plus whole-document push. Compatibility checks run automatically on protected branches.

**What it is not**: a file server for `.proto` files. Schemas live in the registry; your build pipeline pulls descriptors from it.

See `docs/design.md` for the architecture deep-dive, `docs/grpc-api.md` for the wire contract, `docs/crate-structure.md` for the workspace layout, `docs/openapi-ast.md` for the OpenAPI AST, `docs/ui-design.md` for the web-console product design, and `docs/gui.md` for the implemented React GUI architecture and usage.

---

## Workspace structure

Nine crates, organized as two layers (format-agnostic JJ + per-format compilers):

| Crate | Purpose |
|---|---|
| `schemahub-types` | Shared types, the `Compiler` trait, auth traits (`AuthnProvider`/`AuthzPolicy`), errors |
| `schemahub-jj` | jj-lib over a swappable `ObjectDb`: `DbBackend` + `DbOpStore`; redb default, in-memory + Postgres impls |
| `schemahub-core` | Orchestration: mutations, transactions, compatibility, conflicts, history, GC, RBAC (real `BearerTokenAuthn` + `RoleBasedAuthz`) |
| `schemahub-api` | tonic/prost-generated gRPC bindings for the protos in `crates/schemahub-api/proto/schemahub/v1/` |
| `schemahub-compiler-protobuf` | Protobuf compiler — wraps `protobuf-rs` (parse/AST/codegen); owns the `.proto` printer + mutation validator + compat checker |
| `schemahub-compiler-flatbuffers` | FlatBuffers compiler — wraps `flatbuffers-rs`; owns the `.fbs` printer |
| `schemahub-compiler-openapi` | OpenAPI compiler — in-tree AST/parser/printer (`docs/openapi-ast.md`) |
| `schemahub-server` | gRPC server (binary `schemahub-server`) — SchemaService, RefService, HistoryService, ExplorationService, CodegenService, ProjectService, AdminService |
| `schemahub-cli` | `schemahub` CLI binary — pure gRPC client |

---

## Getting started

### Build

```bash
cargo build --release
```

Produces two binaries: `schemahub-server` and `schemahub` (the CLI). The Postgres-backed `ObjectDb` is feature-gated:

```bash
# Build with Postgres support
cargo build --release --features postgres -p schemahub-server
```

### Run the server

```bash
# Defaults: redb at ./schemahub.db, listening on 0.0.0.0:50051
schemahub-server

# Pin everything explicitly
schemahub-server --listen 0.0.0.0:50051 --db ./schemahub.db --config schemahub.toml

# Postgres backend (requires --features postgres)
schemahub-server --db-url postgres://user:pass@host:5432/dbname \
                 --config schemahub.toml
```

If the `TAILSCALE_IP` environment variable is set and `--listen` is not given, the server binds to that IP on the port from `[listen].addr` (user infra convention). Otherwise the default is `0.0.0.0:50051`.

### Configure the CLI

```toml
# ~/.schemahub/config
[default]
server = "http://[::1]:50051"
token  = ""

[prod]
server = "https://schemahub.example.com"
token  = "my-token"
```

Use `--profile prod` to select a non-default profile, or override per-invocation with `--server` / `--token` (or env vars `SCHEMAHUB_SERVER` / `SCHEMAHUB_TOKEN`).

### Quick example

```bash
# Create project + repo in one step
schemahub repo init mycompany/payments

# Push a schema
schemahub schema create --project mycompany --repo payments payments/v1/payment.proto

# Add a field via granular mutation
schemahub field add mycompany/payments/payments/v1/payment.proto CreatePaymentRequest \
    "currency:string:3" --branch main

# Pull (print) the schema back
schemahub schema pull mycompany/payments/payments/v1/payment.proto

# Download a FileDescriptorSet for codegen
schemahub codegen get mycompany/payments/payments/v1/payment.proto
```

---

## CLI reference

Global flags (apply to all subcommands):

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--server` | `SCHEMAHUB_SERVER` | `http://[::1]:50051` | Server address |
| `--token` | `SCHEMAHUB_TOKEN` | _(empty)_ | Auth token |
| `--profile` | — | `default` | Config profile |

### `repo` — project and repo initialization

```
schemahub repo init [--public] [--default-branch B] <project/repo>
```

Creates the project and repo in one idempotent step. If the project already exists, it is reused. Use this instead of calling `CreateProject` and `CreateRepo` separately.

### `project` — project + member management (RBAC)

```
schemahub project create [--public] <name>
schemahub project member add        <project> <identity_id> [--role Reader]
schemahub project member remove     <project> <identity_id>
schemahub project member set-role   <project> <identity_id> --role <role>
```

Roles: `Reader` / `Writer` / `Maintainer` / `Owner`. `CreateProject` requires an authenticated identity (anonymous callers are rejected); the caller becomes the project's Owner. Member CRUD is Owner-only and enforces the "last Owner" invariant — a project must always have at least one Owner.

### `schema` — schema lifecycle

```
schemahub schema create [--project P] [--repo R] [--branch B] [--name N] [--base-revision H] <file>
schemahub schema update [--project P] [--repo R] [--branch B] [--name N] [--base-revision H] [--force] <file>
schemahub schema pull   [--branch B] <project/repo/schema_name>
schemahub schema delete [--branch B] [--base-revision H] [--force] <project/repo/schema_name>
```

Format is auto-detected from the file extension: `.proto` → Protobuf, `.fbs` → FlatBuffers, `.yaml`/`.yml`/`.json` → OpenAPI.

`--force` on `update` bypasses the compatibility check (requires Maintainer role on the repo). `--force` on `delete` bypasses the dependency guard.

### `field` — field-level Protobuf mutations

```
schemahub field add    [--branch B] [--base-revision H] <project/repo/schema> <MessageName> <name:type:number>
schemahub field remove [--branch B] [--base-revision H] <project/repo/schema> <MessageName> <field_name>
schemahub field rename [--branch B] [--base-revision H] <project/repo/schema> <MessageName> <old_name> <new_name>
```

`add` format example: `"currency:string:3"`. Removing a field automatically reserves its field number and name to prevent accidental reuse.

### `branch` — branch management

```
schemahub branch create [--from B] [--project P] [--repo R] <name>
schemahub branch delete [--project P] [--repo R] <name>
schemahub branch list   [--project P] [--repo R] [--prefix PREFIX]
schemahub branch merge  [--into B] [--base-revision H] [--message MSG] [--project P] [--repo R] <source>
```

Merge is a real jj 3-way merge: the server creates a 2-parent merge commit whose tree is jj's auto-merge over the merge base. Same-declaration divergence becomes a stored first-class conflict (resolve with `schemahub resolve`), not an error.

### `tag` — tag management

```
schemahub tag create [--commit H | --branch B] [--message MSG] [--project P] [--repo R] <name>
schemahub tag delete --force [--project P] [--repo R] <name>
schemahub tag list   [--project P] [--repo R] [--prefix PREFIX]
```

Providing `--message` creates an annotated tag. Without it, the tag is lightweight.

### `log` — commit history

```
schemahub log [--branch B] [--limit N] <project/repo>
```

Default limit: 20 commits. Walks the real commit/change graph via `Jj::commit_log`, surfacing each commit's content-addressed `commit_id`, stable jj `change_id`, parents, author, and message.

### `op log` — operation log (jj-style audit record)

```
schemahub op log [--limit N] <project/repo>
```

`--limit 0` (the default) returns the full log. Each entry shows the operation id, author, timestamp, and description. Every schemahub write is one operation.

### `undo` — undo the last operation

```
schemahub undo [--author A] <project/repo>
```

Linear monotonic walk-back: consecutive `undo` calls step further back through content ops, rather than redoing the previous undo. Prints the id of the operation whose effect was rolled past.

### `resolve` — render / resolve a conflicted declaration

```
schemahub resolve <project/repo/schema> <declaration>
                  [--branch B] [--from <file>] [--author A] [--message M]
```

Omit `--from` to render the conflict's competing sides (the `base` and each `side`) for inspection. Pass `--from <file>` containing the resolved schema source to commit the resolution: the server parses the file, extracts the named declaration's blob, validates it, and records the resolution as one operation.

### `codegen` — descriptor generation

```
schemahub codegen get     [--branch B | --at @<sha> | --at tag:<name>] <project/repo/schema>
schemahub codegen preview [--branch B] [--lang rust] [--rust-pluggable-buffer] <project/repo/schema>
```

`get` downloads the schema descriptor (`FileDescriptorSet` for Protobuf, reconstructed `.fbs` bundle for FlatBuffers, resolved YAML for OpenAPI) and prints it to stdout, following transitive imports automatically. `preview` renders generated source for the chosen language — implemented for Protobuf and FlatBuffers; OpenAPI returns `UNIMPLEMENTED`. `--rust-pluggable-buffer` is FlatBuffers Rust only: it asks the sibling `flatc-rs-codegen` backend to generate `FlatBufferRead`-based readers and `root_as_<name>_in(&buffer)` helpers for custom byte-buffer providers.

Ref formats accepted by `--at` / `--branch`:
- `main` (branch name, default)
- `@<sha>` (pinned commit)
- `tag:<name>` (tag)

### `diff` — semantic diff

```
schemahub diff [--schema-path S] <project/repo> <base..head>
```

Range example: `main..feature/add-user`. Output lists added, removed, and changed declarations between the two refs.

---

## gRPC API

All services are defined in `crates/schemahub-api/proto/schemahub/v1/`. The default listen address is `0.0.0.0:50051` (overridden by `TAILSCALE_IP` env when `--listen` is omitted).

### SchemaService

| RPC | Description |
|---|---|
| `CreateSchema` | Create a new schema on a branch. Returns `ALREADY_EXISTS` if the name is taken. |
| `UpdateSchema` | Replace a schema's full source. Runs compatibility check on protected branches. |
| `DeleteSchema` | Delete a schema. Rejected if importers exist unless `force=true`. |
| `ApplyMutation` | Apply a single granular mutation to one declaration. |
| `ApplyTransaction` | Apply up to 500 mutations across up to 20 schemas atomically. |

### RefService

Branch names map to jj **bookmarks**; the branch RPCs are a compatibility-shaped face over them.

| RPC | Description |
|---|---|
| `GetCommit` | Fetch commit metadata by hash. |
| `ListCommits` | Stream commits in reverse chronological order. |
| `Diff` | Per-declaration semantic diff between two version refs. |
| `CreateBranch` / `DeleteBranch` / `ListBranches` / `GetBranch` | Branch CRUD. |
| `CreateTag` / `DeleteTag` / `ListTags` | Tag CRUD. `DeleteTag` requires `force=true`. |
| `Merge` | Real jj 3-way merge with first-class conflicts; produces a 2-parent merge commit. Same-declaration divergence is recorded as a stored conflict, not an error. |

### HistoryService

The wire surface for the jj operation log and first-class conflict resolution (new in v2).

| RPC | Description |
|---|---|
| `Log` | Commit/change history graph from a ref. Each entry carries both the content-addressed `commit_id` and the stable jj `change_id`. Honors `at` + `limit` (default 100). |
| `OpLog` | The operation log — every schemahub write (mutation, transaction, bookmark move, undo, …) is one operation. `limit = 0` returns the full log. |
| `Undo` | Linear monotonic walk-back stack: each call steps further back through content ops (not jj's bare op-toggle). Returns the id of the operation whose effect was rolled past. |
| `RenderConflict` | Render a conflicted declaration's competing sides for human/agent display. Returns `FAILED_PRECONDITION` if the decl is not conflicted. |
| `ResolveConflict` | Submit a resolved schema source; the server parses it, extracts the named declaration's blob, validates it, and commits the resolution as one operation. |

A `VersionRef` can be a branch name, a commit hex, or a tag name:

```protobuf
message VersionRef {
  oneof ref {
    string branch = 1;
    string commit = 2;
    string tag    = 3;
  }
}
```

### ExplorationService

| RPC | Description |
|---|---|
| `ListSchemas` | All schema names in a repo at a given ref. |
| `ListDeclarations` | Top-level declarations in a schema, with optional kind filter. |
| `GetDeclaration` | Full detail for a named declaration (fields, RPCs, enum values, etc.). |
| `GetSchemaSource` | Return the reconstructed source text of a schema at a given ref. |
| `Search` | Search declarations by name prefix across all schemas in a repo. Supports kind filter and a limit. |
| `FollowType` | Resolve a type name to its declaration, following imports. `declaration_name` anchors the search; `field_name` is the type name to look up. |
| `ListDependencies` | List schemas a given schema imports, with optional transitive closure. Returns `(importing_schema, import_path, resolved_commit)` tuples. |

### CodegenService

| RPC | Description |
|---|---|
| `GetDescriptors` | Return a reconstructed descriptor for a schema and all its transitive imports. Protobuf → `FileDescriptorSet`; FlatBuffers → reconstructed `.fbs` bundle; OpenAPI → resolved YAML. |
| `PreviewCodegen` | Render generated source code server-side for the requested language. Implemented for Protobuf and FlatBuffers; OpenAPI returns `UNIMPLEMENTED`. Response carries the rendered text (no files written). `rust_pluggable_buffer=true` enables FlatBuffers Rust pluggable-buffer readers. |

### ProjectService

Manages the project / repo / membership hierarchy. Projects and members are real (persisted via the `ProjectStore` + `RoleStore`); the repo registry is still implicit (a `(project, repo)` springs into existence on first write).

| RPC | Description |
|---|---|
| `CreateProject` / `GetProject` / `ListProjects` | Real — wired to the `ProjectStore`. `CreateProject` rejects anonymous identities; the caller becomes the project's Owner. `ListProjects` returns only projects the caller can `Read` (public ∪ private-where-member). |
| `DeleteProject` | `UNIMPLEMENTED`. |
| `CreateRepo` / `GetRepo` / `UpdateRepo` | Echo back a `RepoConfig` with defaults (`default_branch="main"`, `protected_branches=["main"]`, direction `FULL`). Per-repo compatibility config lives in `[repos.*]` in `schemahub.toml`. |
| `ListRepos` | Returns an empty list (no persisted repo registry yet). |
| `DeleteRepo` | `UNIMPLEMENTED`. |
| `AddMember` / `RemoveMember` / `UpdateMemberRole` / `ListMembers` | Real role-based membership. Owner-only. Enforces the "last Owner" invariant. |

### AdminService

| RPC | Description |
|---|---|
| `RunGC` | Garbage-collect unreferenced objects. Supports `dry_run` and project/repo scoping. |
| `RebuildIndex` | Rebuild the search and dependency indices from scratch. |
| `GetServerConfig` | Returns server limits and configuration. |

---

## Schema formats

| Format | Extensions | Granular mutations | Compatibility check | GetDescriptors output |
|---|---|---|---|---|
| Protobuf | `.proto` | Yes — full suite | Yes | `FileDescriptorSet` (binary proto) |
| FlatBuffers | `.fbs` | Yes — see restrictions | Yes | Reconstructed `.fbs` bundle |
| OpenAPI | `.yaml` `.yml` `.json` | Partial — 6 granular ops (`ApplyMutation` only, not transactions); plus whole-document push | Yes | Resolved YAML (multi-document for closures) |

---

## Granular mutations

Granular mutations are applied via `ApplyMutation` (single) or `ApplyTransaction` (batch). They target a specific schema path and declaration name. The mutation payload is format-specific.

### Protobuf

**Fields** (target: a message declaration)

| Mutation | Description |
|---|---|
| `AddField` | Add a field with name, type, number, repeated flag, doc comment. |
| `RemoveField` | Remove a field. Automatically reserves the field number and name. |
| `RenameField` | Rename a field. Wire identity (field number) is unchanged. |
| `ChangeFieldType` | Change the type. Must stay within the same wire type. |
| `ChangeFieldLabel` | Switch between `optional` and `repeated`. |
| `ReorderFields` | Reorder fields by name list. Field numbers are unchanged. |

**Messages** (schema-level — `declaration_name` is the new or old message name)

| Mutation | Description |
|---|---|
| `AddMessage` | Add an empty message. |
| `RemoveMessage` | Remove a message. |
| `RenameMessage` | Rename a message (updates same-file references). |

**Enums** (schema-level for Add/Remove; enum-level for value ops)

| Mutation | Description |
|---|---|
| `AddEnum` | Add an empty enum. |
| `RemoveEnum` | Remove an enum. |
| `AddEnumValue` | Add a value with number and doc comment. |
| `RemoveEnumValue` | Remove an enum value. |
| `RenameEnumValue` | Rename an enum value. |

**Services** (schema-level for Add/Remove; service-level for RPC ops)

| Mutation | Description |
|---|---|
| `AddService` | Add an empty service. |
| `RemoveService` | Remove a service. |
| `AddRpc` | Add an RPC with request/response types and streaming flags. |
| `RemoveRpc` | Remove an RPC. |
| `RenameRpc` | Rename an RPC. Rejects if the new name already exists. |

**Imports** (schema-level)

| Mutation | Description |
|---|---|
| `UpdateImport` | Register or update an import dependency. Stores the logical path (`project/repo/schema.proto`) and an optional pin (`to_commit` or `to_tag`). Used by `ListDependencies` to track the import graph. |

### FlatBuffers

**Fields** (target: a table declaration)

| Mutation | Description |
|---|---|
| `AddField` | Append a field with type, default value, doc comment. Always added at the end — slot order is frozen. |
| `DeprecateField` | Mark a field as deprecated. Use instead of `RemoveField` (not supported in FlatBuffers). |
| `RenameField` | Rename a field. Wire identity (slot index) is unchanged. |

`RemoveField` and `ReorderFields` are permanently rejected — slot indices are part of the FlatBuffers wire format.

**Tables**

| Mutation | Description |
|---|---|
| `AddTable` | Add an empty table. |
| `RemoveTable` | Remove a table. |
| `RenameTable` | Rename a table. |

**Enums / Unions**

| Mutation | Description |
|---|---|
| `AddEnum` | Add an enum with a base integer type (`int8`–`uint64`). |
| `AddEnumValue` | Add a value to an existing enum. |
| `AddUnion` | Add a union with a list of initial member table names. |
| `AddUnionMember` | Add a table to an existing union. Rejects duplicates. |
| `RemoveUnionMember` | Remove a table from an existing union. Rejects if the member is not present. |

**Imports**

| Mutation | Description |
|---|---|
| `UpdateImport` | Add or update an import path. `to_commit` / `to_tag` are stored as pinning hints. |

### OpenAPI

OpenAPI supports a focused set of granular operations plus a whole-document push (used internally by `UpdateSchema`). All ops are reachable via `ApplyMutation`; OpenAPI ops are **not** transactionable.

| Mutation | Description |
|---|---|
| `PushDocument` | Whole-document replacement. Used internally by `UpdateSchema`. |
| `AddPath` | Add a new empty `path:<pattern>` declaration. Fails if the path already exists. |
| `RemovePath` | Remove the `path:<pattern>` declaration. |
| `AddOperation` | Add one HTTP method (`get`/`post`/`put`/`delete`/`patch`/`head`/`options`/`trace`) to a path item. |
| `RemoveOperation` | Remove one HTTP method from a path item. |
| `AddComponentSchema` | Add a new `schema:<name>` declaration with a JSON Schema type. |
| `RemoveComponentSchema` | Remove the `schema:<name>` declaration. |

Any other granular OpenAPI op returns `UnsupportedInV1`. See `docs/openapi-ast.md` for the AST and the per-declaration key scheme (`path:`, `schema:`, `param:`, `response:`, `requestBody:`).

---

## Version control model

schemahub uses the **Jujutsu (jj) model** via `jj-lib` (default features off — no git interop), with all persistence delegated to the `ObjectDb`:

- **Commits** — Immutable, content-addressed (`CommitId` via blake2b). Each carries a stable **`ChangeId`** that survives rewrite/rebase/squash — the durable identity of an edit even after history is rewritten.
- **Trees** — Per-declaration storage: a schema file is a jj subtree `<schema-file>/`; each top-level declaration is a file entry `<schema-file>/<Decl>` holding the `DeclBlob`; `<schema-file>/__meta__` holds the file's `MetaBlob` (package, imports, syntax/edition).
- **Branches (bookmarks)** — Mutable named refs. Names support glob patterns for protection rules (e.g., `main`, `release/*`). The branch RPCs are a compatibility-shaped face over jj bookmarks.
- **Tags** — Immutable refs. Lightweight tags are just a ref; annotated tags carry a message, tagger, and timestamp.
- **First-class conflicts** — Concurrent edits to the **same** declaration produce a stored conflict (a multi-side tree entry), surfaced in `conflicted_decls` on the response, not a hard error. The caller resolves it later via `HistoryService.ResolveConflict`. Concurrent edits to **different** declarations merge automatically.
- **Merge** — Real 3-way merge via `jj_lib::rewrite::merge_commit_trees`, producing a 2-parent merge commit. Same-decl divergence becomes a stored conflict; the merge itself never fails for this reason.
- **Operation log + undo** — Every write (mutation, transaction, bookmark move, tag, GC, resolve) is one jj `Operation`. `Undo` is a linear monotonic walk-back stack — consecutive `undo` calls step further back through content ops, rather than redoing the previous undo.

All ref arguments (`--branch`, `--at`) accept:
- A branch name: `main`, `feature/xyz`
- A commit hash: `@abc123...`
- A tag: `tag:v1.2.0`

---

## Compatibility checking

Compatibility checks run automatically when a mutation targets a protected branch and `--force` is not set.

### Directions

| Direction | Rule |
|---|---|
| `BACKWARD` | New schema must be readable by old clients. Adding optional fields is safe; removing fields is not. |
| `FORWARD` | Old schema must be readable by new clients. Removing fields is safe; adding enum values is not. |
| `FULL` | Both backward and forward. The strictest mode. |
| `DISABLED` | No checks. Suitable for scratch / development repos. |

Set the direction per repo via `CreateRepo` or `UpdateRepo` (`compatibility_direction`).

### What is checked (Protobuf)

| Change | Backward | Forward | Full |
|---|---|---|---|
| Add optional field | OK | OK | OK |
| Remove field | OK | FAIL | FAIL |
| Change field number | FAIL | FAIL | FAIL |
| Type change crossing wire type | FAIL | FAIL | FAIL |
| Type narrowing (e.g., int64 → int32) | OK | FAIL | FAIL |
| Label change (repeated ↔ optional) | FAIL | FAIL | FAIL |
| Remove enum value | FAIL | OK | FAIL |
| Add enum value | OK | FAIL | FAIL |
| Remove RPC | FAIL | OK | FAIL |
| Add RPC | OK | FAIL | FAIL |

Violations are returned as structured `CompatibilityError` details in the gRPC status, listing each declaration and field that caused the conflict.

---

## Auth

Auth is pluggable via two traits in `schemahub-types`:

```rust
pub trait AuthnProvider: Send + Sync + 'static {
    fn identify(&self, token: Option<&str>) -> Result<Identity, AuthnError>;
}

pub trait AuthzPolicy: Send + Sync + 'static {
    fn check(&self, caller: &Identity, action: Action, resource: &ResourcePath)
        -> Result<(), AuthzError>;
}
```

**Actions**: `Read`, `Write`, `Force` (for `--force` mutations), `ManageProject`, `ManageRepo`.

**Resource paths**: scoped to project or project/repo.

The server picks one of two modes at startup (`schemahub-server/src/lib.rs::build_core`):

### Noop (default)

If `schemahub.toml` has no `[auth]` section (and no `[projects.*]` bootstrap), the server installs `NoopAuthn` (every request is `Identity::Anonymous`) and `NoopAuthz` (every action allowed). Tokens are accepted but ignored. This is the getting-started default.

**Project + member RPCs fail fast in Noop mode.** Because there is no role store or project store to write into, `schemahub project create`, `schemahub project member add|remove|set-role`, and the underlying `ProjectService.{CreateProject, AddMember, RemoveMember, UpdateMemberRole}` RPCs return a `FailedPrecondition` error pointing at `[auth]` rather than silently no-op'ing. To use those commands, add an `[auth]` section (see below) — even a one-token table is enough to switch on the real RBAC layer.

### BearerToken + RBAC (configured)

When `[auth].tokens` is non-empty, the real RBAC layer turns on automatically:

- **`BearerTokenAuthn`** — a static `Bearer <token> → Identity` table from `[auth].tokens`.
- **`RoleBasedAuthz`** — project-scoped role checks.
- **`FileRoleStore` + `FileProjectStore`** — JSON persistence at `[auth].data_dir/{roles.json, projects.json}`.

Four roles, descending: `Owner` / `Maintainer` / `Writer` / `Reader`. `--force` requires `Maintainer`+; `ManageProject` (member CRUD) is `Owner`-only. The server enforces a **last Owner** invariant: removing or downgrading the only Owner of a project fails fast.

`[projects.<name>]` blocks seed the project + role registries at startup, idempotently — entries already in the on-disk stores are not overwritten. See `docs/design.md` §11 for details.

---

## Storage

The JJ layer (`schemahub-jj`) implements jj-lib's `Backend` and `OpStore` traits over a small `ObjectDb` abstraction. Three backends ship:

| Backend | Build | Use case |
|---|---|---|
| `RedbObjectDb` | default | Embedded single-file MVCC store; ideal for self-hosted/dev. The whole database is one file on disk. |
| `PgObjectDb` | `--features postgres` on `schemahub-server` | Multi-instance server deployments; uses `sqlx 0.9` with `runtime-tokio + tls-rustls`. |
| `MemoryObjectDb` | (tests only) | In-memory, non-persistent. |

Pick the backend in `schemahub.toml`:

```toml
[storage]
backend = "redb"                       # or "postgres" (requires `--features postgres`)
path    = "/var/lib/schemahub/data.db" # honored for backend = "redb"
url     = "postgres://user:pass@host/db" # honored for backend = "postgres"
```

`--db` / `--db-url` on the server binary override `path` / `url` respectively.

Internally, every object is content-addressed via jj-lib's blake2b hashing — files (`DeclBlob`/`MetaBlob`), trees, commits, views, plus a per-`(project, repo)` operation log. Content dedups globally; the op-log and refs are scoped per repo. There are no v1-era `pending/` or `idempotency/` key namespaces — durability comes from the op-log and content addressing.

The `ObjectDb` trait is the only persistence seam: implementing it (plus the per-repo `set_ref`/`get_ref` ref table) is enough to add a new backend.

---

## Configuration

### Server flags

| Flag | Default | Description |
|---|---|---|
| `--listen` | `0.0.0.0:50051` (or `TAILSCALE_IP:50051` if `TAILSCALE_IP` env is set) | Address to bind |
| `--db` | `schemahub.db` | Path to the redb database file (honored when `storage.backend = "redb"`) |
| `--db-url` | _(none)_ | Postgres connection URL (honored when `storage.backend = "postgres"`; requires `--features postgres`) |
| `--config` | `schemahub.toml` | Path to the server config file (optional — defaults apply if missing) |

### Server config file

`schemahub.toml` (optional):

```toml
[storage]
backend = "redb"          # or "postgres"
path    = "schemahub.db"  # for redb
# url   = "postgres://..." # for postgres

[listen]
addr = "0.0.0.0:50051"     # overridden by TAILSCALE_IP env when --listen is omitted

[repos."acme/payments"]    # per-(project/repo) compatibility config
default_bookmark    = "main"
compatibility       = "full"           # backward | forward | full | disabled
protected_bookmarks = ["main", "release/*"]

[auth]                                  # presence flips Noop → BearerToken + RBAC
data_dir = "schemahub-data"
[auth.tokens."secret-token-alice"]
id      = "alice"
display = "Alice Example"

[projects.acme]                         # bootstrap project + roles at startup
visibility = "private"                  # or "public"
owners     = ["alice"]
members    = { bob = "Writer", carol = "Reader" }
```

### CLI config file

`~/.schemahub/config` (TOML):

```toml
[default]
server = "http://[::1]:50051"
token  = ""

[prod]
server = "https://schemahub.example.com"
token  = "Bearer eyJ..."
```

Resolution order (first wins): CLI flags → environment variables (`SCHEMAHUB_SERVER`, `SCHEMAHUB_TOKEN`) → config file profile → built-in defaults.

---

## Web console

An experimental React console lives in `apps/schemahub-gui`. It is a Vite + React + TypeScript + Mantine app with a typed `SchemaHubClient` boundary. It uses mock data by default and can read from the server's HTTP/JSON BFF when `VITE_SCHEMAHUB_API_BASE` is set.

Current screens include project listing, repo dashboard, schema detail, compare, history, admin config, and codegen preview.

Run it locally:

```bash
cd apps/schemahub-gui
pnpm install
pnpm run dev
```

Run the server with the HTTP BFF:

```bash
schemahub-server --listen 0.0.0.0:50051 --http-listen 0.0.0.0:8080
```

Run the GUI against that BFF:

```bash
cd apps/schemahub-gui
VITE_SCHEMAHUB_API_BASE=http://localhost:8080 pnpm run dev
```

Run it on Tailscale:

```bash
cd apps/schemahub-gui
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
VITE_SCHEMAHUB_API_BASE="http://$TAILSCALE_HOST:8080" pnpm run dev -- --force
```

Open `http://$TAILSCALE_HOST:5173/`.

See `docs/gui.md` for the GUI architecture, route map, Tailscale setup, troubleshooting, and the planned path from mock data to the live gRPC server. See `docs/ui-design.md` for the product and component design.

---

## Limitations in v1

| Feature | Status |
|---|---|
| OpenAPI granular mutations | Partial. The compiler implements `AddPath`, `RemovePath`, `AddOperation`, `RemoveOperation`, `AddComponentSchema`, `RemoveComponentSchema` plus the whole-document `PushDocument` (used by `UpdateSchema`). Other granular ops return `UnsupportedInV1`. OpenAPI ops are reachable via `ApplyMutation` only, not `ApplyTransaction`. |
| `CodegenService.PreviewCodegen` | Implemented for Protobuf and FlatBuffers. For OpenAPI it returns `UNIMPLEMENTED` (OpenAPI client/server codegen is out of scope). |
| Protobuf `UpdateImport` remove | The API proto does not expose a `remove` flag; import entries can be added/updated but not removed via the API in v1. |
| Cross-repo `Search` | Not supported. `SearchRequest.project` + `repo` are required; cross-repo search returns `INVALID_ARGUMENT`. |
| Persisted repo registry | `CreateRepo` / `GetRepo` / `UpdateRepo` echo back defaults; `ListRepos` returns empty; `DeleteRepo` and `DeleteProject` are `UNIMPLEMENTED`. Per-repo compatibility config lives in `[repos.*]` in `schemahub.toml` instead. |
| Cross-repo rename propagation | Not automatic. `Diff`/dependency reads surface the affected importers; the caller issues `UpdateImport` against the downstream repos. |
