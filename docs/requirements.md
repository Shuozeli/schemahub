# schemahub — Requirements

> Requirements only. Detailed design is owned by a separate engineer; this document defines *what* schemahub must do, not *how*.

## Overview

A general-purpose schema registry server that decouples schema definitions from filesystem files. Schemas (Protobuf, FlatBuffers, OpenAPI) are stored as parsed, structured representations in a versioned database and managed via a gRPC API and CLI. Both humans and AI agents are first-class clients.

## Goals

- Single source of truth for schemas across an organization, accessed by gRPC instead of file paths.
- Git-style evolution of schemas: commits, branches, tags, history, diff, merge.
- Agent-friendly: programmatic AST traversal that lets an agent explore a schema without loading the full text into context.
- Pluggable storage and pluggable auth — adopters configure their own.
- Designed so DB-schema management can be added later without re-architecting.

## Non-Goals (v1)

- Database schema management (DDL / table-and-column registry). Future format; the AST and storage abstractions must not preclude it.
- A web UI. CLI + gRPC only in v1.
- Hosting or running generated code. The server can render a *preview* of generated code, but does not produce build artifacts.

## Functional Requirements

### 1. Schema Storage

- **Formats supported in v1:** Protobuf (`.proto`), FlatBuffers (`.fbs`), OpenAPI / Swagger (JSON / YAML).
- **Stored representation:** parsed AST + descriptors + dependency graph. Raw source files are **not** stored.
- **Deterministic round-trip:** the AST must capture enough information (declaration order, comments, options, formatting hints) that the canonical source text can be reconstructed deterministically from the AST. Custom AST schemas owned by this project — not the upstream `protoc` / `flatc` descriptors — to ensure ordering and fidelity.

### 2. Versioning

- Git-style semantics: commits, branches, tags, history, diff, merge.
- Storage backend is a trait. Choice between a KV-style backend (BoltDB-shaped, e.g. `jammdb` / `redb`) versus a real-git-backed backend is deferred to detailed design.
- Compatibility checks run on push; user can `--force` to override.

### 3. Code Generation

- **Local codegen via CLI:** client pulls descriptors and generates code on the user's machine. Reuses `codegen-infra`, `protobuf-rs`, and `flatbuffers-rs` (refactor as needed).
- **Server-side preview:** an RPC that renders generated code on demand for inspection. No files written; response is the rendered text.

### 4. Schema Exploration API (Read)

- Tree-walking RPCs designed for agents: resolve a message, list fields, follow a field's type, list dependencies, fetch a single node by path. Lets an agent traverse a large schema incrementally.
- Searchable by name, type, path, and project.

### 5. Schema Mutation API (CRUD)

The write counterpart to the Exploration API. Granular RPCs that mutate individual schema elements — not text patches, not whole-file replacements. Designed so an agent or CLI can edit a schema the same way it edits source code: one targeted change at a time.

**Required operations (Protobuf and FlatBuffers, v1):**

- **Messages / tables:** create, rename, delete, move between namespaces.
- **Fields:** add, remove, rename, reorder, change type, change cardinality (optional / required / repeated), edit options, edit comments.
- **Enums:** create / delete / rename; add / remove / rename values; edit comments.
- **Namespaces / packages:** create, rename, delete (with cascade rules TBD).
- **Services / RPCs (Protobuf):** create, rename, delete; add / remove / rename methods; change request / response types.
- **Imports / dependencies:** add, remove, update target version.

**Semantics:**

- Each mutation is a typed operation, validated server-side against the AST.
- Each mutation (or transaction) produces a new commit on the current branch.
- Compatibility checks from Section 6 apply on every mutation; `--force` override available.
- Reference integrity is enforced: renames must propagate to all referencing fields / RPCs / imports atomically. The server must reject a mutation that would leave a dangling reference unless `--force` is given.
- OpenAPI mutation surface is deferred to detailed design — the shape (paths, operations, components, schemas) differs enough from proto / fbs that it warrants its own RPC set.

**Idempotency (required):**

Mutation RPCs must be idempotent to prevent races when an agent or CLI retries a request, when two clients race on the same element, or when a network failure leaves the client unsure whether a mutation landed.

- Every mutation request carries a client-generated idempotency key (UUID or similar).
- Re-sending the same request with the same key on the same branch is a no-op that returns the original result, even if the original committed successfully or partially.
- Mutations are conditional on a base revision (the commit the client believes is current). If the branch has advanced, the server rejects with a conflict and the client must re-read and retry. This gives compare-and-swap semantics on top of the git-style log.
- Idempotency keys and base-revision checks together ensure at-most-once semantics regardless of retries.

**Transactions (required):**

Many real edits touch multiple elements at once — e.g. add a field to ten messages, rename a type used across a service, restructure a namespace. These must commit atomically or not at all.

- A transaction RPC accepts an ordered list of mutation operations and applies them in a single server-side transaction.
- Compatibility and reference-integrity checks run on the *final* state, not after each step. Intermediate states may be inconsistent (e.g. a rename in progress); only the final state must be valid.
- The whole transaction succeeds and produces one commit, or fails and produces none. No partial application.
- Transactions are also idempotency-keyed and base-revision-conditional.

**This needs careful design.** Flagged areas for the design owner:

- Operation granularity: which mutations are first-class RPCs vs. expressed as compositions of smaller ops.
- Field-number reuse and reservation rules (Protobuf has explicit semantics here; FlatBuffers has different ordering constraints).
- Rename propagation across imports and across other repos in the registry.
- Conflict handling when two clients mutate the same element on the same branch (resolution strategy beyond CAS rejection).
- Idempotency-key TTL and storage (how long the server remembers prior keys).
- Transaction size limits and timeout behavior.

### 6. Compatibility

- Server enforces backwards-compatibility rules per format on push.
- `--force` flag bypasses the check (recorded in commit metadata).

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
- **gRPC framework:** `tonic` (avoid circular dependency with `pure-grpc-rs`, which is itself a sibling project).
- **Storage:** trait-abstracted; backend choice deferred to design.
- **Auth:** trait-abstracted; no-op shipped, custom implementations pluggable.
- **Public:** open-source.

## Open Questions for Detailed Design

1. Storage backend choice (BoltDB-style KV vs. git-backed).
2. Exact AST schema per format (must support deterministic round-trip).
3. Compatibility-check rule set per format.
4. ACL / role model granularity.
5. Conflict resolution on merge (auto vs. manual).
6. CLI UX shape (`schemahub push` / `pull` / `branch` / `log` / `diff` ...).
7. Mutation API operation set (first-class vs. composed) and reference-integrity model — see Section 5.
8. OpenAPI mutation surface (deferred from v1 mutation API).
9. Idempotency-key storage, TTL, and uniqueness scope (per-branch? per-repo?) — see Section 5.
10. Transaction size and timeout limits — see Section 5.
