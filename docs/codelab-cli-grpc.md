<!-- agent-updated: 2026-07-30T04:16:42Z -->

# Codelab: Build and Audit the SchemaHub CLI over gRPC

This codelab shows how the `schemahub` CLI works as a thin gRPC client and how to verify the full path end to end:

1. Build `schemahub-server` and the `schemahub` CLI.
2. Start a real gRPC server backed by redb.
3. Use the CLI to record change intent, then create schemas, branches, tags,
   mutations, merges, diffs, logs, and generated code.
4. Resolve an immutable schema revision, fetch and verify its artifact, and
   compile the generated Rust code locally.
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
| `schemahub project create/get/list/set-visibility/archive/member/audit ...` | `ProjectServiceClient` |
| `schemahub change note/add-source/validate/ready/review/apply/abandon` | `ChangeServiceClient` |
| `schemahub schema create/update/delete` | `SchemaServiceClient` |
| `schemahub schema pull/dependents` | `ExplorationServiceClient` |
| `schemahub branch ...`, `tag ...`, `diff`, `log` | `RefServiceClient` |
| `schemahub op log`, `undo`, `resolve` | `HistoryServiceClient` |
| `schemahub codegen get/preview` | `CodegenServiceClient` |
| `schemahub artifact resolve/fetch/verify` | `ServingServiceClient` |
| `schemahub capabilities [--json]` | `AdminServiceClient` |

The important audit point: the CLI does not call `schemahub-core` directly. It only talks to the server through the generated gRPC API in `schemahub-api`.

## 2. Build the binaries

From the repo root:

```bash
export SCHEMAHUB_REPO="$(pwd)"
cargo build --locked --release -p schemahub-server -p schemahub-cli
```

Expected binaries:

```bash
"${SCHEMAHUB_REPO}/target/release/schemahub-server" --help
"${SCHEMAHUB_REPO}/target/release/schemahub" --help
```

## 3. Start the gRPC server

Bind to the Tailscale interface and address it through the full MagicDNS name:

```bash
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(
  tailscale status --json \
    | jq -r '.Self.DNSName' \
    | sed 's/\.$//'
)"

export SCHEMAHUB_PORT=50051
export SCHEMAHUB_SERVER="http://${TAILSCALE_HOST}:${SCHEMAHUB_PORT}"
export SCHEMAHUB_TMP="$(mktemp -d)"
export SCHEMAHUB_DB="${SCHEMAHUB_TMP}/schemahub.redb"
export SCHEMAHUB_CONFIG="${SCHEMAHUB_TMP}/schemahub.toml"
export SCHEMAHUB_TOKEN="codelab-owner-token"

cat > "${SCHEMAHUB_CONFIG}" <<'EOF'
[auth.tokens.codelab-owner-token]
id = "codelab-owner"
display = "Codelab Owner"
kind = "human"
EOF

"${SCHEMAHUB_REPO}/target/release/schemahub-server" \
  --listen "${TAILSCALE_IP}:${SCHEMAHUB_PORT}" \
  --db "${SCHEMAHUB_DB}" \
  --config "${SCHEMAHUB_CONFIG}"
```

Keep that process running. In another shell, define a helper:

```bash
export SCHEMAHUB_REPO="/home/cyuan/projects/shuozeli/codegen/schemahub"
export TAILSCALE_HOST="$(
  tailscale status --json \
    | jq -r '.Self.DNSName' \
    | sed 's/\.$//'
)"
export SCHEMAHUB_PORT=50051
export SCHEMAHUB_SERVER="http://${TAILSCALE_HOST}:${SCHEMAHUB_PORT}"
export SCHEMAHUB_TOKEN="codelab-owner-token"

schemahub_cli() {
  "${SCHEMAHUB_REPO}/target/release/schemahub" \
    --server "${SCHEMAHUB_SERVER}" \
    --token "${SCHEMAHUB_TOKEN}" "$@"
}
```

Before relying on an operation, inspect the executable contract served by this
binary. The human form is convenient for discovery; the JSON form is stable for
agents and CI feature negotiation:

```bash
schemahub_cli capabilities
schemahub_cli capabilities --json | jq '{matrix_version, formats}'
```

The result reports format-level parse/print, compatibility, conflict,
descriptor, and codegen support plus direct/transaction reachability for each
mutation. It is authoritative over examples in this codelab.

For non-interactive callers, add `--json` to ChangeRecord, artifact, and
capability commands and add `--json-errors` globally. Failures then emit one
JSON object on stderr with `exit_code`, stable `kind`, optional `grpc_code`, and
the causal message:

```bash
schemahub_cli --json-errors change get \
  projects/acme/repos/commerce/changes/does-not-exist --json \
  2>error.json || status=$?
jq . error.json
```

Stable process codes are `0` success, `1` local error, `2` invalid argument,
`10` unauthenticated, `11` permission denied, `12` not found, `13` already
exists, `14` state/precondition conflict, `20` transient transport failure,
`21` resource exhaustion, and `22` server or unimplemented failure. Clap syntax
errors also use `2`.

