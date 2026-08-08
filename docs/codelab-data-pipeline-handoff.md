<!-- agent-updated: 2026-07-23T14:57:16Z -->
# Codelab: Hand Off and Roll Back Data-Pipeline Schemas

This lab models two application-data paths:

- a batch producer stores Protobuf `PipelineOrder` records;
- a stream producer stores FlatBuffers `PipelineEvent` records.

Their schemas live in separate SchemaHub repositories. The producer stores the
immutable revision and digests beside each data file, then both repositories
advance. A replay consumer must still decode the old batch. Finally, an
operator rolls each repository back explicitly.

## What you will prove

- served generated bindings can encode real application bytes;
- a data sidecar is sufficient to retrieve and verify the historical binding;
- advancing `main` does not alter old artifact bytes;
- old data decodes after both repositories advance;
- repository rollback moves mutable `main` but does not erase v2 revisions;
- SchemaHub does not pretend two repository undos are one global transaction.

## 1. Run the complete lab

```bash
./codelabs/real-world/rw-05-data-pipeline/run.sh
```

To preserve the evidence at a known path:

```bash
export SCHEMAHUB_CODELAB_EVIDENCE_DIR=/tmp/schemahub-pipeline-evidence
./codelabs/real-world/rw-05-data-pipeline/run.sh
jq . /tmp/schemahub-pipeline-evidence/result.json
```

The runner starts a release server on the Tailscale interface with one project,
two repositories, a human owner, a delegated orchestration agent, and separate
producer/consumer service identities.

## 2. Publish the producer contracts

The agent uses one ChangeRecord in `analytics/orders` and another in
`analytics/events`. Each repository returns its own commit and revision:

```bash
agent_schemahub change note analytics/orders \
  --title "Publish the batch order record" \
  --id pipeline-order-v1 --json

agent_schemahub change note analytics/events \
  --title "Publish the stream event record" \
  --id pipeline-event-v1 --json
```

The complete source/validate/ready/apply sequence is implemented in
`codelabs/real-world/rw-05-data-pipeline/run.sh`. A stable request ID is scoped
to each Apply.

## 3. Encode application data with served code

The producer resolves both commits, fetches generated Rust, and compiles two
small programs:

```text
generated PipelineOrder binding -> data/orders.bin
generated PipelineEvent binding -> data/events.bin
```

The programs live under `codelabs/real-world/consumers/src/bin/` and use the
actual `prost` and `flatbuffers` runtimes. This proves the artifacts are usable
by data code, not merely downloadable.

## 4. Store immutable schema sidecars

`orders.bin.schema.json` has this shape:

```json
{
  "schemahub_revision": "projects/analytics/repos/orders/revisions/…",
  "schema_path": "schemas/pipeline-order.proto",
  "descriptor_digest": "sha256:…",
  "generated_rust_digest": "sha256:…"
}
```

The FlatBuffers sidecar has the same fields and its own repository revision.
The binary payload checksums are captured before any schema advance.

In a database or message system, these fields can be columns or headers. The
important invariant is that the mutable bookmark is not the stored coordinate.

## 5. Advance both repositories

The order schema gains `warehouse_zone`; the event schema gains `region`. Both
are additive and applied from their explicit v1 base commits.

After `main` advances, the replay path does not resolve `main`. It uses the
sidecar's v1 revision:

```bash
consumer_schemahub artifact fetch "$SIDECAR_REVISION" \
  --schema-path "$SIDECAR_SCHEMA_PATH" \
  --kind generated-code --language rust \
  --output historical.rs --json

consumer_schemahub artifact verify "$SIDECAR_REVISION" \
  --schema-path "$SIDECAR_SCHEMA_PATH" \
  --kind generated-code --language rust \
  --digest "$SIDECAR_GENERATED_DIGEST" --json
```

The fetched bytes compare equal to the producer's original binding. The
consumer compiles them and decodes both stored files. Their payload checksums
remain unchanged.

## 6. Roll back without erasing history

SchemaHub's operation log is repository-scoped. The operator therefore makes
two visible actions:

```bash
human_schemahub undo analytics/orders --author data-platform-owner
human_schemahub undo analytics/events --author data-platform-owner
```

Afterward:

- `analytics/orders@main` resolves to the order v1 commit;
- `analytics/events@main` resolves to the event v1 commit;
- both v2 immutable revisions remain fetchable and their descriptor digests
  still verify.

This is a coordinated operational workflow, not an automatic multi-repository
transaction. If the second undo failed, the operator would see a partial state
and retry or roll forward explicitly.

## 7. Audit the result

Read:

```bash
jq . "$EVIDENCE/result.json"
jq . "$EVIDENCE/data/orders.bin.schema.json"
jq . "$EVIDENCE/data/events.bin.schema.json"
```

The result deliberately reports `"global_transaction": false`. This makes the
1.0 product boundary executable: SchemaHub provides immutable coordinates and
per-repository history; the release orchestrator owns cross-repository
coordination.

For the collaboration path that produces explicit conflicts, see
[the concurrent human/agent codelab](codelab-concurrent-human-agent.md).
