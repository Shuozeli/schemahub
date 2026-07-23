<!-- agent-updated: 2026-07-23T14:57:16Z -->
# Codelab: Roll Out a Protobuf Order Contract

This lab models a retail team evolving the Protobuf record stored by its order
writer. A delegated schema agent prepares the change, a human owner reviews it,
and producer/consumer services read immutable artifacts. You will prove the
wire behavior with compiled generated bindings—not only a compatibility label.

## What you will prove

- `main` requires a ChangeRecord and one human approval.
- an additive field passes full compatibility;
- an identical Apply retry returns the original commit and operation;
- old bytes decode with the new binding and new bytes decode with the old one;
- a persisted identifier changing from `string` to `int64` is rejected;
- the rejected draft cannot move `main`;
- application data can retain the immutable revision and descriptor digest
  needed for later replay.

## 1. Run the complete lab

From the SchemaHub repository root:

```bash
./codelabs/real-world/rw-02-commerce/run.sh
```

The runner builds release binaries unless
`SCHEMAHUB_CODELAB_SKIP_BUILD=1` is set, creates a disposable redb database,
binds the server to the Tailscale IP, and uses the full MagicDNS hostname as the
client endpoint. It prints an evidence directory at completion.

To retain evidence at a known path:

```bash
export SCHEMAHUB_CODELAB_EVIDENCE_DIR=/tmp/schemahub-commerce-evidence
./codelabs/real-world/rw-02-commerce/run.sh
jq . /tmp/schemahub-commerce-evidence/result.json
```

## 2. Inspect the business fixtures

The initial contract is
`codelabs/real-world/rw-02-commerce/fixtures/order-v1.proto`. Version 2 adds
only field 4:

```proto
message OrderRecord {
  string id = 1;
  int64 total_cents = 2;
  int64 created_at_unix_ms = 3;
  string settlement_currency = 4;
}
```

The negative fixture reuses field number 1 as `int64`. That is a wire-type
change for a persisted key, not an additive rollout.

## 3. Follow the control-plane handoff

The runner executes the complete lifecycle with the ETag returned by each
step. The essential commands are:

```bash
agent_schemahub change note retail/orders \
  --title "Add settlement currency to the order record" \
  --base-revision "$V1_COMMIT" \
  --id order-v2 --json

agent_schemahub change add-source "$CHANGE_NAME" \
  --etag "$ETAG" \
  --schema-path schemas/order.proto \
  --file codelabs/real-world/rw-02-commerce/fixtures/order-v2.proto \
  --json

agent_schemahub change validate "$CHANGE_NAME" --etag "$ETAG" --json
agent_schemahub change ready "$CHANGE_NAME" --etag "$ETAG" --json
human_schemahub change approve "$CHANGE_NAME" \
  --etag "$ETAG" \
  --reason "Compatibility report and rollout behavior reviewed" --json
agent_schemahub change apply "$CHANGE_NAME" \
  --etag "$ETAG" \
  --request-id apply-order-v2 --json
```

`agent_schemahub` and `human_schemahub` are identity-aware helpers defined by
the shared harness. The server derives the delegated agent and human reviewer
from their bearer tokens.

The v1 Apply is immediately retried with the same request ID. Compare:

```bash
jq '.apply_result | {commit_id, operation_id}' \
  "$EVIDENCE/order-v1-06-apply.json" \
  "$EVIDENCE/order-v1-07-apply-retry.json"
```

Both coordinates must be identical.

## 4. Prove actual wire interoperability

The producer resolves each commit to a revision and fetches generated Rust:

```bash
producer_schemahub artifact fetch "$V1_REVISION" \
  --schema-path schemas/order.proto \
  --kind generated-code --language rust \
  --output "$EVIDENCE/order-v1.rs" --json
```

The permanent consumer at
`codelabs/real-world/consumers/src/bin/protobuf_compat.rs` compiles those exact
v1/v2 files. It performs two independent checks:

1. encode v1, decode v2, and observe an empty default currency;
2. encode v2 with `USD`, decode v1, and preserve every known v1 field.

The encoded v2 order is written as `order-v2.bin`.

## 5. Store schema coordinates beside the order

The data sidecar is `order-v2.bin.schema.json`:

```json
{
  "schemahub_revision": "projects/retail/repos/orders/revisions/…",
  "schema_path": "schemas/order.proto",
  "artifact_kind": "descriptors",
  "artifact_digest": "sha256:…"
}
```

The order bytes remain in the application's storage system. SchemaHub stores
the schema history and immutable artifacts, not the business record.

## 6. Observe the breaking-change gate

Validation of `order-breaking.proto` succeeds as an RPC but returns a stored
negative report:

```bash
jq '{
  valid: .validation.valid,
  issues: .validation.issues
}' "$EVIDENCE/order-breaking-03-validate.json"
```

Expected: `valid` is false and at least one issue has code
`compatibility_violation`. The subsequent `ready` call fails with
`FAILED_PRECONDITION`, and resolving `main` still returns the v2 commit.

## 7. Read the evidence

`result.json` is the normalized exit record. Transition files preserve all
actor, review, validation, commit, operation, revision, and digest fields.
`consumer.txt` is the generated-binding execution result. No external secrets
or machine-specific coordinates are checked into the repository.

Continue with
[the FlatBuffers telemetry codelab](codelab-mobile-telemetry-flatbuffers.md) or
[the data-pipeline handoff codelab](codelab-data-pipeline-handoff.md).
