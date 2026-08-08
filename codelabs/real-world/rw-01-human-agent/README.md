<!-- agent-updated: 2026-07-30T04:16:42Z -->
# RW-01: Human-Governed Agent Proposal

This executable lab turns the primary SchemaHub tutorial into a release-mode
acceptance scenario. A delegated agent records intent and attaches Protobuf
source, SchemaHub blocks publication before independent review, a human
approves the validated snapshot, and the agent applies it with a retry-safe
request ID.

```bash
./codelabs/real-world/rw-01-human-agent/run.sh
```

The producer resolves an immutable revision, verifies its descriptor, compiles
the served Rust binding, and writes real encoded order bytes plus a schema
sidecar. The lab then restarts SchemaHub and verifies the durable ChangeRecord,
actor/reviewer attribution, Apply receipt, and exact generated artifact bytes.

Expected negative state is `FAILED_PRECONDITION` for Apply before review.
Evidence includes every lifecycle response, the negative error, descriptor and
generated artifacts, encoded data, sidecar coordinates, restart reads, server
events, and `result.json`.

Follow the expanded walkthrough in
[`docs/codelab-human-agent-schema-workflow.md`](../../../docs/codelab-human-agent-schema-workflow.md).
