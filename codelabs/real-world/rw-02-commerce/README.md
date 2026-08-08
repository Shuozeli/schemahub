<!-- agent-updated: 2026-07-23T14:57:16Z -->
# RW-02: Commerce Contract Rollout

This runner publishes a Protobuf order contract through agent proposal and
human approval, advances it additively, compiles both immutable generated Rust
bindings, proves old/new reader interoperability, and rejects a breaking
wire-type edit.

```bash
./codelabs/real-world/rw-02-commerce/run.sh
```

Arrange, Act, Assert phases are visible in `run.sh`. The expected negative case
is `compatibility_violation`; a failed `ready` transition proves the proposal
never became publishable. The runner leaves descriptor bytes, generated
bindings, an encoded order, its revision/digest sidecar, all lifecycle JSON,
and `result.json` in the printed evidence directory.

Follow the guided version in
[`docs/codelab-commerce-protobuf.md`](../../../docs/codelab-commerce-protobuf.md).
