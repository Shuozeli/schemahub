<!-- agent-updated: 2026-07-23T14:57:16Z -->
# Codelab: Evolve a FlatBuffers Mobile Telemetry Event

This lab models mobile clients that cannot all upgrade at once. A telemetry
team must retain the old FlatBuffers slot, mark it deprecated, and add a
sampling field whose default is meaningful to already-deployed readers.

## What you will prove

- deprecating a field preserves its slot;
- adding a field at the end with a default passes full compatibility;
- a v2 reader sees `1.0` when reading a v1 event;
- a v1 reader ignores the v2 sampling slot and retains known fields;
- physically removing the deprecated field is rejected;
- immutable generated-code bytes are identical after a server restart.

## 1. Run the complete lab

```bash
./codelabs/real-world/rw-03-mobile-telemetry/run.sh
```

To choose the evidence path:

```bash
export SCHEMAHUB_CODELAB_EVIDENCE_DIR=/tmp/schemahub-telemetry-evidence
./codelabs/real-world/rw-03-mobile-telemetry/run.sh
jq . /tmp/schemahub-telemetry-evidence/result.json
```

The runner uses a release server on the Tailscale interface, isolated redb,
separate human/agent/service identities, and the repository's full
compatibility plus review policy.

## 2. Inspect the safe evolution

Version 1 has three implicit slots. Version 2 retains their order:

```fbs
namespace telemetry;

table MobileEvent {
  event_id: string;
  captured_at_unix_ms: long;
  legacy_session_id: string (deprecated);
  sampling_rate: float = 1.0;
}

root_type MobileEvent;
```

FlatBuffers compatibility depends on slot identity. Deprecation is safe because
the slot remains. Deleting the line would cause later fields to reuse different
slot positions.

## 3. Publish both layouts

For each version, the delegated agent notes intent, attaches the `.fbs` file,
validates it, marks the frozen snapshot Ready, receives human approval, and
applies it with a stable request ID:

```bash
agent_schemahub change note mobile/telemetry \
  --title "Deprecate legacy session and add sampling rate" \
  --base-revision "$V1_COMMIT" \
  --id mobile-event-v2 --json

agent_schemahub change add-source "$CHANGE_NAME" \
  --etag "$ETAG" \
  --schema-path schemas/mobile-event.fbs \
  --file codelabs/real-world/rw-03-mobile-telemetry/fixtures/mobile-event-v2.fbs \
  --json

agent_schemahub change validate "$CHANGE_NAME" --etag "$ETAG" --json
agent_schemahub change ready "$CHANGE_NAME" --etag "$ETAG" --json
human_schemahub change approve "$CHANGE_NAME" --etag "$ETAG" \
  --reason "Old/new reader behavior and compiler report reviewed" --json
agent_schemahub change apply "$CHANGE_NAME" --etag "$ETAG" \
  --request-id apply-mobile-event-v2 --json
```

Inspect the stored report:

```bash
jq '.validation | {valid, resolved_base_commit, issues}' \
  "$EVIDENCE/mobile-event-v2-03-validate.json"
```

## 4. Exercise the real generated readers

SchemaHub serves generated Rust for both immutable revisions. The consumer in
`codelabs/real-world/consumers/src/bin/flatbuffers_compat.rs` compiles those
exact bytes and runs two directions:

- a v1 builder writes `event-1001`; the v2 reader returns sampling rate `1.0`;
- a v2 builder writes sampling rate `0.25`; the v1 reader returns the known
  event ID and timestamp.

The result is not inferred from the compatibility checker—the runtime actually
parses both buffers.

## 5. Prove that deprecation is not deletion

The breaking fixture omits `legacy_session_id`. Its validation report contains
`compatibility_violation`, and `ready` returns `FAILED_PRECONDITION`:

```bash
jq '.validation' \
  "$EVIDENCE/mobile-event-breaking-03-validate.json"
jq '.error' \
  "$EVIDENCE/mobile-event-breaking-ready-error.json"
```

`main` remains at the accepted v2 revision.

## 6. Restart and compare immutable artifacts

After first materialization, the runner stops the process and starts a new one
against the same redb file. It fetches v1 and v2 generated Rust again and uses
`cmp`:

```bash
cmp "$EVIDENCE/mobile-event-v1.rs" \
  "$EVIDENCE/mobile-event-v1-after-restart.rs"
cmp "$EVIDENCE/mobile-event-v2.rs" \
  "$EVIDENCE/mobile-event-v2-after-restart.rs"
```

Both comparisons must be byte-identical. The v2 descriptor digest is verified
through the serving API after restart.

## 7. Known finding exposed by this lab

The current FlatBuffers generated Rust is correct but emits normal compiler
warnings for generated camel-case builder functions, unused root helpers, and
a deprecated accessor used by its own `Debug` implementation. This is tracked
as `RW-03-001` in the real-world bug ledger. It is an ergonomics/codegen
cleanliness issue, not a wire-correctness failure.

Continue with
[the concurrent-editor codelab](codelab-concurrent-human-agent.md).
