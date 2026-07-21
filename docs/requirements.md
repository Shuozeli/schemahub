<!-- agent-updated: 2026-07-21T18:31:43Z -->
# schemahub — Requirements

> Requirements only. Detailed design is owned by a separate engineer; this document defines *what* schemahub must do, not *how*. Where this document names a specific technology (jj-lib, protobuf-rs), it is because that choice is now a fixed project constraint, not because the requirement depends on the implementation.
>
> **Revision (2026-05-26):** Two project-level decisions are now fixed and supersede the original git-style framing:
> 1. **Compilers, not bespoke parsers.** Each format is fronted by a real compiler. Protobuf and FlatBuffers **reuse the sibling compiler projects** `protobuf-rs` and `flatbuffers-rs` (their parsers, ASTs, and codegen). schemahub does not hand-roll format parsers.
> 2. **Jujutsu-style version control.** The VCS layer follows the Jujutsu (`jj`) model — stable change identities, first-class conflicts, and an operation log with undo — built on `jj-lib`, persisted to a database. This replaces the original git-style "CAS-reject + fast-forward-only" model.

## Overview

A general-purpose schema registry server that decouples schema definitions from filesystem files. Schemas (Protobuf, FlatBuffers, OpenAPI) are stored as parsed, structured representations in a versioned database and managed via a gRPC API and CLI. Both humans and AI agents are first-class clients.

The system is composed of two clearly separated parts:

- **Format compilers** — one per format. Each turns source text into a structured AST, reconstructs canonical source from the AST, diffs two ASTs, checks compatibility, validates granular edits, and generates code/descriptors. Protobuf and FlatBuffers are thin wrappers over `protobuf-rs` / `flatbuffers-rs`; OpenAPI is in-tree (no sibling compiler exists).
- **Version control system** — format-agnostic. Stores ASTs as content-addressed objects, versions them with Jujutsu-style semantics, and exposes commits/changes, bookmarks, history, diff, and merge. It knows nothing about any format.

## Goals

- Single source of truth for schemas across an organization, accessed by gRPC instead of file paths.
- Durable schema-change intent: humans and authenticated software agents can
  create a note or executable change record, validate it, follow repository
  review policy, and trace it to the exact commit that was applied.
- Immutable schema serving for stored data: producers can persist a resolved
  revision identifier or digest with encoded data, and consumers can retrieve
  the identical schema or descriptor closure later.
- **Jujutsu-style evolution of schemas:** stable change identities that survive rewrite/rebase, an operation log with undo, and first-class conflicts so concurrent edits never hard-fail.
- Agent-friendly: programmatic AST traversal that lets an agent explore a schema without loading the full text into context, and granular edits that an agent can retry safely.
- Pluggable storage and pluggable auth — adopters configure their own.
- Designed so DB-schema management can be added later without re-architecting.

## Non-Goals (v1)

- Database schema management (DDL / table-and-column registry). Future format; the AST and storage abstractions must not preclude it.
- Operating an identity provider or browser-login system. SchemaHub consumes
  externally issued credentials; the GUI does not issue them.
- Executing generated code. The server may preview or serve immutable generated
  source, but it does not compile or run consumer applications.
- Reimplementing protobuf/flatbuffers parsing. The sibling compilers own that; schemahub consumes them as libraries.

## Functional Requirements

### 1. Schema Storage

- **Formats supported in v1:** Protobuf (`.proto`), FlatBuffers (`.fbs`), OpenAPI / Swagger (JSON / YAML).
- **Compiler-owned ASTs:** the structured representation is the AST produced by the format's compiler.
  - Protobuf → `protobuf-rs`'s `FileDescriptorProto` (and its sub-descriptors).
  - FlatBuffers → `flatbuffers-rs`'s `Schema`.
  - OpenAPI → an in-tree AST (see `openapi-ast.md`).
- **Stored representation:** parsed AST + dependency graph. Raw source files are **not** stored.
- **Per-declaration granularity (required).** A schema is stored as **one object per top-level declaration** (one message, one enum, one service; one table, struct, enum, union; one OpenAPI path-group / component), plus a file-level metadata object (package, imports, syntax/edition). The whole file must **not** be stored as a single opaque object — that defeats deduplication, granular diff, granular addressing, and per-declaration conflict resolution.
- **Deterministic round-trip:** the AST must capture enough information (declaration order, comments, options/attributes, formatting hints) that canonical source text can be reconstructed deterministically. The sibling compilers provide the AST and the source-location/comment data; **schemahub owns the printer** (AST → canonical source) for each format, because the sibling compilers do not currently ship one.

