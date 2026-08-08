<!-- agent-updated: 2026-07-30T04:23:54Z -->
# schemahub

A collaborative schema-change and serving platform. Humans and software agents
record why a Protobuf, FlatBuffers, or OpenAPI schema should change, apply it
safely through **jj-style version control**, and serve immutable schemas,
descriptors, and generated bindings to systems that store and read data.

## Contents

- [Overview](#overview)
- [Product direction](#product-direction)
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
- [Release artifacts and container](#release-artifacts-and-container)
- [Production operations](#production-operations)
- [HTTP/JSON API](#httpjson-api)
- [Web console](#web-console)
- [Limitations in v1](#limitations-in-v1)

---

## Overview

schemahub stores schemas as structured, parsed representations rather than raw files. A durable **ChangeRecord** can first capture why a human or agent believes a schema should change; executable writes then produce commits with stable jj **change IDs**. Branches (bookmarks) and tags work like git. Concurrent same-declaration edits remain **first-class conflicts** on unprotected bookmarks, while protected bookmarks atomically reject a conflicted final tree; every successful write is recorded in an **operation log** with `undo`. Protobuf and FlatBuffers schemas support field-level mutations (add/remove/rename fields, messages, enums, services); OpenAPI supports a handful of granular path/operation/component mutations plus whole-document push. Compatibility and final-state reference checks run automatically at publication.

**What it is not**: a file server for `.proto` files. Schemas live in the registry; your build pipeline pulls descriptors from it.

See `docs/design.md` for the architecture deep-dive, `docs/grpc-api.md` for the
wire contract, `docs/http-api.md` for the generated browser API contract,
`docs/dependency-discovery.md` for cross-repository downstream discovery,
`docs/authentication.md` for production JWT/JWKS identity,
`docs/format-capabilities.md` for the executable format contract,
`docs/crate-structure.md` for the workspace layout, `docs/openapi-ast.md` for
the OpenAPI schema-format AST, `docs/ui-design.md` for the web-console product
design, `docs/gui.md` for the implemented React GUI architecture and usage,
and `docs/real-world-validation.md` for the scenario-driven hardening
portfolio and bug-evidence contract.

## Product direction

SchemaHub is evolving from direct schema mutation plus history into two explicit
product surfaces:

- A **change control plane** where a human or authenticated agent can record
  intent, build a typed change, validate compatibility, obtain any required
  review, and link the record to the commit that was applied.
- An **immutable serving plane** where data producers and consumers resolve and
  retrieve the exact source or descriptor bundle used to encode stored data.

SchemaHub governs and serves schemas; it does not store application data. See
`docs/product.md` for the product contract, `docs/roadmap.md` for deliverables,
`docs/resources-and-policy.md` for durable project/repository controls,
and `docs/ADR/0001-change-records-and-serving-plane.md` for the architectural
decision.

Start with
[the human-and-agent workflow codelab](docs/codelab-human-agent-schema-workflow.md)
to run the primary agent proposal, human review, Apply, and immutable artifact
serving path end to end. Its interactive companion lives in
[`apps/schemahub-demo`](apps/schemahub-demo). Seven executable real-world
codelabs exercise the primary workflow plus a
[Protobuf commerce rollout](docs/codelab-commerce-protobuf.md),
[FlatBuffers mobile telemetry evolution](docs/codelab-mobile-telemetry-flatbuffers.md),
[concurrent human/agent editing](docs/codelab-concurrent-human-agent.md), and a
[producer/consumer data-pipeline handoff](docs/codelab-data-pipeline-handoff.md),
then add
[multi-file payments dependency closure](docs/codelab-payments-dependency-closure.md)
and [private tenant isolation](docs/codelab-private-tenant-isolation.md).
Run all seven real-server scenarios with
`./codelabs/real-world/run-all.sh`. A successful run also emits normalized
`GA-READINESS.md` and `ga-readiness.json` reports under its evidence directory;
candidate CI retains the same secret-free evidence as a checksummed release
input.

---

## Workspace structure

Nine crates, organized as two layers (format-agnostic JJ + per-format compilers):

| Crate | Purpose |
|---|---|
| `schemahub-types` | Shared types, the `Compiler` trait, auth traits (`AuthnProvider`/`AuthzPolicy`), errors |
| `schemahub-jj` | jj-lib over a swappable `ObjectDb`: `DbBackend` + `DbOpStore`; redb default, in-memory + Postgres impls |
| `schemahub-core` | Orchestration: durable change records, mutations, transactions, compatibility, conflicts, history, GC, and provider-independent RBAC |
| `schemahub-api` | tonic/prost-generated gRPC bindings for the protos in `crates/schemahub-api/proto/schemahub/v1/` |
| `schemahub-compiler-protobuf` | Protobuf compiler — wraps `protobuf-rs` (parse/AST/codegen); owns the `.proto` printer + mutation validator + compat checker |
| `schemahub-compiler-flatbuffers` | FlatBuffers compiler — wraps `flatbuffers-rs`; owns the `.fbs` printer |
| `schemahub-compiler-openapi` | OpenAPI compiler — in-tree AST/parser/printer (`docs/openapi-ast.md`) |
| `schemahub-server` | gRPC/HTTP server (binary `schemahub-server`) — change control, immutable artifacts, and production JWT/JWKS composition |
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
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"

# Minimal authenticated development identity (required for project creation)
cat > schemahub.toml <<'EOF'
[auth.tokens.dev-owner]
id = "dev-owner"
kind = "human"
EOF

# Embedded redb, bound only to the Tailscale interface
schemahub-server --listen "${TAILSCALE_IP}:50051" \
  --db ./schemahub.db --config schemahub.toml

# Postgres backend (requires --features postgres)
schemahub-server --db-url postgres://user:pass@host:5432/dbname \
                 --config schemahub.toml
```

If `TAILSCALE_IP` is set and `--listen` is omitted, the server binds to that
interface on the configured port. It falls back to `0.0.0.0` only when the
environment variable is absent. Address clients through the full MagicDNS
value in `TAILSCALE_HOST`.

### Configure the CLI

```toml
# ~/.schemahub/config
[default]
server = "http://shuoze25-yuacx.tail8f3b66.ts.net:50051"
token  = "dev-owner"

[prod]
server = "https://schemahub.example.com"
token  = "my-token"
```

Use `--profile prod` to select a non-default profile, or override per-invocation
with `--server` / `--token` (or env vars `SCHEMAHUB_SERVER` /
`SCHEMAHUB_TOKEN`). A server address is required; the CLI never guesses a
loopback endpoint. Missing config files are optional, but unreadable or
malformed files fail closed even when command-line overrides are present.

### Quick example

```bash
# Create project + repo in one step
schemahub repo init mycompany/payments

# Record intent before editing (add --json for agent/CI output)
schemahub change note mycompany/payments \
    --title "Add settlement currency" \
    --description "Needed by the multi-currency ledger" \
    --reference PAY-2048

# Push a schema
schemahub schema create --project mycompany --repo payments payments/v1/payment.proto

# Add a field via granular mutation
schemahub field add mycompany/payments/payments/v1/payment.proto CreatePaymentRequest \
    "currency:string:3" --branch main

# Pull (print) the schema back
schemahub schema pull mycompany/payments/payments/v1/payment.proto

# Discover direct downstream importers visible to this identity
schemahub schema dependents mycompany/payments/payments/v1/payment.proto --json

# Pin the revision used by a data producer, then download its descriptor
REVISION="$(schemahub artifact resolve mycompany/payments --at main --json | jq -r '.name')"
schemahub artifact fetch "$REVISION" \
    --schema-path payments/v1/payment.proto \
    --kind descriptors \
    --output payment.desc
```

---

## CLI reference

Global flags (apply to all subcommands):

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--server` | `SCHEMAHUB_SERVER` | profile; otherwise required | Server address; set it to the full Tailscale MagicDNS URL |
| `--token` | `SCHEMAHUB_TOKEN` | _(empty)_ | Auth token |
| `--profile` | — | `default` | Config profile |
| `--json-errors` | `SCHEMAHUB_JSON_ERRORS` | `false` | Emit one machine-readable JSON error object on stderr |

Agent/CI exit codes are stable: `0` success, `1` local error, `2` invalid
argument, `10` unauthenticated, `11` permission denied, `12` not found, `13`
already exists, `14` failed precondition/concurrency conflict, `20` transient
transport failure, `21` resource exhaustion, and `22` server/unimplemented
failure. Clap syntax failures retain exit code `2`.

### `capabilities` — executable format contract

```text
schemahub capabilities [--json]
```

Reads the running server's versioned format and mutation support matrix.
`--json` emits the stable machine-readable form intended for agents, CI, and
client feature negotiation. See `docs/format-capabilities.md`.

### `repo` — project and repo initialization

```
schemahub repo init [--public] [--default-branch B] <project/repo>
```

Creates the project and repository idempotently. Existing resources are reused,
so retrying the setup command is safe. Use this instead of calling
`CreateProject` and `CreateRepo` separately during initial setup.

### `project` — project + member management (RBAC)

```
schemahub project create [--public] <name>
schemahub project get <name> [--include-archived]
schemahub project list [--prefix P] [--page-size N] [--include-archived]
schemahub project set-visibility <name> <public|private> --etag <etag>
schemahub project archive <name> --etag <etag> [--force]
schemahub project member list       <project> [--page-size N] [--json]
schemahub project member add        <project> <identity_id> [--role Reader]
schemahub project member remove     <project> <identity_id>
schemahub project member set-role   <project> <identity_id> --role <role>
schemahub project audit <project> [--page-size N] [--json]
```

Roles: `Reader` / `Writer` / `Maintainer` / `Owner`. `CreateProject` requires an authenticated identity (anonymous callers are rejected); the caller becomes the project's Owner in the same database transaction. Updates and archive require the current ETag. Archive retains repository/schema history, is Owner-only, and needs `--force` when repository records exist. Member mutation is Owner-only and enforces the "last Owner" invariant; readable project members can traverse the bounded identity-ordered list.
Every successful project, member, and repository mutation appends a typed
before/after audit event in the same database transaction. `project audit` is
Owner-only; `--json` emits stable machine-readable event resources.
Each page is read through an immutable newest-first index and a bounded backend
range query; it does not load the project's complete history.
Project-keyed cross-instance coordination keeps authorization and the
last-Owner invariant valid under concurrent administration.

### `change` — human/agent change notes

```text
schemahub change note <project/repo> --title T [--description D] [--reference R]... [--target-bookmark main] [--base-revision H] [--id ID]
schemahub change get <projects/P/repos/R/changes/C>
schemahub change list <project/repo> [--status draft] [--page-size 50] [--page-token TOKEN]
schemahub change update <projects/P/repos/R/changes/C> --etag E [--title T] [--description D] [--reference R]... [--clear-references] [--target-bookmark B] [--base-revision H]
schemahub change add-source <projects/P/repos/R/changes/C> --etag E --schema-path P --file FILE [--format-id F]
schemahub change add-mutation <projects/P/repos/R/changes/C> --etag E --schema-path P --format-id F --operation-file FILE
schemahub change delete-schema <projects/P/repos/R/changes/C> --etag E --schema-path P [--format-id F]
schemahub change validate <projects/P/repos/R/changes/C> --etag E
schemahub change ready <projects/P/repos/R/changes/C> --etag E
schemahub change approve <projects/P/repos/R/changes/C> --etag E [--reason R]
schemahub change reject <projects/P/repos/R/changes/C> --etag E --reason R
schemahub change apply <projects/P/repos/R/changes/C> --etag E --request-id ID
schemahub change abandon <projects/P/repos/R/changes/C> --etag E
```

Add `--json` anywhere on a `change` command for stable machine-readable output.
`--reference` is repeatable for issue, incident, design, or automation
correlation; update replaces the ordered set, while `--clear-references`
removes it. Actor kind, identity, display name, and agent delegation are derived
from the bearer token; callers cannot supply or forge them. Updates and
abandonment require the current ETag. Deletion is a soft transition to
`ABANDONED`, so the audit record remains readable.

### `schema` — schema lifecycle

```
schemahub schema create [--project P] [--repo R] [--branch B] [--name N] [--base-revision H] <file>
schemahub schema update [--project P] [--repo R] [--branch B] [--name N] [--base-revision H] [--force] <file>
schemahub schema pull   [--branch B] <project/repo/schema_name>
schemahub schema delete [--branch B] [--base-revision H] [--force] <project/repo/schema_name>
schemahub schema dependents <project/repo/schema_name> [--json]
```

The CLI detects format from the file extension and sends it explicitly:
`.proto` → Protobuf, `.fbs` → FlatBuffers, `.yaml`/`.yml`/`.json` → OpenAPI.
The server rejects an unspecified or mismatched create format.

`--force` on update or delete bypasses protected-bookmark compatibility and
requires Maintainer. It is durably audited and never bypasses reference
integrity: remove/update live unpinned importers first. Immutable pinned imports
continue to resolve their retained commit.

`dependents` performs a bounded scan of every repository readable by the caller
and returns direct import edges. Each result includes the exact importing commit,
pin state, and a per-repository immutable snapshot manifest. It is an advisory
coordination read, not a global transaction or automatic downstream rewrite;
see `docs/dependency-discovery.md`.

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
schemahub branch list   <project/repo> [--prefix PREFIX] [--page-size N]
schemahub branch merge  [--into B] [--base-revision H] [--message MSG] [--project P] [--repo R] <source>
```

Merge is a real jj 3-way merge: the server creates a 2-parent merge commit whose tree is jj's auto-merge over the merge base. Same-declaration divergence becomes a stored first-class conflict (resolve with `schemahub resolve`), not an error.

### `tag` — tag management

```
schemahub tag create [--commit H | --branch B] [--message MSG] [--project P] [--repo R] <name>
schemahub tag delete --force [--project P] [--repo R] <name>
schemahub tag list   <project/repo> [--prefix PREFIX] [--page-size N]
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

`get` downloads the schema descriptor (`FileDescriptorSet` for Protobuf, reconstructed `.fbs` bundle for FlatBuffers, resolved YAML for OpenAPI) and prints it to stdout, following transitive imports automatically. `preview` renders generated source for the chosen language — implemented for Protobuf and FlatBuffers; OpenAPI returns `UNIMPLEMENTED`. Multi-file previews retain the explicitly requested root schema: Protobuf resolves imported and nested named types across the closure, while FlatBuffers uses only the requested file's `root_type` for root helpers. `--rust-pluggable-buffer` is FlatBuffers Rust only: it asks the sibling `flatc-rs-codegen` backend to generate `FlatBufferRead`-based readers and `root_as_<name>_in(&buffer)` helpers for custom byte-buffer providers.

Ref formats accepted by `--at` / `--branch`:
- `main` (branch name, default)
- `@<sha>` (pinned commit)
- `tag:<name>` (tag)

### `artifact` — immutable schema serving

```text
schemahub artifact resolve <project/repo> [--at main] [--json]
schemahub artifact fetch <projects/P/repos/R/revisions/H> --schema-path S
    [--kind source|descriptors|generated-code] [--language rust]
    [--output FILE] [--if-none-match sha256:HEX] [--json]
schemahub artifact verify <projects/P/repos/R/revisions/H> --schema-path S
    [--kind source|descriptors|generated-code] [--language rust]
    --digest sha256:HEX [--json]
```

`resolve` converts a mutable branch or tag into a repository-scoped immutable
revision. On the first `fetch`, SchemaHub atomically persists canonical source,
native descriptor bundles, or generated code before returning it; later reads
reuse those exact bytes across restarts and renderer upgrades. `verify`
downloads the bytes, recomputes SHA-256 locally, checks the server-declared
digest, and exits nonzero on any mismatch. See `docs/serving.md` for the
first-materialization, digest, and cache contracts.

### `diff` — semantic diff

```
schemahub diff [--schema-path S] <project/repo> <base..head>
```

Range example: `main..feature/add-user`. Output lists added, removed, and changed declarations between the two refs.

---

## gRPC API

All services are defined in `crates/schemahub-api/proto/schemahub/v1/`. With
`TAILSCALE_IP` set, the default listener is that interface on port 50051; the
wildcard address is only the local-development fallback.

### SchemaService

| RPC | Description |
|---|---|
| `CreateSchema` | Create a new schema on a branch. Returns `ALREADY_EXISTS` if the name is taken. |
| `UpdateSchema` | Replace a schema's full source. Runs compatibility check on protected branches. |
| `DeleteSchema` | Delete a schema. Live same-repository unpinned importers always block deletion; `force` overrides compatibility only. |
| `ApplyMutation` | Apply a single granular mutation to one declaration. |
| `ApplyTransaction` | Apply up to 100 mutations across up to 20 schemas atomically. |

### RefService

Branch names map to jj **bookmarks**; the branch RPCs are a compatibility-shaped face over them.

| RPC | Description |
|---|---|
| `GetCommit` | Fetch commit metadata by hash. |
| `ListCommits` | Stream real commit-graph entries newest-first, with an optional exclusive retained stop commit and schema-touch filter. Initial metadata reports the exact traversal root. |
| `Diff` | Per-declaration semantic diff between two immutable resolved snapshots; the response reports both exact commit IDs. |
| `CreateBranch` / `DeleteBranch` / `ListBranches` / `GetBranch` | Branch CRUD. Lists use repository/filter-bound opaque pagination over stable name order; Get is a direct named lookup. |
| `CreateTag` / `DeleteTag` / `ListTags` | Tag CRUD. Lists use repository/filter-bound opaque pagination over stable name order. `DeleteTag` requires `force=true`. |
| `Merge` | Real jj 3-way merge with first-class conflicts; produces a 2-parent merge commit. Same-declaration divergence is recorded as a stored conflict, not an error. |

### HistoryService

The wire surface for the jj operation log and first-class conflict resolution (new in v2).

| RPC | Description |
|---|---|
| `Log` | Commit/change history graph from one immutable resolved ref. Each entry carries both the content-addressed `commit_id` and stable jj `change_id`; the response reports `at_commit`. Honors `at` + `limit` (default 100). |
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

Every read resolves a mutable branch/tag once and uses the resulting immutable
commit for the complete payload. An omitted ref selects the repository's
configured default bookmark. A raw commit is accepted only when it belongs to
the named repository's retained history; globally deduplicated objects cannot
be read or published through another repository's coordinates.

### ExplorationService

| RPC | Description |
|---|---|
| `ListSchemas` | All schema names at one immutable repository snapshot; returns `at_commit`. |
| `ListDeclarations` | Top-level declarations at one immutable snapshot, with optional kind filter; returns `at_commit`. |
| `GetDeclaration` | Summary and full detail for a named declaration, plus the exact source commit. |
| `GetSchemaSource` | Reconstructed source text plus the exact source commit. |
| `Search` | Search declaration-name prefixes across one repository snapshot. Supports kind/limit filters and returns `at_commit`. |
| `FollowType` | Resolve the actual type of `field_name` (or OpenAPI property) inside `declaration_name`, following the matching local or imported declaration. Returns source/target commits, pin state, import path, summary, and detail. Scalars return `INVALID_ARGUMENT`; missing or ambiguous references fail explicitly. |
| `ListDependencies` | List normalized direct or transitive import edges. Each edge reports importing/target coordinates and commits, stored pin/path, and whether the target was readable and resolved. |
| `ListDependents` | Find direct downstream imports across repositories visible to the caller. Returns pin state plus an immutable snapshot manifest for every repository inspected. |

### CodegenService

| RPC | Description |
|---|---|
| `GetDescriptors` | Return a reconstructed descriptor for a schema and all its transitive imports. Protobuf → `FileDescriptorSet`; FlatBuffers → reconstructed `.fbs` bundle; OpenAPI → resolved YAML. |
| `PreviewCodegen` | Render generated source code server-side for the requested language. Implemented for Protobuf and FlatBuffers; OpenAPI returns `UNIMPLEMENTED`. Response carries the rendered text (no files written). `rust_pluggable_buffer=true` enables FlatBuffers Rust pluggable-buffer readers. |

### ServingService

| RPC | Description |
|---|---|
| `ResolveRevision` | Resolve a bookmark, tag, or commit once to `projects/{project}/repos/{repo}/revisions/{commit}` and validate repository ownership. |
| `GetSchemaArtifact` | Fetch immutable canonical source, descriptors, or generated code with payload/closure digests, dependency metadata, and conditional-read support. |

### ProjectService

Manages the durable project / repository / membership hierarchy in the selected
redb/PostgreSQL database.

| RPC | Description |
|---|---|
| `CreateProject` / `GetProject` / `UpdateProject` / `ListProjects` | Durable resources with caller-owned creation, ETags, field masks, timestamps, and RBAC-filtered pagination over bounded active/all name-index ranges. A filtered page can be empty while still carrying a continuation token. |
| `DeleteProject` | Soft archive; requires ETag and `force=true` when repositories exist. Descendant history is retained and normal runtime access fails closed. |
| `CreateRepo` / `GetRepo` / `UpdateRepo` / `ListRepos` | Durable repository resources and effective compatibility/review/serving policy with ETags and bounded per-project name-index pagination. |
| `DeleteRepo` | Soft archive; retained JJ refs require `force=true`. |
| `AddMember` / `RemoveMember` / `UpdateMemberRole` / `ListMembers` | Real role-based membership with Owner-only mutation, the "last Owner" invariant, and project-bound pagination over bounded identity-key ranges. An inactive-tombstone page can be empty while carrying a continuation token. |
| `ListControlPlaneAuditEvents` | Owner-only, cursor-paginated immutable project/member/repository events with server-derived actor, event time, and typed before/after snapshots. |

### ChangeService

The complete policy-neutral control-plane lifecycle is durable in both redb
and PostgreSQL.

| RPC | Description |
|---|---|
| `CreateChange` | Create a note-only or executable draft under `projects/{project}/repos/{repo}`. Actor metadata is server-derived. |
| `GetChange` / `ListChanges` | Read one record or a bounded repository-index page in stable creation order. List supports status filtering and parent/filter-bound opaque cursors. |
| `UpdateChange` | Patch draft fields with a field mask and ETag. Stale ETags return `ABORTED`. |
| `ValidateChange` / `MarkChangeReady` | Resolve and validate the exact edit snapshot, persist findings, and advance a passing record to review-ready state. |
| `ApproveChange` / `RejectChange` | Record an authenticated maintainer review without allowing self-review. |
| `ApplyChange` | Publish once through a durable lease and JJ correlation receipt; stable request retries return the same result. |
| `DeleteChange` / `AbandonChange` | Soft-delete a draft/ready record by transitioning it to `ABANDONED`. |

### AdminService

| RPC | Description |
|---|---|
| `RunGC` | Garbage-collect unreferenced objects. Supports `dry_run` and project/repo scoping. |
| `RebuildIndex` | Rebuild the search and dependency indices from scratch. |
| `GetServerConfig` | Returns server limits and configuration. |
| `GetFormatCapabilities` | Returns the versioned, executable format and mutation support contract. |

---

## Schema formats

| Format | Extensions | Granular mutations | Compatibility check | GetDescriptors output |
|---|---|---|---|---|
| Protobuf | `.proto` | Yes — full suite | Yes | `FileDescriptorSet` (binary proto) |
| FlatBuffers | `.fbs` | Yes — see restrictions | Yes | Reconstructed `.fbs` bundle |
| OpenAPI | `.yaml` `.yml` `.json` | Yes — 6 selected granular ops plus whole-document push; direct or transactional | Yes | Resolved YAML (multi-document for closures) |

Run `schemahub capabilities --json` for the authoritative matrix served by the
running binary.

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
| `RenameService` | Rename a service while preserving its RPCs. |
| `AddRpc` | Add an RPC with request/response types and streaming flags. |
| `RemoveRpc` | Remove an RPC. |
| `RenameRpc` | Rename an RPC. Rejects if the new name already exists. |
| `ChangeRpcType` | Change an RPC request and/or response type. |

**Imports** (schema-level)

| Mutation | Description |
|---|---|
| `UpdateImport` | Add, update, or remove an import dependency. `to_tag` is resolved immediately and stored as an immutable commit pin, just like `to_commit`. Used by `ListDependencies` to track the import graph. |

### FlatBuffers

**Fields** (target: a table declaration)

| Mutation | Description |
|---|---|
| `AddField` | Append a field with type, default value, doc comment. Always added at the end — slot order is frozen. |
| `DeprecateField` | Mark a field as deprecated. Use instead of `RemoveField` (not supported in FlatBuffers). |
| `RenameField` | Rename a field. Wire identity (slot index) is unchanged. |
| `ChangeFieldType` | Change a table field type; compatibility policy determines whether publication is allowed. |

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
| `RemoveEnum` / `RenameEnum` | Remove an unreferenced enum or rename it and update same-file references. |
| `AddEnumValue` | Add a value to an existing enum. |
| `RemoveEnumValue` / `RenameEnumValue` | Remove or rename a value while preserving valid defaults. |
| `AddUnion` | Add a union with a list of initial member table names. |
| `RemoveUnion` / `RenameUnion` | Remove an unreferenced union or rename it and update same-file references. |
| `AddUnionMember` | Add a table to an existing union. Rejects duplicates. |
| `RemoveUnionMember` | Remove a table from an existing union. Rejects if the member is not present. |

**Imports**

| Mutation | Description |
|---|---|
| `UpdateImport` | Add, update, or remove an include. `to_tag` is resolved immediately and stored as an immutable commit pin. |

### OpenAPI

OpenAPI supports a focused set of granular operations plus a whole-document push (used internally by `UpdateSchema`). Every listed op is reachable via both `ApplyMutation` and `ApplyTransaction`; transaction reference checks run against the final document.

| Mutation | Description |
|---|---|
| `PushDocument` | Whole-document replacement. Used internally by `UpdateSchema`. |
| `AddPath` | Add a new empty `path:<pattern>` declaration. Fails if the path already exists. |
| `RemovePath` | Remove the `path:<pattern>` declaration. |
| `AddOperation` | Add one HTTP method (`get`/`post`/`put`/`delete`/`patch`/`head`/`options`/`trace`) to a path item. |
| `RemoveOperation` | Remove one HTTP method from a path item. |
| `AddComponentSchema` | Add a new `schema:<name>` declaration with a JSON Schema type. |
| `RemoveComponentSchema` | Remove the `schema:<name>` declaration; rejects a remaining local `$ref`. |

Any other granular OpenAPI op returns `UnsupportedInV1`. See `docs/openapi-ast.md` for the AST and the per-declaration key scheme (`path:`, `schema:`, `param:`, `response:`, `requestBody:`).

---

## Version control model

schemahub uses the **Jujutsu (jj) model** via `jj-lib` (default features off — no git interop), with all persistence delegated to the `ObjectDb`:

- **Commits** — Immutable, content-addressed (`CommitId` via blake2b). Each carries a stable **`ChangeId`** that survives rewrite/rebase/squash — the durable identity of an edit even after history is rewritten.
- **Trees** — Per-declaration storage: a schema file is a jj subtree `<schema-file>/`; each top-level declaration is a file entry `<schema-file>/<Decl>` holding the `DeclBlob`; `<schema-file>/__meta__` holds the file's `MetaBlob` (package, imports, syntax/edition).
- **Branches (bookmarks)** — Mutable named refs. Names support glob patterns for protection rules (e.g., `main`, `release/*`). The branch RPCs are a compatibility-shaped face over jj bookmarks; list responses are bounded and cursor-paginated.
- **Tags** — Immutable refs. Lightweight tags are just a ref; annotated tags carry a message, tagger, and timestamp. Tag lists use the same bounded cursor contract.
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

The server picks one of three mutually exclusive modes at startup:

### Noop (default)

If `schemahub.toml` has no `[auth]` section (and no `[projects.*]` bootstrap), the server installs `NoopAuthn` (every request is `Identity::Anonymous`) and `NoopAuthz` (every action allowed). Tokens are accepted but ignored. This is the getting-started default.

**Project + member RPCs fail fast in Noop mode.** Because there is no role store or project store to write into, `schemahub project create`, `schemahub project member add|remove|set-role`, and the underlying `ProjectService.{CreateProject, AddMember, RemoveMember, UpdateMemberRole}` RPCs return a `FailedPrecondition` error pointing at `[auth]` rather than silently no-op'ing. To use those commands, add an `[auth]` section (see below) — even a one-token table is enough to switch on the real RBAC layer.

### Static BearerToken + RBAC (development)

When `[auth].tokens` is non-empty, the real RBAC layer turns on automatically:

- **`BearerTokenAuthn`** — a static `Bearer <token> → Identity` table from `[auth].tokens`.
- **`RoleBasedAuthz`** — project-scoped role checks.
- **`ObjectDbRoleStore` + `ObjectDbProjectStore`** — transactional records in the selected redb/PostgreSQL database. Former JSON files under `[auth].data_dir` are one-time migration inputs.

Four roles, descending: `Owner` / `Maintainer` / `Writer` / `Reader`. `--force` requires `Maintainer`+; `ManageProject` (member CRUD) is `Owner`-only. The server enforces a **last Owner** invariant: removing or downgrading the only Owner of a project fails fast.

`[projects.<name>]` blocks seed missing projects and reconcile configured roles
at startup. See `docs/resources-and-policy.md` for lifecycle and migration
details.

### JWT + RBAC (production)

When `[auth.jwt]` is configured, the server validates externally issued bearer
JWTs against an HTTPS or local-file JWKS and installs the same durable
`RoleBasedAuthz` layer. Static tokens and JWT configuration cannot coexist.

The verifier requires explicit issuer, audience, asymmetric algorithms, token
type, identity prefix, refresh/staleness bounds, and payload-size limits. It
requires `iss`, `aud`, `sub`, and `exp`, supports trusted human/agent/service
audit claims, atomically rotates key sets, retains the last known-good set on a
refresh failure, and fails closed when that set exceeds its configured age.
`/readyz` reports stale verification keys as an authentication failure.
The locked verifier uses jsonwebtoken's AWS-LC backend, including tested RS256
JWK verification; release CI rejects known vulnerable or unsound Rust
dependency drift.

SchemaHub is the resource server, not the token issuer: OAuth login, token
issuance, and revocation live at the external identity provider. See
`docs/authentication.md` for the complete configuration, claims contract,
rotation drill, and security boundaries.

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

Internally, schema state is content-addressed via jj-lib's blake2b hashing —
files (`DeclBlob`/`MetaBlob`), trees, commits, views, plus a per-`(project,
repo)` operation log. Mutable ChangeRecords, projects, memberships, and
repository policies live in independent ObjectDb resource-record collections
with transactions and compare-and-swap. Project/member/repository mutations
atomically append immutable, project-partitioned control-plane audit events;
the resource update cannot commit without its event and ordered index entry.
Audit reads page that index with bounded range queries on redb and PostgreSQL
and validate every index target and typed event before returning it.
Membership reads use the existing project-prefixed, hex-identity primary keys
as their ordered range and never scan another project's roles.
ChangeRecord creates and lifecycle transitions likewise maintain
repository-scoped creation-order and status indexes atomically. Public
`ListChanges` pages use bounded range reads and validate each index target.
An existing pre-index database is backfilled once on the first ChangeRecord
ledger operation and records a durable completion marker.

PostgreSQL uses a fixed long-lived async executor instead of spawning a thread
per query. Embedded, checksum-verified SQLx migrations run before readiness;
the initial migration adopts pre-migration databases without rewriting data.
GC discovers every repository in the shared store and uses a mutation/GC
read-write fence (a database advisory lock on PostgreSQL) so a concurrent write
cannot race mark-and-sweep.

The `ObjectDb` trait is the only persistence seam: implementing it (plus the per-repo `set_ref`/`get_ref` ref table) is enough to add a new backend.

---

## Configuration

### Server flags

| Flag | Default | Description |
|---|---|---|
| `--print-openapi` | `false` | Print the generated OpenAPI 3.1 HTTP contract and exit without loading config or storage |
| `--check-ready URL` | _(none)_ | Check an HTTP readiness endpoint and exit; used by the distroless image health check |
| `--listen` | `TAILSCALE_IP:50051` when set; wildcard fallback otherwise | Address to bind |
| `--db` | `schemahub.db` | Path to the redb database file (honored when `storage.backend = "redb"`) |
| `--db-url` | _(none)_ | Postgres connection URL (honored when `storage.backend = "postgres"`; requires `--features postgres`) |
| `--config` | `./schemahub.toml` when present | Path to a required server config file; missing or unreadable explicit paths fail startup |
| `--http-listen` | _(disabled)_ | HTTP BFF plus `/healthz`, `/readyz`, `/metrics`, and `/api/openapi.json` listener |
| `--gui-dir` | `[http].gui_dir` | Production Vite bundle to serve from the HTTP listener; requires `index.html`, `assets/`, and `--http-listen` |
| `--shutdown-timeout-seconds` | `30` | Maximum HTTP/gRPC graceful-drain period; env: `SCHEMAHUB_SHUTDOWN_TIMEOUT_SECONDS` |
| `--log-format` | `json` | Structured `json` or interactive `pretty`; env: `SCHEMAHUB_LOG_FORMAT` |

### Server config file

`schemahub.toml` (optional):

```toml
[storage]
backend = "redb"          # or "postgres"
path    = "schemahub.db"  # for redb
# url   = "postgres://..." # for postgres

[http]
max_request_body_bytes = 8388608
# Serve the version-matched production console from the same origin:
# gui_dir = "/usr/share/schemahub/gui"
# Exact browser origins only when the GUI is hosted separately:
# allowed_origins = ["https://schemahub-gui.example.com"]

[repos."acme/payments"]    # seeds a durable repository resource
default_bookmark    = "main"
compatibility       = "full"           # backward | forward | full | disabled
protected_bookmarks = ["main", "release/*"]

[repos."acme/payments".review]
required_approvals = 1
require_change_record = true

[repos."acme/payments".serving]
source = true
descriptors = true
generated_code = true

[auth]                                  # static development credential mode
data_dir = "schemahub-data"             # legacy projects.json/roles.json import only
[auth.tokens."secret-token-alice"]
id      = "alice"
display = "Alice Example"
kind    = "human"          # default; human | agent | service

[auth.tokens."secret-token-schema-agent"]
id           = "schema-agent"
display      = "Schema Maintenance Agent"
kind         = "agent"
delegated_by = "alice"

[projects.acme]                         # bootstrap project + roles at startup
visibility = "private"                  # or "public"
owners     = ["alice"]
members    = { bob = "Writer", carol = "Reader" }
```

The HTTP BFF is same-origin by default: an empty `allowed_origins` list emits
no CORS permission headers. Cross-origin GUI deployments must list each trusted
canonical `http://` or `https://` origin exactly, including a non-default port;
wildcards, paths, credentials, query strings, fragments, and duplicates fail
startup. Browser cookies are never enabled. `max_request_body_bytes` defaults
to 8 MiB and must remain between 1 KiB and 64 MiB. When `gui_dir` is set,
startup fails unless it resolves to a directory containing a regular
`index.html` and `assets/`, and the complete tree contains only regular files
and directories. Symbolic links fail startup so static assets cannot escape
the configured root. Configuring it without `--http-listen` also fails. The CLI
`--gui-dir` flag overrides the config value. Successful console responses deny
framing, camera, geolocation, and microphone use and apply a self-only content
security policy. Inline scripts, form submission, frames, objects, and
third-party runtime origins are blocked; inline styles remain allowed for the
bundled component library.

For production, replace the static token tables above with an explicit JWT
resource-server policy (do not configure both):

```toml
[auth.jwt]
issuer = "https://identity.example.com"
audiences = ["schemahub"]
algorithms = ["RS256"]
token_type = "at+jwt"
identity_id_prefix = "corp-oidc:"
jwks_url = "https://identity.example.com/.well-known/jwks.json"
clock_skew_seconds = 30
refresh_interval_seconds = 300
max_stale_seconds = 1800
request_timeout_seconds = 5
max_token_bytes = 8192
max_jwks_bytes = 1048576

[projects.acme]
visibility = "private"
owners = ["corp-oidc:248289761001"]
```

### CLI config file

`~/.schemahub/config` (TOML):

```toml
[default]
server = "http://shuoze25-yuacx.tail8f3b66.ts.net:50051"
token  = ""

[prod]
server = "https://schemahub.example.com"
token  = "eyJ..." # raw token; the CLI adds the Bearer scheme
```

Resolution order (first wins): CLI flags → environment variables
(`SCHEMAHUB_SERVER`, `SCHEMAHUB_TOKEN`) → config file profile. The server has
no built-in fallback and must be present in one of those sources; the token may
remain empty for a noop deployment. An existing unreadable or malformed config
file is always an error.

---

## Release artifacts and container

The prepared tag workflow builds tagged Linux, macOS, and Windows archives,
SHA-256 checksums, distribution/container SPDX SBOMs, and a PostgreSQL-capable
multi-architecture image. Release binaries embed auditable Rust dependency
metadata. The Node GUI builder, Rust builder, and distroless runtime are pinned
to exact multi-architecture manifest digests. The Dockerfile frontend and
PostgreSQL/curl CI helpers are digest-pinned too, and the image build's pnpm
coordinate is non-overridable. Native GUI release builds use the same exact
Node 24.18.0 runtime. Binary `--version`, `/healthz`, gRPC server configuration,
`schemahub_build_info`, the archive's generated HTTP OpenAPI document, archive
metadata, and OCI labels all use the tag version. Every native archive contains
the exact production console under `schemahub-gui/`; the release container
serves that same locked build at `/` from `/usr/share/schemahub/gui`. The
console's read-only source viewer is self-contained, and the build contract
rejects known runtime CDN references.

Release CI verifies the exact cargo-audit 0.22.2 crates.io archive, overlays a
repository-reviewed dependency lock, installs that graph with `--locked`, and
self-audits it before scanning SchemaHub. The SchemaHub scan accepts only zero
vulnerabilities plus the exact two reviewed non-runtime warnings. A new,
changed, or disappeared warning fails until the policy is reviewed.
Low-severity pnpm audits cover both frozen web lockfiles. Static contracts guard
the auditor source and lock identities, cargo-auditable's exact clean release
tool graph and isolated invocation, JWT crypto backend, patched dependency
versions, every audit step, and the GUI's sole unused-RSC advisory exception.

Every tag also requires version-matched release notes that state the upgrade,
migration, mixed-version, rollback, compatibility, and known-issue contract.
The workflow validates that source before any artifact publication, injects the
exact SchemaHub/compiler revisions and multi-architecture image digest, then
includes the rendered `RELEASE-NOTES.md` in the SBOM, checksums, release assets,
and GitHub release body. The 1.0 contract also requires the stable staging
evidence and frozen API/limitation boundaries. A finding can declare a
`must_fix_before` version; the release workflow permits prerelease validation
but rejects the stable deadline and every later release until it is fixed.

The runtime image is distroless, runs as UID/GID `65532`, and listens inside the
container on gRPC 50051 and HTTP 8080. Bind published ports to Tailscale:

```bash
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
export SCHEMAHUB_IMAGE=ghcr.io/shuozeli/schemahub:0.9.0-rc.1

docker run --detach --name schemahub \
  --publish "$TAILSCALE_IP:50051:50051" \
  --publish "$TAILSCALE_IP:8080:8080" \
  --volume schemahub-data:/var/lib/schemahub \
  "$SCHEMAHUB_IMAGE"

curl --fail --silent "http://$TAILSCALE_HOST:8080/readyz" | jq
curl --fail --silent "http://$TAILSCALE_HOST:8080/" \
  | grep '<title>SchemaHub Console</title>'
```

No 0.9 tag/image has been published yet. The compiler boundary is reproducible
from independent checkouts: `protobuf-rs` is pinned at
`a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac` and `flatbuffers-rs` at
`59756d23993538b722f68675c35129c3cebb7aa1`. See
[docs/release.md](docs/release.md) for the artifact matrix and candidate gate,
[docs/codelab-deploy.md](docs/codelab-deploy.md) for a complete Tailscale-safe
rehearsal,
[docs/codelab-stable-release-staging.md](docs/codelab-stable-release-staging.md)
for the protected exact-digest stable promotion, and
[docs/compatibility-policy.md](docs/compatibility-policy.md) for the intended
1.0 freeze.

---

## Production operations

The HTTP operations surface provides unauthenticated liveness and
storage/authentication-aware readiness probes plus Prometheus text metrics. HTTP responses propagate or
generate `x-request-id`; gRPC and ChangeRecord transition spans use the same
correlation field. The standard `grpc.health.v1.Health` service is always
registered, including when the HTTP BFF is disabled. SIGINT/SIGTERM changes
both readiness layers before a bounded graceful drain.

See [docs/codelab-operations.md](docs/codelab-operations.md) for Tailscale-safe
startup, log/metric collection, JWT key rotation, versioned migrations, redb
and PostgreSQL backup/restore, upgrades, rollback, and GC recovery drills.

---

## HTTP/JSON API

The optional HTTP listener publishes its generated OpenAPI 3.1 document at
`GET /api/openapi.json`. `schemahub.v1` gRPC/protobuf is the designated public
1.0 API; unversioned `/api/*` is a GUI-only BFF outside the public compatibility
promise and emits `x-schemahub-api-surface: gui-bff`. Health, readiness, and
metrics are separately supported operational routes. The same exact-build HTTP
contract can be emitted without starting the server:

```bash
schemahub-server --print-openapi > schemahub-http-openapi.json
```

Native release archives include that JSON generated by their versioned server
binary. See [docs/http-api.md](docs/http-api.md) for generation, authentication,
catch-all path semantics, drift tests, and boundary metadata; see
[ADR 0002](docs/ADR/0002-public-api-and-gui-bff-boundary.md) for the decision.

---

## Web console

The React operator console lives in `apps/schemahub-gui`. It uses Vite, React,
TypeScript, and Mantine behind a typed `SchemaHubClient` boundary.
The production path uses the live HTTP/JSON BFF by default (same-origin unless
`VITE_SCHEMAHUB_API_BASE` is set); demo data is explicit with
`VITE_SCHEMAHUB_USE_MOCKS=true`.
Production code views use a bundled accessible line-numbered viewer rather
than fetching Monaco or another editor from a third-party CDN.

Current screens include persisted project/repository navigation, schema detail,
compare, history, direct executable source/deletion ChangeRecord authoring,
draft editing, review/apply, conflict resolution, repository search, immutable
artifact download, authenticated human/agent identity, and admin config.
Project and repository navigation consumes bounded 50-item BFF pages and
offers explicit continuation controls. The BFF binds each opaque token to its
catalog kind, project scope, and name prefix; project summaries no longer
trigger a repository scan merely to calculate a count.
Repository dashboards likewise page schemas, branches, and tags with one
repository/ref-bound continuation while retaining the immutable commit chosen
by the first page. The selected schema page and repository-local name inventory
load together in one tree traversal; declaration counts remain
compiler-validated and dependency counts represent unique declared direct
imports without traversing their targets. ChangeRecord lists page the durable
repository/status index; both screens request additional rows explicitly.
CI exercises both isolated mock edit authoring and a live Chromium governance
journey backed by the real HTTP BFF, redb server, and release CLI: an agent
authors source, a human reviews it, the agent applies it, and descriptor/audit
identity is verified again after restart. The remote-CDP path asserts the
identity control's exact accessible name and closes its connection on every
outcome.

The release container enables the version-matched same-origin console by
default. With its HTTP port mapped to the Tailscale interface, open
`http://shuoze25-yuacx.tail8f3b66.ts.net:8080/`. From a native release archive,
serve its bundled directory explicitly:

```bash
schemahub-server \
  --listen "${TAILSCALE_IP}:50051" \
  --http-listen "${TAILSCALE_IP}:8080" \
  --gui-dir ./schemahub-gui \
  --config schemahub.toml
```

For console development, run Vite separately on the Tailscale interface:

```bash
cd apps/schemahub-gui
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
pnpm install
schemahub-server --listen "${TAILSCALE_IP}:50051" \
  --http-listen "${TAILSCALE_IP}:8080" --config schemahub.toml
VITE_SCHEMAHUB_API_BASE="http://$TAILSCALE_HOST:8080" pnpm run dev -- --force
```

Open `http://$TAILSCALE_HOST:5173/`.

Successful Vite assets matching
`/assets/<name>-<eight-character-content-hash>.<extension>` receive one-year
immutable caching. Any successful unhashed asset is served with `no-cache`, so
a custom bundle cannot pin mutable GUI code across an upgrade. Both HTML and
assets receive the console's CSP, framing denial, browser-feature restrictions,
MIME-sniffing protection, and same-origin referrer policy.

See `docs/gui.md` for the GUI architecture, BFF route map, Tailscale setup, and
troubleshooting. See `docs/ui-design.md` for the product and component design.

---

## Limitations in v1

| Feature | Status |
|---|---|
| OpenAPI mutation scope | The selected 1.0 surface is `AddPath`, `RemovePath`, `AddOperation`, `RemoveOperation`, `AddComponentSchema`, `RemoveComponentSchema`, and whole-document `PushDocument`. Other granular OpenAPI edits are not advertised. All selected operations support direct and transactional application. |
| OpenAPI external dependency scope | Supported schema, parameter, response, and request-body component `$ref` values using logical SchemaHub paths participate in dependency discovery, immutable closure serving, `FollowType`, and deletion guards. Explicit `./`/`../` paths resolve within the repository. Network URLs, arbitrary fragments, repository escapes, `$ref` siblings, and standalone reference shapes that the selected AST cannot preserve are rejected; other OpenAPI component categories are outside the 1.0 dependency guarantee. Source refs are live/unpinned. |
| `CodegenService.PreviewCodegen` | Implemented for Protobuf and FlatBuffers. For OpenAPI it returns `UNIMPLEMENTED` (OpenAPI client/server codegen is out of scope). |
| Cross-repo `Search` | Not supported. `SearchRequest.project` + `repo` are required; cross-repo search returns `INVALID_ARGUMENT`. |
| Durable resource contract | D3 is implemented: project/repository persistence, ETags, archive, policy, JSON migration, atomically maintained bounded catalogs, immutable tag names, repository-owned causal bases, and bounded restart-safe direct-write receipts. |
| ChangeRecord workflow | Executable draft edits, compiler validation, Ready/review, policy-gated durable Apply, JJ correlation/recovery, idempotent receipts, and CLI JSON are implemented. The GUI can create or ETag-update note-only drafts with complete source replacements and schema deletions; compiler-specific granular mutation builders remain a CLI/gRPC workflow. Redb process-restart recovery and 32-writer lease/receipt convergence are covered by release tests. |
| Cross-repo dependency coordination | `ListDependents` and `schema dependents --json` provide a bounded, authorization-filtered direct-edge scan with per-repository immutable snapshots. There is no global snapshot, transitive reverse traversal, automatic rename propagation, or cross-repository transaction; callers issue explicit downstream ChangeRecords and should prefer immutable pins for durable data. |
| Atomic publication policy | Delivered. A backend repository guard spans final merge, protected-conflict and live-import validation, JJ commit, and operation-head publication; PostgreSQL coordinates it across instances. |
| Transaction deadline | Delivered. Requests are bounded to 100 operations/20 schemas and `ApplyTransaction` has an independent 30-second server timer plus a cooperative Core deadline checked again at the atomic publication boundary. Clients may use a shorter deadline. |
| Cross-release artifact bytes | Delivered through versioned first-materialization storage. The first successful response is persisted atomically and reused byte-for-byte after restart or renderer upgrade; corrupt records fail closed. Servers that predate this contract are excluded from rolling upgrade/downgrade windows. |
| Production identity | External JWT/JWKS verification with strict claims, key rotation, delegated-agent audit metadata, and stale-key readiness is implemented. Interactive browser login and per-token revocation remain the external identity provider's responsibility. |
| Browser HTTP boundary | Same-origin by default. Cross-origin bearer-token access requires an exact `[http].allowed_origins` entry; request bodies are bounded by `[http].max_request_body_bytes`. Cookies/credentialed CORS are not enabled. |
| Public API versus BFF | `schemahub.v1` gRPC/protobuf is the public 1.0 API. Unversioned `/api/*` is a GUI-only, same-release BFF excluded from the 1.x API compatibility promise and labeled in responses/OpenAPI. Project/repository navigators, repository dashboards, and ChangeRecord lists all use bounded continuations; dashboard schema pages remain pinned to their first immutable commit. These DTOs are still same-release projections, not public REST list contracts. A future public REST API requires a separate versioned contract. |
| Release publication | CI, archives, the distroless image, checksums, provenance, dependency audits, and auditable SBOMs are prepared and locally rehearsed. A tag cannot publish before the full CI matrix passes or with mutable/missing compiler provenance. Stable publication additionally requires a protected-environment attestation that matches the exact source, image digest, and retained GA evidence. The live `PROTOBUF_RS_REF` and `FLATBUFFERS_RS_REF` repository variables now match the immutable compiler coordinates in `Cargo.lock`. No RC is published; publication of the current SchemaHub tree, protected staging/provider configuration, clean candidate evidence, and explicit tag authorization remain. |
