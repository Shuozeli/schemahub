<!-- agent-updated: 2026-07-22T16:00:04Z -->
# Codelab: From an Agent Proposal to an Immutable Data Schema

This codelab exercises the primary SchemaHub workflow:

1. A delegated agent records why a schema should change.
2. The agent attaches executable schema source and asks SchemaHub to validate it.
3. A human reviews and approves the exact validated change.
4. The agent applies the approved change with an idempotency key.
5. A data producer resolves the resulting immutable revision and downloads a
   descriptor or generated binding whose digest can be stored with application
   data.

SchemaHub stores schema intent, versioned schema state, and immutable artifacts.
It does **not** store the application records encoded with those schemas. The
producer's database, stream, or object store should retain the SchemaHub revision
and artifact digest beside the data so a future reader can retrieve the same
schema bytes.

```text
agent: draft -> attach source -> validate -> ready
                                      |
human:                         review + approve
                                      |
agent:                              apply
                                      |
consumer:              resolve revision -> fetch artifact
```

The complete lab uses static development credentials and embedded redb. For a
production deployment, use externally issued JWTs and PostgreSQL as described in
[Authentication](authentication.md) and
[Deploy a SchemaHub Release on Tailscale](codelab-deploy.md).

## 1. Prerequisites

Run the lab from the SchemaHub repository root. You need:

- Rust and Cargo;
- `jq`;
- `rg` (ripgrep);
- Tailscale connected on the host;
- a shell with `tailscale ip -4` and `tailscale status --json` available.

Build production-profile binaries:

```bash
export SCHEMAHUB_REPO="$(pwd)"
cargo build --locked --release -p schemahub-server -p schemahub-cli

export SCHEMAHUB_SERVER_BIN="${SCHEMAHUB_REPO}/target/release/schemahub-server"
export SCHEMAHUB_CLI_BIN="${SCHEMAHUB_REPO}/target/release/schemahub"
"${SCHEMAHUB_SERVER_BIN}" --version
"${SCHEMAHUB_CLI_BIN}" --version
```

## 2. Configure a human, an agent, and repository policy

Use a temporary redb database and two identities. The agent has `Writer` access;
the human is the project `Owner`. Repository policy requires one approval and
forbids bypassing the ChangeRecord workflow.

```bash
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(
  tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//'
)"
test -n "${TAILSCALE_IP}"
test -n "${TAILSCALE_HOST}"

export SCHEMAHUB_GRPC_PORT=50061
export SCHEMAHUB_SERVER="http://${TAILSCALE_HOST}:${SCHEMAHUB_GRPC_PORT}"
export SCHEMAHUB_LAB="$(mktemp -d /tmp/schemahub-human-agent.XXXXXX)"
export SCHEMAHUB_DB="${SCHEMAHUB_LAB}/schemahub.redb"
export SCHEMAHUB_CONFIG="${SCHEMAHUB_LAB}/schemahub.toml"
export SCHEMAHUB_HUMAN_TOKEN="codelab-human-token"
export SCHEMAHUB_AGENT_TOKEN="codelab-agent-token"

cat >"${SCHEMAHUB_CONFIG}" <<EOF
[auth]
data_dir = "${SCHEMAHUB_LAB}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "human-owner"
display = "Human Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "schema-agent"
display = "Schema Agent"
kind = "agent"
delegated_by = "human-owner"

[projects.codelab]
visibility = "private"
owners = ["human-owner"]
members = { schema-agent = "Writer" }

[repos."codelab/orders"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."codelab/orders".review]
required_approvals = 1
require_change_record = true

[repos."codelab/orders".serving]
source = true
descriptors = true
generated_code = true
EOF
```

These identities are deliberately separate. SchemaHub rejects self-review, and
the reviewer identity is derived from the bearer token rather than supplied by
the client.

## 3. Start SchemaHub and define identity-aware CLI helpers

Bind the server to the Tailscale interface. Clients use the full MagicDNS name:

