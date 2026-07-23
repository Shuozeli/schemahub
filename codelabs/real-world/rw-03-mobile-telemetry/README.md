<!-- agent-updated: 2026-07-23T14:57:16Z -->
# RW-03: Mobile Telemetry Evolution

This runner publishes two FlatBuffers telemetry layouts, compiles both served
Rust bindings, proves default and unknown-slot reader behavior, rejects
physical removal of a deprecated slot, and compares artifact bytes across a
server restart.

```bash
./codelabs/real-world/rw-03-mobile-telemetry/run.sh
```

The expected negative case is `compatibility_violation`. Evidence includes the
old/new generated bindings, reconstructed descriptor bundle, encoded event,
validation reports, post-restart artifacts, and `result.json`.

Follow the guided version in
[`docs/codelab-mobile-telemetry-flatbuffers.md`](../../../docs/codelab-mobile-telemetry-flatbuffers.md).
