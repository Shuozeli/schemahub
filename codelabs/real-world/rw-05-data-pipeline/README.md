<!-- agent-updated: 2026-07-23T14:57:16Z -->
# RW-05: Data-Pipeline Handoff and Rollback

This runner models a batch Protobuf producer and a stream FlatBuffers producer
in separate repositories. It stores immutable schema coordinates beside their
application bytes, advances both repositories, replays old data from the
sidecars, then rolls back each repository explicitly while retaining every
immutable v2 artifact.

```bash
./codelabs/real-world/rw-05-data-pipeline/run.sh
```

The scenario intentionally has no global transaction: the evidence contains
two independent Apply histories and two independent `undo` operations. It also
contains encoded data, sidecars, generated bindings, descriptors, replay
checks, and `result.json`.

Follow the guided version in
[`docs/codelab-data-pipeline-handoff.md`](../../../docs/codelab-data-pipeline-handoff.md).