```bash
"${SCHEMAHUB_SERVER_BIN}" \
  --listen "${TAILSCALE_IP}:${SCHEMAHUB_GRPC_PORT}" \
  --db "${SCHEMAHUB_DB}" \
  --config "${SCHEMAHUB_CONFIG}" \
  --log-format pretty \
  >"${SCHEMAHUB_LAB}/server.log" 2>&1 &
export SCHEMAHUB_PID=$!

human_schemahub() {
  "${SCHEMAHUB_CLI_BIN}" \
    --server "${SCHEMAHUB_SERVER}" \
    --token "${SCHEMAHUB_HUMAN_TOKEN}" \
    --json-errors "$@"
}

agent_schemahub() {
  "${SCHEMAHUB_CLI_BIN}" \
    --server "${SCHEMAHUB_SERVER}" \
    --token "${SCHEMAHUB_AGENT_TOKEN}" \
    --json-errors "$@"
}

for attempt in $(seq 1 30); do
  if agent_schemahub capabilities --json >"${SCHEMAHUB_LAB}/capabilities.json" 2>/dev/null; then
    break
  fi
  sleep 1
done

agent_schemahub capabilities --json \
  | jq '{matrix_version, formats: [.formats[] | .format_id]}'
human_schemahub repo init codelab/orders
```

`repo init` is safe to retry. In this lab, startup configuration has already
seeded the project, repository, roles, and policy, so the command reuses them.
Agents should inspect `capabilities --json` rather than assuming that every
running server supports every format, mutation, or code-generation language.

## 4. Create the proposed schema source

The first change introduces a Protobuf record used by an order data producer:

```bash
mkdir -p "${SCHEMAHUB_LAB}/schemas/orders/v1"
cat >"${SCHEMAHUB_LAB}/schemas/orders/v1/order.proto" <<'EOF'
syntax = "proto3";
package codelab.orders.v1;

message OrderRecord {
  string id = 1;
  int64 created_at_unix_ms = 2;
  bytes payload = 3;
}
EOF
```

The source file is only the proposal input. Once applied, consumers should use
the immutable revision returned by SchemaHub rather than treating this local
file as the source of truth.

## 5. Let the agent create and validate a ChangeRecord

Use `--json` for stable machine-readable output. Keep the ETag returned by every
step: each successful mutation advances it, and a stale writer is rejected.

```bash
CHANGE_JSON="$(agent_schemahub change note codelab/orders \
  --title "Introduce the persisted order envelope" \
  --description "The order writer and replay worker need one versioned wire contract" \
  --reference CODELAB-ORDER-1 \
  --id introduce-order-record \
  --json)"

export CHANGE_NAME="$(printf '%s' "${CHANGE_JSON}" | jq -r '.name')"
export CHANGE_ETAG="$(printf '%s' "${CHANGE_JSON}" | jq -r '.etag')"
printf '%s\n' "${CHANGE_JSON}" \
  | jq '{name, status, created_by, external_references, etag}'
```

The output should identify `schema-agent` as an `agent` delegated by
`human-owner`. Attach the executable edit:

```bash
CHANGE_JSON="$(agent_schemahub change add-source "${CHANGE_NAME}" \
  --etag "${CHANGE_ETAG}" \
  --schema-path orders/v1/order.proto \
  --file "${SCHEMAHUB_LAB}/schemas/orders/v1/order.proto" \
  --json)"
export CHANGE_ETAG="$(printf '%s' "${CHANGE_JSON}" | jq -r '.etag')"

CHANGE_JSON="$(agent_schemahub change validate "${CHANGE_NAME}" \
  --etag "${CHANGE_ETAG}" \
  --json)"
export CHANGE_ETAG="$(printf '%s' "${CHANGE_JSON}" | jq -r '.etag')"
printf '%s\n' "${CHANGE_JSON}" \
  | jq -e '.validation | {valid, resolved_base_commit, edit_digest, issues}'

CHANGE_JSON="$(agent_schemahub change ready "${CHANGE_NAME}" \
  --etag "${CHANGE_ETAG}" \
  --json)"
export CHANGE_ETAG="$(printf '%s' "${CHANGE_JSON}" | jq -r '.etag')"
printf '%s\n' "${CHANGE_JSON}" | jq -e '.status == "ready"'
```

