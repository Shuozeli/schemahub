# schemahub

A general-purpose schema registry server. Manage Protobuf, FlatBuffers, and OpenAPI schemas via gRPC and a CLI — with git-style version control, granular mutations, compatibility enforcement, and code generation built in.

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
- [Limitations in v1](#limitations-in-v1)

---

## Overview

schemahub stores schemas as structured, parsed representations rather than raw files. Every change is a commit. Branches and tags work like git. Protobuf and FlatBuffers schemas support field-level mutations (add/remove/rename fields, messages, enums, services) applied atomically through a structured API — no text editing required. Compatibility checks run automatically on protected branches.

**What it is not**: a file server for `.proto` files. Schemas live in the registry; your build pipeline pulls descriptors from it.

---

## Workspace structure

| Crate | Purpose |
|---|---|
| `schemahub-types` | Shared types, auth traits, error types, compatibility definitions |
| `schemahub-storage` | `StorageBackend` trait + redb implementation |
| `schemahub-core` | Business logic: mutations, version control, compatibility, search index |
| `schemahub-api` | Proto definitions and generated gRPC bindings |
| `schemahub-plugin-protobuf` | Protobuf: parse, print, diff, compat, mutations, codegen |
| `schemahub-plugin-flatbuffers` | FlatBuffers: parse, print, diff, compat, mutations |
| `schemahub-plugin-openapi` | OpenAPI: parse, print, diff, compat (whole-document only in v1) |
| `schemahub-server` | gRPC server — SchemaService, RefService, ExplorationService, CodegenService, ProjectService, AdminService |
| `schemahub-cli` | `schemahub` CLI binary |

---

## Getting started

### Build

```bash
cargo build --release
```

Produces two binaries: `schemahub-server` and `schemahub` (the CLI).

### Run the server

```bash
schemahub-server --listen [::1]:50051 --db ./schemahub.db
```

Both flags are optional (those are the defaults).

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

Merge is fast-forward only in v1. The target branch must be a direct ancestor of the source.

### `tag` — tag management

```
schemahub tag create [--commit H | --branch B] [--message MSG] [--project P] [--repo R] <name>
schemahub tag delete --force [--project P] [--repo R] <name>
schemahub tag list   [--project P] [--repo R] [--prefix PREFIX]
```

Providing `--message` creates an annotated tag. Without it, the tag is lightweight.

### `log` — commit history

```
schemahub log [--branch B] [--limit N] [--project P] [--repo R]
```

Default limit: 20 commits.

### `codegen` — descriptor generation

```
schemahub codegen get     [--branch B | --at @<sha> | --at tag:<name>] <project/repo/schema>
schemahub codegen preview [--branch B] <project/repo/schema>
```

`get` downloads the schema descriptor (FileDescriptorSet for Protobuf, YAML for OpenAPI/FlatBuffers) and prints it to stdout, following transitive imports automatically. `preview` is not yet implemented server-side.

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

All services are defined in `crates/schemahub-api/proto/schemahub/v1/`. The default listen address is `[::1]:50051`.

### SchemaService

| RPC | Description |
|---|---|
| `CreateSchema` | Create a new schema on a branch. Returns `ALREADY_EXISTS` if the name is taken. |
| `UpdateSchema` | Replace a schema's full source. Runs compatibility check on protected branches. |
| `DeleteSchema` | Delete a schema. Rejected if importers exist unless `force=true`. |
| `ApplyMutation` | Apply a single granular mutation to one declaration. |
| `ApplyTransaction` | Apply up to 500 mutations across up to 20 schemas atomically. |

### RefService

| RPC | Description |
|---|---|
| `GetCommit` | Fetch commit metadata by hash. |
| `ListCommits` | Stream commits in reverse chronological order. |
| `Diff` | Semantic diff between two version refs. |
| `CreateBranch` / `DeleteBranch` / `ListBranches` / `GetBranch` | Branch CRUD. |
| `CreateTag` / `DeleteTag` / `ListTags` | Tag CRUD. `DeleteTag` requires `force=true`. |
| `Merge` | Fast-forward merge. Returns `FAILED_PRECONDITION` if not fast-forwardable. |

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
| `GetDescriptors` | Return a reconstructed descriptor for a schema and all its transitive imports. Protobuf → `FileDescriptorSet`; OpenAPI / FlatBuffers → resolved YAML. |
| `PreviewCodegen` | _(not yet implemented)_ Server-side code generation for a specified language. |

### ProjectService

Manages the project / repo / membership hierarchy.

| RPC | Description |
|---|---|
| `CreateProject` / `GetProject` / `ListProjects` / `DeleteProject` | Project CRUD. `DeleteProject` requires Owner role; fails if repos exist unless `force=true`. |
| `CreateRepo` / `GetRepo` / `UpdateRepo` / `ListRepos` / `DeleteRepo` | Repo CRUD. `CreateRepo` accepts `compatibility_direction`, `protected_branches` (glob patterns), and `default_branch`. |
| `AddMember` / `RemoveMember` / `UpdateMemberRole` / `ListMembers` | Role-based membership. |

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
| FlatBuffers | `.fbs` | Yes — see restrictions | Yes | YAML representation |
| OpenAPI | `.yaml` `.yml` `.json` | No (v1) | Yes | Resolved YAML |

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

Granular mutations are not supported in v1. Use `UpdateSchema` to replace the full document.

---

## Version control model

schemahub uses a git-inspired object model stored in redb:

- **Commits** — Immutable objects with hash, parent hashes, timestamp, author, and message.
- **Trees** — Map schema names to their content blobs (two-level: root tree → per-schema subtree → `__schema__` blob).
- **Branches** — Mutable refs pointing to the latest commit. Branch names support glob patterns for protection rules (e.g., `main`, `release/*`).
- **Tags** — Immutable refs. Lightweight tags are just a ref; annotated tags carry a message, tagger, and timestamp.
- **Merge** — Fast-forward only in v1. Returns `FAILED_PRECONDITION` if the target is not a direct ancestor of the source.

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

**Defaults**: The server ships with `NoopAuthn` (all requests are `Identity::Anonymous`) and `NoopAuthz` (all actions allowed). Replace these with real implementations for production deployments.

---

## Storage

The storage backend is [redb](https://github.com/cberner/redb), an embedded key-value store. The entire database is a single file on disk.

```
schemahub-server --db /var/lib/schemahub/data.db
```

The `StorageBackend` trait in `schemahub-storage` abstracts all storage access. A different backend can be plugged in by implementing the trait.

**Key namespaces used internally:**

| Prefix | Contents |
|---|---|
| `objects/` | Content-addressed blobs (schemas, trees, commits) |
| `refs/branches/` | Branch HEAD pointers |
| `refs/tags/` | Tag pointers |
| `search/` | Declaration name index (used by `Search`) |
| `idempotency/` | Idempotency key → result mappings (24 h TTL) |
| `pending/` | In-flight mutation markers |
| `config/` | Per-repo configuration |

---

## Configuration

### Server flags

| Flag | Default | Description |
|---|---|---|
| `--listen` | `[::1]:50051` | Address to bind |
| `--db` | `schemahub.db` | Path to the redb database file |

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

Resolution order (first wins): CLI flags → config file profile → environment variables → built-in defaults.

---

## Limitations in v1

| Feature | Status |
|---|---|
| OpenAPI granular mutations | Not supported. Use `UpdateSchema` (whole-document replacement). |
| `CodegenService.PreviewCodegen` | Not implemented. |
| Protobuf `UpdateImport` remove | The API proto does not expose a `remove` flag; import entries can be added/updated but not removed via the API in v1. |
| 3-way merge | Not supported. Merge is fast-forward only. |
| Multiple storage backends | Only redb is implemented. The trait is ready for other backends. |
