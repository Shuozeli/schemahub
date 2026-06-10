<!-- agent-updated: 2026-06-06T18:23:30Z -->

# Codelab: Build and Audit the SchemaHub CLI over gRPC

This codelab shows how the `schemahub` CLI works as a thin gRPC client and how to verify the full path end to end:

1. Build `schemahub-server` and the `schemahub` CLI.
2. Start a real gRPC server backed by redb.
3. Use the CLI to create schemas, branches, tags, mutations, merges, diffs, logs, and generated code.
4. Compile the generated Rust code locally.
5. Run the Docker e2e that automates the same workflow in a clean container.

## 1. What the CLI wraps

The CLI binary is `schemahub-cli`, exposed as `schemahub`.

The top-level entry point parses global flags, builds a `tonic::transport::Channel`, and dispatches each subcommand to a generated gRPC client:

```rust
// crates/schemahub-cli/src/main.rs
let ch = client::build_channel(&cfg.server).await?;
schema::run(args, ch, &cfg.token).await
```

The channel is a normal tonic channel:

```rust
// crates/schemahub-cli/src/client.rs
Channel::from_shared(server.to_string())?
    .connect()
    .await?
```

Every command wraps its protobuf request in `tonic::Request` and attaches `Authorization: Bearer <token>` when a token is present:

```rust
// crates/schemahub-cli/src/cmd/mod.rs
let mut req = Request::new(body);
req.metadata_mut().insert("authorization", header);
```

Examples of command-to-service mapping:

| CLI command | gRPC service |
|---|---|
| `schemahub repo init` | `ProjectServiceClient` |
| `schemahub schema create/update/delete` | `SchemaServiceClient` |
| `schemahub schema pull` | `ExplorationServiceClient` |
| `schemahub branch ...`, `tag ...`, `diff`, `log` | `RefServiceClient` |
| `schemahub op log`, `undo`, `resolve` | `HistoryServiceClient` |
| `schemahub codegen get/preview` | `CodegenServiceClient` |

The important audit point: the CLI does not call `schemahub-core` directly. It only talks to the server through the generated gRPC API in `schemahub-api`.

## 2. Build the binaries

From the repo root:

```bash
export SCHEMAHUB_REPO="$(pwd)"
cargo build --locked -p schemahub-server -p schemahub-cli
```

Expected binaries:

```bash
"${SCHEMAHUB_REPO}/target/debug/schemahub-server" --help
"${SCHEMAHUB_REPO}/target/debug/schemahub" --help
```

## 3. Start the gRPC server

Prefer binding to the Tailscale interface when available:

```bash
export TAILSCALE_IP="$(tailscale ip -4 2>/dev/null || true)"
export TAILSCALE_HOST="$(
  tailscale status --json 2>/dev/null \
    | jq -r '.Self.DNSName // empty' \
    | sed 's/\.$//'
)"

export SCHEMAHUB_BIND="${TAILSCALE_IP:-0.0.0.0}"
export SCHEMAHUB_HOST="${TAILSCALE_HOST:-127.0.0.1}"
export SCHEMAHUB_PORT=50051
export SCHEMAHUB_SERVER="http://${SCHEMAHUB_HOST}:${SCHEMAHUB_PORT}"
export SCHEMAHUB_DB="$(mktemp -u /tmp/schemahub-codelab.XXXXXX.redb)"

"${SCHEMAHUB_REPO}/target/debug/schemahub-server" \
  --listen "${SCHEMAHUB_BIND}:${SCHEMAHUB_PORT}" \
  --db "${SCHEMAHUB_DB}"
```

Keep that process running. In another shell, define a helper:

```bash
export SCHEMAHUB_REPO="/home/cyuan/projects/shuozeli/codegen/schemahub"
export TAILSCALE_HOST="$(
  tailscale status --json 2>/dev/null \
    | jq -r '.Self.DNSName // empty' \
    | sed 's/\.$//'
)"
export SCHEMAHUB_HOST="${TAILSCALE_HOST:-127.0.0.1}"
export SCHEMAHUB_PORT=50051
export SCHEMAHUB_SERVER="http://${SCHEMAHUB_HOST}:${SCHEMAHUB_PORT}"

schemahub_cli() {
  "${SCHEMAHUB_REPO}/target/debug/schemahub" --server "${SCHEMAHUB_SERVER}" "$@"
}
```

If auth is not configured, the server runs in the default no-auth mode and the CLI sends anonymous requests.

## 4. Create local schema inputs

Use a temporary workspace:

```bash
export LAB_DIR="$(mktemp -d /tmp/schemahub-cli-grpc.XXXXXX)"
cd "${LAB_DIR}"
```

Create a shared Protobuf file:

```bash
cat > common.proto <<'EOF'
syntax = "proto3";
package commerce.v1;

message Money {
  string currency_code = 1;
  int64 units = 2;
  int32 nanos = 3;
}
EOF
```

Create an importing Protobuf file:

```bash
cat > order.proto <<'EOF'
syntax = "proto3";
package commerce.v1;

import "acme/commerce/common.proto";

message Order {
  string id = 1;
  Money total = 2;
}
EOF
```