### 2. Versioning (Jujutsu-style)

The VCS layer is built on `jj-lib` and follows its model. Concretely required behavior:

- **Commits and stable change IDs.** Every state is a commit (immutable, content-addressed). Every logical edit also has a **change ID** that is stable across amend/rebase/squash — a client (human or agent) can refer to "the edit I made" even after history is rewritten underneath it.
- **First-class conflicts.** A merge, rebase, or concurrent edit that cannot be cleanly combined produces a **committed conflict object** (at per-declaration granularity), not a hard rejection. Conflicts can be inspected and resolved later. The registry may still *refuse to publish* a conflicted state to a protected bookmark (policy), but the VCS itself never loses the ability to record the divergent states.
- **Operation log + undo.** Every operation that changes repository state
  (mutation, bookmark move, GC, role change) is recorded in an operation log.
  An operation can be undone by restoring the repo to a prior operation, subject
  to current protected-bookmark publication policy; undo must not reintroduce a
  protected conflict or broken live import. This is the registry's audit and
  recovery story.
- **Bookmarks** (named pointers to commits) stand in for branches. History, diff, and merge are expressed over the commit graph.
- **Database-backed.** All objects (commits, trees, declaration blobs, conflicts) and the operation log are persisted to a database via custom `jj-lib` backends. schemahub does **not** use jj's on-disk git/file working-copy layout.
- Compatibility checks run when publishing to a protected bookmark; `--force` (elevated role) can override.

### 3. Code Generation

- **Local codegen via CLI:** client pulls descriptors and generates code on the user's machine. **Reuses the sibling compilers' codegen** (`protobuf-rs` Rust/`FileDescriptorSet` output, `flatbuffers-rs` Rust/TS/Dart output) and `codegen-infra`.
- **Server-side preview:** an RPC that renders generated code on demand for inspection. No files written; response is the rendered text.

### 4. Schema Exploration API (Read)

- Tree-walking RPCs designed for agents: resolve a message, list fields, follow a field's type, list dependencies, fetch a single node by path. Per-declaration storage makes each of these a direct object lookup. Lets an agent traverse a large schema incrementally.
- Searchable by name, type, path, and project.
- Every repository-local read must resolve a branch/tag once, use one immutable
  commit for the complete payload, and report that commit. Omitted refs use the
  repository's configured default bookmark. A raw commit must belong to the
  named repository's retained history even when its objects exist in globally
  deduplicated storage.
- Field-type traversal must resolve the requested field/property rather than an
  arbitrary reference in its containing declaration. It must return the
  resolved declaration and source/target snapshot coordinates, preserve
  immutable pins, and fail explicitly for scalar, missing, or ambiguous types.
- Forward dependency traversal must preserve every import edge and distinguish
  its stored pin from the effective immutable target snapshot. Unavailable
  external/builtin targets may be returned as explicit unresolved leaves, but
  invalid pins, storage/decoder failures, and finite-bound exhaustion must fail
  the call instead of yielding a partial closure.
- Direct downstream dependency discovery across every repository visible to the
  caller. The response must identify the exact immutable snapshot used for each
  repository, distinguish pinned from live imports, enforce finite scan bounds,
  and fail rather than silently returning a partial inventory.

### 5. Schema Mutation API (CRUD)

The write counterpart to the Exploration API. Granular RPCs that mutate individual schema elements — not text patches, not whole-file replacements. Designed so an agent or CLI can edit a schema the same way it edits source code: one targeted change at a time.

**Required 1.0 operation set:**

The exact contract is the versioned `GetFormatCapabilities` response and its
checked-in mirror in `format-capabilities.md`. It currently covers selected
field/message/enum/service/RPC/import operations for Protobuf; selected
field/table/enum/union/import operations for FlatBuffers; and seven OpenAPI
document/path/operation/component operations. An operation is public only when
it is advertised by that matrix and exercised through both direct and
transaction conformance tests. Namespace moves, arbitrary comment/option edits,
and other operations absent from the matrix are post-1.0 candidates rather than
implicit requirements.