`validate` resolves the exact base, runs the selected compiler, checks
compatibility and references, and stores the result on the ChangeRecord.
`ready` is allowed only for a current, passing validation snapshot.

## 6. Prove that review policy is enforced

Applying before review must fail without changing the ETag:

```bash
if agent_schemahub change apply "${CHANGE_NAME}" \
  --etag "${CHANGE_ETAG}" \
  --request-id apply-introduce-order-record \
  --json \
  >"${SCHEMAHUB_LAB}/unexpected-apply.json" \
  2>"${SCHEMAHUB_LAB}/apply-before-review-error.json"; then
  echo "ERROR: unreviewed apply unexpectedly succeeded" >&2
  exit 1
fi

jq -e '.error.grpc_code == "FAILED_PRECONDITION"' \
  "${SCHEMAHUB_LAB}/apply-before-review-error.json"
```

This demonstrates that an agent with write access cannot bypass the repository's
human-review requirement.

## 7. Let the human review, then let the agent apply

The human should inspect the stored proposal and validation before approving:

```bash
human_schemahub change get "${CHANGE_NAME}" --json \
  | jq '{name, title, edits, validation, created_by, status, etag}'

CHANGE_JSON="$(human_schemahub change approve "${CHANGE_NAME}" \
  --etag "${CHANGE_ETAG}" \
  --reason "Compiler validation and persisted-data contract reviewed" \
  --json)"
export CHANGE_ETAG="$(printf '%s' "${CHANGE_JSON}" | jq -r '.etag')"
printf '%s\n' "${CHANGE_JSON}" \
  | jq '{status, reviews, etag}'
```

The agent can now apply the reviewed snapshot. `--request-id` is the stable
idempotency key: reuse the same value when retrying this logical Apply after a
timeout or disconnect.

```bash
CHANGE_JSON="$(agent_schemahub change apply "${CHANGE_NAME}" \
  --etag "${CHANGE_ETAG}" \
  --request-id apply-introduce-order-record \
  --json)"

export COMMIT_ID="$(printf '%s' "${CHANGE_JSON}" | jq -r '.apply_result.commit_id')"
printf '%s\n' "${CHANGE_JSON}" \
  | jq -e '{status, created_by, reviews, apply_result}'
test "$(printf '%s' "${CHANGE_JSON}" | jq -r '.status')" = "applied"
test -n "${COMMIT_ID}"
```

The durable record now connects the original agent intent, human decision,
idempotent application, JJ operation, and immutable commit.

## 8. Resolve and fetch immutable artifacts

Resolve the mutable `main` bookmark once, then use only the returned revision
resource for artifact reads:

```bash
REVISION_JSON="$(agent_schemahub artifact resolve codelab/orders \
  --at main \
  --json)"
export REVISION_NAME="$(printf '%s' "${REVISION_JSON}" | jq -r '.name')"

printf '%s\n' "${REVISION_JSON}" \
  | jq '{name, commit_id, resolved_from}'
test "$(printf '%s' "${REVISION_JSON}" | jq -r '.commit_id')" = "${COMMIT_ID}"
```

Fetch the binary descriptor and record its digest:

```bash
DESCRIPTOR_JSON="$(agent_schemahub artifact fetch "${REVISION_NAME}" \
  --schema-path orders/v1/order.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_LAB}/order.desc" \
  --json)"
export DESCRIPTOR_DIGEST="$(printf '%s' "${DESCRIPTOR_JSON}" | jq -r '.artifact_digest')"

printf '%s\n' "${DESCRIPTOR_JSON}" \
  | jq '{revision, schema_path, kind, format, content_length, artifact_digest, closure_digest}'

agent_schemahub artifact verify "${REVISION_NAME}" \
  --schema-path orders/v1/order.proto \
  --kind descriptors \
  --digest "${DESCRIPTOR_DIGEST}" \
  --json | jq -e '.valid == true'
```