## 5. Initialize the project and repo

```bash
schemahub_cli repo init acme/commerce --public
```

Audit points:

- `repo init` uses `ProjectServiceClient`.
- It creates or reuses project `acme`.
- It creates repo `commerce`.
- Writes still go through the server, not through a local database.

## 6. Push schemas through gRPC

```bash
schemahub_cli schema create common.proto \
  --project acme \
  --repo commerce \
  --name common.proto

schemahub_cli schema create order.proto \
  --project acme \
  --repo commerce \
  --name order.proto
```

Expected output includes `Created commit: ...` for each schema.

Now pull the schema back:

```bash
schemahub_cli schema pull acme/commerce/order.proto --branch main
```

Expected source contains:

```proto
message Order {
  string id = 1;
  Money total = 2;
}
```

## 7. Tag the release and create a feature branch

```bash
schemahub_cli tag create acme/commerce release-2026-06-06 --branch main
schemahub_cli branch create acme/commerce feature/shipping-note --from main
```

Audit points:

- Tags are immutable release snapshots.
- Branches are JJ bookmarks exposed through the gRPC `RefService`.

## 8. Mutate the schema on the branch

Add a field through a granular Protobuf mutation:

```bash
schemahub_cli field add \
  acme/commerce/order.proto \
  Order \
  shipping_note:string:3 \
  --branch feature/shipping-note
```

Verify branch isolation:

```bash
schemahub_cli schema pull acme/commerce/order.proto --branch main \
  | tee main-before.proto

schemahub_cli schema pull acme/commerce/order.proto --branch feature/shipping-note \
  | tee feature-after.proto

! grep -q shipping_note main-before.proto
grep -q shipping_note feature-after.proto
```

Expected: `main` does not contain `shipping_note`; the feature branch does.

## 9. Merge and verify the tag stayed pinned

```bash
schemahub_cli branch merge \
  acme/commerce \
  feature/shipping-note \
  --into main \
  --message "merge shipping note"
```

Pull `main` and the release tag:

```bash
schemahub_cli schema pull acme/commerce/order.proto --branch main \
  | tee main-after.proto

schemahub_cli schema pull acme/commerce/order.proto --branch tag:release-2026-06-06 \
  | tee release-after.proto

grep -q shipping_note main-after.proto
! grep -q shipping_note release-after.proto
```

Expected: `main` contains the field after merge; the tag does not.

## 10. Inspect diff, commit log, and operation log

```bash
schemahub_cli diff \
  acme/commerce \
  tag:release-2026-06-06..main \
  --schema-path order.proto
```

Expected output includes:

```text
schema: order.proto
  modified Order
```

Inspect commit history:

```bash
schemahub_cli log acme/commerce --branch main --limit 5
```

Expected output includes `commit`.

Inspect JJ-style operation history:

```bash
schemahub_cli op log acme/commerce --limit 10
```

Expected output includes `op`.

Audit point: commit log answers "what content state exists"; operation log answers "what registry operation changed the view".

## 11. Generate Rust code through gRPC and compile it

Preview generated Rust:

```bash
schemahub_cli codegen preview \
  acme/commerce/order.proto \
  --branch main \
  --lang rust \
  > generated.rs

grep -q shipping_note generated.rs
```

Create a tiny generated-code crate:

```bash
cat > Cargo.toml <<'EOF'
[package]
name = "schemahub-codelab-generated-check"
version = "0.0.0"
edition = "2021"

[dependencies]
prost = "0.13"
EOF

mkdir -p src
cat > src/lib.rs <<'EOF'
#![allow(warnings)]
include!("../generated.rs");
EOF
```

Compile it:

```bash
CARGO_TARGET_DIR=/tmp/schemahub-codelab-generated-target cargo check --quiet
```

Expected: no compiler errors.

## 12. Run the automated Docker e2e

The codelab above is also automated in Docker:

```bash
cd "${SCHEMAHUB_REPO}"
tests/docker/run_e2e.sh
```

That script builds the CLI and server inside Docker, starts a server container, drives the CLI over gRPC, verifies branch/tag/diff/log/codegen behavior, and compiles the generated Rust artifact in the container.

Expected final line:

```text
Docker e2e passed.
```

## 13. Auditor checklist

Use this checklist when reviewing the CLI:

| Check | Evidence |
|---|---|
| CLI is a gRPC client, not an in-process core caller | `crates/schemahub-cli/src/client.rs`, `crates/schemahub-cli/src/main.rs` |
| Requests use generated protobuf types | imports from `schemahub_api::schemahub_v1::*` |
| Auth metadata is consistently attached | `crates/schemahub-cli/src/cmd/mod.rs::bearer` |
| Schema writes go through `SchemaServiceClient` | `crates/schemahub-cli/src/cmd/schema.rs` |
| Codegen preview goes through `CodegenServiceClient::preview_codegen` | `crates/schemahub-cli/src/cmd/codegen.rs` |
| Version operations go through `RefServiceClient` / `HistoryServiceClient` | `branch`, `tag`, `log`, `history` command modules |
| End-to-end proof exists in Docker | `tests/docker/run_e2e.sh` |
