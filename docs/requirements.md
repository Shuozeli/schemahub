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
- **Jujutsu-style evolution of schemas:** stable change identities that survive rewrite/rebase, an operation log with undo, and first-class conflicts so concurrent edits never hard-fail.
- Agent-friendly: programmatic AST traversal that lets an agent explore a schema without loading the full text into context, and granular edits that an agent can retry safely.
- Pluggable storage and pluggable auth — adopters configure their own.
- Designed so DB-schema management can be added later without re-architecting.

## Non-Goals (v1)

- Database schema management (DDL / table-and-column registry). Future format; the AST and storage abstractions must not preclude it.
- A web UI. CLI + gRPC only in v1.
- Hosting or running generated code. The server can render a *preview* of generated code, but does not produce build artifacts.
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
- **Operation log + undo.** Every operation that changes repository state (mutation, bookmark move, GC, role change) is recorded in an operation log. Any operation can be undone / the repo restored to a prior operation. This is the registry's audit and recovery story.
- **Bookmarks** (named pointers to commits) stand in for branches. History, diff, and merge are expressed over the commit graph.
- **Database-backed.** All objects (commits, trees, declaration blobs, conflicts) and the operation log are persisted to a database via custom `jj-lib` backends. schemahub does **not** use jj's on-disk git/file working-copy layout.
- Compatibility checks run when publishing to a protected bookmark; `--force` (elevated role) can override.

### 3. Code Generation

- **Local codegen via CLI:** client pulls descriptors and generates code on the user's machine. **Reuses the sibling compilers' codegen** (`protobuf-rs` Rust/`FileDescriptorSet` output, `flatbuffers-rs` Rust/TS/Dart output) and `codegen-infra`.
- **Server-side preview:** an RPC that renders generated code on demand for inspection. No files written; response is the rendered text.

### 4. Schema Exploration API (Read)

- Tree-walking RPCs designed for agents: resolve a message, list fields, follow a field's type, list dependencies, fetch a single node by path. Per-declaration storage makes each of these a direct object lookup. Lets an agent traverse a large schema incrementally.
- Searchable by name, type, path, and project.

### 5. Schema Mutation API (CRUD)

The write counterpart to the Exploration API. Granular RPCs that mutate individual schema elements — not text patches, not whole-file replacements. Designed so an agent or CLI can edit a schema the same way it edits source code: one targeted change at a time.

**Required operations (Protobuf and FlatBuffers, v1):**

- **Messages / tables:** create, rename, delete, move between namespaces.
- **Fields:** add, remove, rename, reorder, change type, change cardinality (optional / required / repeated), edit options/attributes, edit comments.
- **Enums:** create / delete / rename; add / remove / rename values; edit comments.
- **Namespaces / packages:** create, rename, delete (with cascade rules TBD).
- **Services / RPCs (Protobuf):** create, rename, delete; add / remove / rename methods; change request / response types.
- **Imports / dependencies:** add, remove, update target version.

> Note: because the AST is now the real compiler AST, every operation above has a concrete, correct place to live (e.g. proto3 `optional`/presence, structured `oneof`, nested types, field options). The cardinality and options operations in particular were impossible against the previous hand-rolled AST.

**Semantics:**

- Each mutation is a typed operation, validated against the AST by the format compiler.
- Each mutation (or transaction) produces a new commit / advances a change.
- Compatibility checks from Section 6 apply when publishing to a protected bookmark; `--force` override available.
- Reference integrity is enforced: renames must propagate to all referencing fields / RPCs / imports atomically. A mutation that would leave a dangling reference is rejected unless `--force` is given. Where propagation spans commits that have descendants, the VCS's auto-rebase carries the change forward; unresolved cases surface as conflicts rather than silent breakage.
- OpenAPI mutation surface is deferred to detailed design — the shape differs enough that it warrants its own RPC set.

**Concurrency model (Jujutsu-style, replaces the original idempotency/CAS framing):**

The original requirement was at-most-once mutation via client idempotency keys + base-revision CAS that *rejects* on conflict. Under the jj model the durable guarantees come from change IDs and the operation log instead:

- A client-supplied **idempotency key** is still honored at the RPC edge to dedupe retried network calls (returns the original result).
- A concurrent edit to the **same** declaration does **not** reject; it produces a first-class conflict that the loser (or a later resolver) reconciles.
- A concurrent edit to a **different** declaration merges automatically (different objects in the tree).
- The **change ID** is the durable identity of an edit, so "did my mutation land, and where is it now" is answerable even after the branch advanced or history was rewritten.
- The operation log makes every mutation reversible (`undo`).

**Transactions (required):**

Many real edits touch multiple elements at once. These must commit atomically or not at all.

- A transaction RPC accepts an ordered list of mutation operations and applies them in a single commit.
- Compatibility and reference-integrity checks run on the *final* state, not after each step.
- The whole transaction succeeds and produces one commit, or fails and produces none.

**Flagged areas for the design owner:**

- Operation granularity: which mutations are first-class RPCs vs. compositions of smaller ops.
- Field-number reuse / reservation (Protobuf) and slot-ordering constraints (FlatBuffers).
- Rename propagation across imports and repos, and how it interacts with auto-rebase.
- Conflict **resolution** UX: how a client inspects and resolves a first-class conflict (per-declaration).
- Idempotency-key TTL and scope at the RPC edge.
- Transaction size limits and timeout behavior.

### 6. Compatibility

- Server enforces backwards-compatibility rules per format when publishing to a protected bookmark.
- `--force` flag bypasses the check (recorded in commit metadata, requires elevated role).

### 7. Project / Repo Management

- Resources are namespaced as `project / repo / schema`.
- Per-project ACLs and visibility (public / private).
- RPCs to create, list, update, archive projects and repos.
- Detailed permission model deferred to detailed design.

### 8. Auth

- **AuthN:** trait-based, configurable. A no-op implementation ships in-tree for getting started.
- **AuthZ:** required for project management. Trait-based, with a default implementation that enforces project-scoped roles.

## Non-Functional Requirements

- **Language:** Rust.
- **Compilers:** reuse `protobuf-rs` and `flatbuffers-rs` as libraries; OpenAPI in-tree.
- **VCS:** `jj-lib`, with custom backends persisting to a database.
- **gRPC framework:** `tonic` (avoid circular dependency with `pure-grpc-rs`, a sibling project).
- **Storage:** trait-abstracted via jj-lib's backend traits; backend choice (embedded `redb` vs. server `postgres`) deferred to design.
- **Auth:** trait-abstracted; no-op shipped, custom implementations pluggable.
- **Public:** open-source.

## Open Questions for Detailed Design

1. Database choice behind the jj-lib backends (embedded `redb` vs. `postgres`) and the schema for objects + operation log.
2. Exact printer design per format (round-trip fidelity from the compiler AST + source-location data).
3. Compatibility-check rule set per format.
4. ACL / role model granularity.
5. Conflict **representation and resolution** at the per-declaration level, per format (how a conflicted message/table/enum is stored and resolved).
6. CLI UX shape — including jj-flavored commands (`log`, `op log`, `undo`, bookmarks).
7. Mutation API operation set (first-class vs. composed) and reference-integrity model — see Section 5.
8. OpenAPI mutation surface (deferred from v1 mutation API).
9. Idempotency-key handling at the RPC edge (TTL, scope) now that durability comes from change IDs + op-log — see Section 5.
10. Transaction size and timeout limits — see Section 5.
11. How much of `jj-lib` we depend on (backend + op-store + repo/commit APIs) vs. bypass (filesystem working copy, git interop).