**Semantics:**

- Each mutation is a typed operation, validated against the AST by the format compiler.
- Each mutation (or transaction) produces a new commit / advances a change.
- Compatibility checks from Section 6 apply when publishing to a protected
  bookmark; an authorized `--force` override is recorded in the durable JJ
  operation.
- Reference integrity is not force-bypassable. Same-file renames update the
  references enumerated in `format-capabilities.md`; referenced declaration or
  component deletion is rejected. Whole-schema deletion rejects any remaining
  same-repository live unpinned import, while immutable commit pins remain
  valid. A ChangeRecord may update/delete consumers and delete the provider in
  one final-state-validated atomic plan. The exact merged tree is checked while
  a backend repository publication guard is held through JJ commit, so both
  consumer-first and delete-first concurrency orderings preserve the invariant.
- Cross-repository propagation is explicit in 1.0: callers use
  `ExplorationService.ListDependents` to discover direct downstream imports at
  per-repository immutable snapshots, then submit coordinated changes.
  SchemaHub does not silently rewrite another repository, claim a globally
  atomic snapshot, traverse reverse dependencies transitively, or provide a
  cross-repository transaction. See `dependency-discovery.md`.
- OpenAPI exposes the selected seven-operation 1.0 surface in the capability
  matrix; additional OpenAPI operations remain post-1.0.

**Concurrency model (Jujutsu-style, replaces the original idempotency/CAS framing):**

The original requirement was at-most-once mutation via client idempotency keys + base-revision CAS that *rejects* on conflict. Under the jj model the durable guarantees come from change IDs and the operation log instead:

- A client-supplied **idempotency key** is honored after authorization to dedupe
  retried network calls. Receipts are scoped by operation kind and repository,
  bind to a semantic request fingerprint, persist in ObjectDb, expire after 24
  hours, and reconcile against JJ operation metadata after a crash.
- An optional **base revision** must identify a retained commit in the target
  repository. It records causal provenance; stale bases are valid and never act
  as branch-head compare-and-swap gates.
- Every mutable bookmark is resolved once to an immutable planning commit before
  parsing or mutation. Publication uses that same commit, so a racing writer is
  merged by JJ rather than accidentally becoming a newly resolved overwrite
  base.
- A concurrent edit to the **same** declaration produces a first-class conflict
  on an unprotected bookmark. A protected bookmark instead rejects the exact
  conflicted final tree before JJ publication; force cannot bypass this policy.
- A concurrent edit to a **different** declaration merges automatically (different objects in the tree).
- The **change ID** is the durable identity of an edit, so "did my mutation land, and where is it now" is answerable even after the branch advanced or history was rewritten.
- The operation log makes every mutation reversible (`undo`).

**Transactions (required):**

Many real edits touch multiple elements at once. These must commit atomically or not at all.

- A transaction RPC accepts an ordered list of mutation operations and applies them in a single commit.
- Compatibility and reference-integrity checks run on the *final* state, not after each step.
- The whole transaction succeeds and produces one commit, or fails and produces none.
- A transaction accepts at most 100 operations touching at most 20 schemas.
  The server independently enforces a 30-second execution deadline and checks
  cooperative cancellation again before atomic publication. Clients may use a
  shorter RPC deadline.

The exact operation granularity, field-slot constraints, conflict inspection,
idempotency bounds, and current cross-repository behavior are resolved in
`format-capabilities.md`, `grpc-api.md`, `idempotency.md`, and `design.md`.
Remaining 1.0 safety work is tracked in `tasks.md`, not left as an implicit
design expansion.

### 6. Compatibility

- Server enforces backwards-compatibility rules per format when publishing to a protected bookmark.
- `--force` flag bypasses the check (recorded in commit metadata, requires elevated role).

### 7. Project / Repo Management

- Resources are namespaced as `project / repo / schema`.
- Per-project ACLs and visibility (public / private).
- RPCs to create, list, update, archive projects and repos.
- The shipped role model is project-scoped Reader, Writer, Maintainer, and
  Owner. Force requires Maintainer; project/repository management and the
  last-Owner invariant are defined in `resources-and-policy.md`.

### 8. Auth

