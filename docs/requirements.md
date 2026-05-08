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

### 4. Schema Exploration API

- Tree-walking RPCs designed for agents: resolve a message, list fields, follow a field's type, list dependencies, fetch a single node by path. Lets an agent traverse a large schema incrementally.
- Searchable by name, type, path, and project.

### 5. Compatibility

- Server enforces backwards-compatibility rules per format on push.
- `--force` flag bypasses the check (recorded in commit metadata).

### 6. Project / Repo Management

- Resources are namespaced as `project / repo / schema`.
- Per-project ACLs and visibility (public / private).
- RPCs to create, list, update, archive projects and repos.
- Detailed permission model deferred to detailed design.

### 7. Auth

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
