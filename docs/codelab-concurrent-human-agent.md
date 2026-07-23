<!-- agent-updated: 2026-07-23T14:57:16Z -->
# Codelab: Resolve Concurrent Human and Agent Schema Edits

This lab gives a human and a delegated agent the same immutable Protobuf base.
Both edit `OrderRecord` differently. SchemaHub must reject stale record
updates, avoid silently choosing a declaration, make Apply retry-safe, and
preserve the explicit resolution through restart.

## What you will prove

- ChangeRecord ETags prevent stale draft overwrite;
- both validations name the same immutable base;
- the second same-base write becomes a first-class JJ declaration conflict;
- an identical Apply retry returns the same commit and operation;
- conflict rendering exposes both sides;
- explicit resolution retains both business fields;
- actor attribution, receipts, bookmark state, and artifacts survive restart.

## 1. Run the complete lab

```bash
./codelabs/real-world/rw-04-concurrent-editors/run.sh
```

For a stable evidence location:

```bash
export SCHEMAHUB_CODELAB_EVIDENCE_DIR=/tmp/schemahub-concurrency-evidence
./codelabs/real-world/rw-04-concurrent-editors/run.sh
jq . /tmp/schemahub-concurrency-evidence/result.json
```

The lab uses protected `main` and an unprotected `collab` bookmark. Publication
still requires ChangeRecords, while the unprotected bookmark is allowed to
retain a conflict for later resolution.

## 2. Arrange one shared causal base

The delegated agent publishes:

```proto
message OrderRecord {
  string id = 1;
}
```

The human creates `collab` from that commit. Both ChangeRecords set
`--base-revision "$BASE_COMMIT"` and `--target-bookmark collab`.

The human side adds:

```proto
string human_note = 2;
```

The agent side independently adds:

```proto
string agent_note = 2;
```

The same field number makes silent selection unacceptable.

## 3. Observe optimistic concurrency on the draft

After the agent attaches its source, the runner deliberately submits an update
using the earlier ETag:

```bash
agent_schemahub change update "$AGENT_CHANGE" \
  --etag "$STALE_ETAG" \
  --description "stale overwrite attempt" --json
```

Expected structured error:

```bash
jq '.error | {kind, grpc_code}' "$EVIDENCE/agent-stale-error.json"
```

`grpc_code` is `ABORTED`; the attached executable edit remains intact.

## 4. Publish both same-base edits

Each record validates against the exact base commit and becomes Ready. The
human Apply moves `collab`; the agent Apply still uses its validated immutable
base. JJ retains the competing `OrderRecord` blobs:

```bash
jq '.apply_result' "$EVIDENCE/agent-05-apply.json"
```

`conflicted_declarations` contains `OrderRecord`.

The runner then retries with the same request ID and original Ready ETag:

```bash
agent_schemahub change apply "$AGENT_CHANGE" \
  --etag "$AGENT_READY_ETAG" \
  --request-id apply-agent-order-edit --json
```

The retry's commit and operation IDs must match the first response.

## 5. Render and resolve deliberately

Render through the public CLI:

```bash
human_schemahub resolve \
  retail/collaboration/schemas/order.proto \
  OrderRecord --branch collab
```

The evidence file `conflict-rendered.proto` contains both competing sides. The
human then submits the reviewed resolution:

```bash
human_schemahub resolve \
  retail/collaboration/schemas/order.proto \
  OrderRecord \
  --branch collab \
  --from codelabs/real-world/rw-04-concurrent-editors/fixtures/order-resolved.proto \
  --author collaboration-owner \
  --message "Resolve human and agent order notes"
```

The resolved declaration assigns distinct tags:

```proto
message OrderRecord {
  string id = 1;
  string human_note = 2;
  string agent_note = 3;
}
```

## 6. Compile the resolution and restart

The lab resolves `collab` to an immutable revision, fetches generated Rust, and
runs `protobuf_concurrent.rs`. That consumer encodes and decodes a record
containing both notes.

After server restart, the lab reads both ChangeRecords and verifies:

- the human record still has `created_by.kind == "human"`;
- the agent record still has `created_by.kind == "agent"`;
- the original operation receipt is unchanged;
- `collab` resolves to the explicit resolution commit;
- the descriptor digest still verifies.

This is the practical distinction between a stale ETag and a JJ content
conflict: the former rejects an unsafe resource overwrite; the latter retains
two valid causal writes until a person or agent chooses a complete result.

Continue with
[the data-pipeline handoff codelab](codelab-data-pipeline-handoff.md).