- **AuthN:** trait-based and configurable. Noop and static-token
  implementations support evaluation/development. Production supports
  externally issued JWTs with explicit issuer, audience, asymmetric algorithm,
  type, identity namespace, bounded HTTPS/file JWKS rotation, and fail-closed
  key freshness.
- **AuthZ:** required for project management. Trait-based, with a default implementation that enforces project-scoped roles.
- Missing credentials may remain anonymous for public reads; presented invalid
  credentials must never silently degrade to anonymous.
- An explicitly configured production policy file must fail startup when it is
  missing, unreadable, or malformed; it must never degrade to noop auth.
- Human, agent, and service principals share authorization rules while retaining
  server-derived kind and delegation in audit records.

### 9. Change Records and Schema Serving

- A `ChangeRecord` is a durable resource separate from a commit. It records the
  target and base revision, human-readable intent, ordered typed mutations or
  replacement source, authenticated actor, validation and compatibility
  results, optional review, lifecycle state, and applied commit/change IDs.
- Humans, agents, and services use the same public API and repository policy.
  Actor kind is explicit, but it does not grant additional authority. The
  server derives audit identity from authentication rather than trusting a
  client-supplied author.
- A draft may initially contain only a note. It must contain an executable and
  validated change before application.
- Applying a record is idempotent and atomic: SchemaHub must not expose an
  applied record without its schema commit, or an applied schema commit without
  the linked record.
- Applied records and schema revisions are immutable. Mutable bookmarks may be
  resolved to a revision, but serving requests identify the resolved commit and
  a deterministic content digest.
- The serving API returns canonical source, native descriptor closures, and
  supported generated source together with format, dependency, digest, and
  resolved-commit metadata.
- Before the first successful response for a canonical artifact request,
  SchemaHub atomically persists its exact bytes and verified metadata. Later
  requests return that first materialization across restart and renderer
  upgrades. Every output-affecting option participates in versioned request
  identity, and corrupt stored records fail closed rather than rerendering.
- SchemaHub stores and serves schema metadata and artifacts. It does not store
  the application data encoded with those schemas.

## Non-Functional Requirements

- **Language:** Rust.
- **Compilers:** reuse `protobuf-rs` and `flatbuffers-rs` as libraries; OpenAPI in-tree.
- **VCS:** `jj-lib`, with custom backends persisting to a database.
- **gRPC framework:** `tonic` (avoid circular dependency with `pure-grpc-rs`, a sibling project).
- **Storage:** trait-abstracted via `ObjectDb`; embedded `redb` and PostgreSQL 17
  are both shipped and tested backends.
- **Auth:** trait-abstracted; noop/static development modes and an in-tree
  production JWT/JWKS resource-server integration ship together.
- **HTTP boundary:** same-origin by default; cross-origin browser access uses an
  exact canonical origin allowlist without cookies, and request bodies are
  subject to a validated finite limit before mutation handlers run.
- **HTTP contract:** generate OpenAPI 3.1 from the registered HTTP handlers,
  expose it from the running server and a no-startup binary command, and place
  the release-versioned document in native archives.
- **API boundary:** `schemahub.v1` gRPC/protobuf is the public 1.0 API. The
  unversioned `/api/*` routes are GUI-only BFF projections outside the public
  compatibility promise and must identify that classification in responses
  and OpenAPI. Operational probes remain a separately supported interface.
- **Future REST resource design:** a future public REST surface must be
  explicitly versioned, follow the accepted resource/method shapes, publish a
  separately identified contract, and receive an explicit compatibility
  declaration rather than inheriting `/api/*`.
- **Reverse-discovery bounds:** one call scans at most 1,000 visible repositories
  and 10,000 schemas. The server runs synchronous storage/compiler work on its
  blocking executor, filters candidates through Core Read authorization, and
  fails the entire call when a limit or read/decoding error occurs.
- **Public:** open-source.

## Remaining 1.0 Safety Gate

Resolved architecture and public behavior live in the focused documents listed
by `MANIFEST.md`. The remaining gates are explicit:

Publish and pin the coordinated sibling FlatBuffers compiler revision so a
clean independent checkout has no cross-repository path dependency. The 1.0
reverse-dependency decision is now frozen as the bounded, direct,
snapshot-manifest `ListDependents` contract in `dependency-discovery.md`.