Project creation requires authentication. This codelab uses one static human
identity; the server derives the ChangeRecord actor and initial project Owner
from that token.

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
- It atomically creates project `acme` plus its initial Owner, or reuses it.
- It creates or reuses persisted repo `commerce`.
- Writes still go through the server, not through a local database.

Inspect the durable resources and their ETags:

```bash
schemahub_cli project get acme
schemahub_cli project list --prefix ac --page-size 10
schemahub_cli project audit acme
schemahub_cli project audit acme --json |
  jq 'map({action, actor, resource_name, before, after})'
```

Project/repository resources, memberships, ChangeRecords, and JJ state all use
the selected redb database. They occupy separate record/object namespaces.
Project/member/repository mutations and their typed administrative events
commit atomically; only Owners can read that audit stream.

Before changing a schema, record the intent. Humans get readable output; agents
and CI can request a stable JSON resource:

```bash
schemahub_cli change note acme/commerce \
  --title "Add currency to orders" \
  --description "Consumers need the settlement currency" \
  --reference COMMERCE-2048 \
  --reference https://tracker.example.test/issues/2048 \
  --id add-order-currency

schemahub_cli change get \
  projects/acme/repos/commerce/changes/add-order-currency \
  --json
```

Attach an executable edit, then use the ETag returned by each command for the
next lifecycle transition:

```bash
schemahub_cli change add-source \
  projects/acme/repos/commerce/changes/add-order-currency \
  --etag '<current-etag>' \
  --schema-path order.proto \
  --file tests/integration/complex_order.proto

schemahub_cli change validate \
  projects/acme/repos/commerce/changes/add-order-currency \
  --etag '<updated-etag>' --json

schemahub_cli change ready \
  projects/acme/repos/commerce/changes/add-order-currency \
  --etag '<validated-etag>'

schemahub_cli change apply \
  projects/acme/repos/commerce/changes/add-order-currency \
  --etag '<ready-etag>' \
  --request-id codelab-add-order-currency --json
```

Audit points:

- `ChangeService` derives actor kind and identity from the bearer token.
- The note is persisted in the same selected redb/PostgreSQL deployment as JJ,
  but outside JJ's immutable object namespace.
- Draft edits and every lifecycle transition use the returned ETag.
- Validation findings are stored data. Apply uses a stable request ID and
  returns the same JJ commit/operation receipt when retried.
- Abandonment retains the record.

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

Ask the same public service which visible schemas directly import
`common.proto` and retain the immutable scan evidence:

```bash
schemahub_cli schema dependents acme/commerce/common.proto --json \
  | jq '{schemasScanned, snapshots, dependents}'
```

The result includes `order.proto`, its exact importing commit, and
`pinned: false` for this same-repository live import. Each snapshot identifies
the configured default bookmark and immutable commit inspected. There is no
global cross-repository instant or automatic rewrite; coordinated releases use
the manifest to create explicit downstream ChangeRecords. See
`dependency-discovery.md`.

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

## 12. Pin, fetch, and verify an immutable artifact

Resolve `main` once and retain the full immutable resource name:

```bash
export REVISION="$(
  schemahub_cli artifact resolve acme/commerce --at main --json \
    | jq -r '.name'
)"
```

Fetch generated code through the serving plane and retain its digest:

```bash
schemahub_cli artifact fetch "${REVISION}" \
  --schema-path order.proto \
  --kind generated-code \
  --language rust \
  --output immutable-generated.rs \
  --json \
  | tee artifact.json

export ARTIFACT_DIGEST="$(jq -r '.artifact_digest' artifact.json)"
```

Verify a fresh download locally against that persisted digest:

```bash
schemahub_cli artifact verify "${REVISION}" \
  --schema-path order.proto \
  --kind generated-code \
  --language rust \
  --digest "${ARTIFACT_DIGEST}"
```

Moving `main` later does not change bytes fetched through `${REVISION}`.

## 13. Run the automated Docker e2e

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

## 14. Auditor checklist

Use this checklist when reviewing the CLI:

| Check | Evidence |
|---|---|
| CLI is a gRPC client, not an in-process core caller | `crates/schemahub-cli/src/client.rs`, `crates/schemahub-cli/src/main.rs` |
| Requests use generated protobuf types | imports from `schemahub_api::schemahub_v1::*` |
| Auth metadata is consistently attached | `crates/schemahub-cli/src/cmd/mod.rs::bearer` |
| Schema writes go through `SchemaServiceClient` | `crates/schemahub-cli/src/cmd/schema.rs` |
| Codegen preview goes through `CodegenServiceClient::preview_codegen` | `crates/schemahub-cli/src/cmd/codegen.rs` |
| Immutable resolve/fetch/verify goes through `ServingServiceClient` | `crates/schemahub-cli/src/cmd/artifact.rs` |
| Agent errors have stable JSON and process classifications | `classify_error` / `classify_grpc_code` in `crates/schemahub-cli/src/main.rs` |
| Version operations go through `RefServiceClient` / `HistoryServiceClient` | `branch`, `tag`, `log`, `history` command modules |
| End-to-end proof exists in Docker | `tests/docker/run_e2e.sh` |
