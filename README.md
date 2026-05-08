# schemahub

A general-purpose schema registry server. Manage Protobuf, FlatBuffers, and OpenAPI schemas via gRPC and CLI — no more checked-in `.proto` / `.fbs` files as the source of truth.

Schemas are stored as parsed, structured representations in a versioned database with git-style semantics (commits, branches, tags, history, diff). Both humans and AI agents are first-class clients: agents can traverse the AST node-by-node without loading entire schemas into context.

**Status:** requirements only. Detailed design pending.

See [`docs/requirements.md`](docs/requirements.md).

## Highlights

- Protobuf, FlatBuffers, OpenAPI in v1 (DB schemas postponed)
- Pluggable storage backend (BoltDB-style KV or git-backed — TBD by design)
- Pluggable auth (no-op default; authz required for project management)
- gRPC server (`tonic`) + CLI client
- Local codegen via CLI; server-side codegen preview RPC
- Compatibility checks with `--force` override
- Project / repo / schema namespacing with ACLs and visibility

## Stack

- Rust
- `tonic` for gRPC
- Reuses `protobuf-rs`, `flatbuffers-rs`, and `codegen-infra` (refactor as needed)
