<!-- agent-updated: 2026-07-23T14:57:16Z -->
# SchemaHub Real-World Codelab Suite

These codelabs run production-profile SchemaHub binaries against isolated redb
databases. They use public CLI commands, distinct human/agent/service
identities, real Protobuf and FlatBuffers compiler output, and small Rust
producers/consumers.

Run the complete suite from the repository root:

```bash
./codelabs/real-world/run-all.sh
```

The harness binds to the current Tailscale IP and addresses the server through
its full MagicDNS name. In CI, explicit environment variables select the
documented local-only fallback.

| Scenario | Runner | Guided codelab |
|---|---|---|
| RW-02 Commerce rollout | `rw-02-commerce/run.sh` | [`docs/codelab-commerce-protobuf.md`](../../docs/codelab-commerce-protobuf.md) |
| RW-03 Mobile telemetry | `rw-03-mobile-telemetry/run.sh` | [`docs/codelab-mobile-telemetry-flatbuffers.md`](../../docs/codelab-mobile-telemetry-flatbuffers.md) |
| RW-04 Concurrent editors | `rw-04-concurrent-editors/run.sh` | [`docs/codelab-concurrent-human-agent.md`](../../docs/codelab-concurrent-human-agent.md) |
| RW-05 Data-pipeline handoff | `rw-05-data-pipeline/run.sh` | [`docs/codelab-data-pipeline-handoff.md`](../../docs/codelab-data-pipeline-handoff.md) |

Each runner prints its temporary evidence directory and leaves it intact. The
directory contains transition JSON, expected error JSON, server events,
immutable artifact metadata and bytes, application-data samples, sidecars, and
a normalized `result.json`. Static codelab tokens are scoped to the disposable
server and evidence never contains external credentials.