Fetch generated Rust when the producer wants a build artifact instead of a
dynamic descriptor:

```bash
agent_schemahub artifact fetch "${REVISION_NAME}" \
  --schema-path orders/v1/order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_LAB}/order.rs" \
  --json | jq '{artifact_digest, closure_digest, content_length}'

rg 'OrderRecord' "${SCHEMAHUB_LAB}/order.rs"
```

Artifact reads are keyed by the immutable revision, schema path, artifact kind,
language, and relevant code-generation options. Moving `main` later cannot
change the bytes returned for `REVISION_NAME`.

## 9. Store schema coordinates with application data

A producer should persist enough metadata to retrieve and verify the schema
used to encode its records. This example writes a sidecar manifest; a database
table or message header can carry the same fields:

```bash
jq -n \
  --arg revision "${REVISION_NAME}" \
  --arg schema_path "orders/v1/order.proto" \
  --arg artifact_kind "descriptors" \
  --arg artifact_digest "${DESCRIPTOR_DIGEST}" \
  '{
    schemahub_revision: $revision,
    schema_path: $schema_path,
    artifact_kind: $artifact_kind,
    artifact_digest: $artifact_digest
  }' >"${SCHEMAHUB_LAB}/order-data.schema.json"

jq . "${SCHEMAHUB_LAB}/order-data.schema.json"
```

The application still writes its business data to its normal storage system.
On replay, migration, or forensic read, it uses the stored revision to fetch the
descriptor and the stored digest to verify the returned bytes before decoding.

## 10. Audit durability across restart

Stop and restart the server against the same redb file:

```bash
kill "${SCHEMAHUB_PID}"
wait "${SCHEMAHUB_PID}"

"${SCHEMAHUB_SERVER_BIN}" \
  --listen "${TAILSCALE_IP}:${SCHEMAHUB_GRPC_PORT}" \
  --db "${SCHEMAHUB_DB}" \
  --config "${SCHEMAHUB_CONFIG}" \
  --log-format pretty \
  >"${SCHEMAHUB_LAB}/server-after-restart.log" 2>&1 &
export SCHEMAHUB_PID=$!

for attempt in $(seq 1 30); do
  if human_schemahub change get "${CHANGE_NAME}" --json >"${SCHEMAHUB_LAB}/restored-change.json" 2>/dev/null; then
    break
  fi
  sleep 1
done

jq -e '.status == "applied"
  and .created_by.kind == "agent"
  and .reviews[0].reviewer.kind == "human"' \
  "${SCHEMAHUB_LAB}/restored-change.json"

agent_schemahub artifact verify "${REVISION_NAME}" \
  --schema-path orders/v1/order.proto \
  --kind descriptors \
  --digest "${DESCRIPTOR_DIGEST}" \
  --json | jq -e '.valid == true'
```

The ChangeRecord, actor attribution, review, apply receipt, immutable revision,
and first-materialized artifact bytes all survive the restart.

Stop the lab server when finished:

```bash
kill "${SCHEMAHUB_PID}"
wait "${SCHEMAHUB_PID}"
echo "Lab files remain at ${SCHEMAHUB_LAB}"
```

## 11. Where to go next

- Use `change add-mutation` for compiler-specific granular operations after
  negotiating support with `capabilities --json`.
- Repeat the workflow with `.fbs` input to retrieve FlatBuffers descriptors and
  Rust generated code. `--rust-pluggable-buffer` selects the pluggable-buffer
  Rust runtime.
- OpenAPI supports parsing, references, compatibility, and executable mutations;
  OpenAPI client/server code generation is outside the 1.0 scope.
- Use `schema dependents --json` before changing a shared schema. Discovery is
  bounded and repository-scoped; SchemaHub does not automatically rewrite or
  transact across repositories.
- Continue with [the complete CLI/gRPC codelab](codelab-cli-grpc.md),
  [the GUI guide](gui.md) for browser review, and
  [the operations codelab](codelab-operations.md) for backup, recovery, upgrade,
  and JWT rotation drills.
