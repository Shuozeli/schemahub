<!-- agent-updated: 2026-07-23T03:45:01Z -->
# Real-World Validation Portfolio

This portfolio hardens SchemaHub by using it as an actual schema control and
serving plane. It complements unit and integration tests: each scenario starts
real release-mode processes, uses public CLI or gRPC interfaces, exercises a
recognizable producer/consumer workflow, and preserves enough evidence to
reproduce any failure.

The interactive site in `apps/schemahub-demo` is the human-facing index. It may
simulate a command sequence for teaching, but a scenario is not complete until
the corresponding sequence has passed against a real SchemaHub server and
compiler.

## Completion Contract

Every scenario must:

1. Declare its business story, actors, repository policy, schema formats, and
   expected compatibility decisions.
2. Create deterministic schema/data fixtures without depending on a prior
   scenario or execution order.
3. Run the production CLI against a release-mode server with distinct human,
   delegated-agent, and service identities where applicable.
4. Capture commands, stable JSON fields, expected failures, commit/revision
   coordinates, artifact digests, and the relevant server events.
5. Verify the artifact with a real producer or consumer, not only by checking
   that bytes were returned.
6. Exercise restart, retry, concurrency, or rollback when those behaviors are
   part of the story.
7. Leave an automated regression test for every product defect found.
8. Record a result in the bug ledger even when no product defect is found.

Secrets, bearer tokens, temporary database paths, and machine-specific
coordinates must not appear in committed evidence.

## Portfolio

| ID | Scenario | Formats and pressure | State | Exit evidence |
|---|---|---|---|---|
| RW-01 | Delegated agent proposes; human approves; consumer pins | Protobuf + FlatBuffers; policy, Apply retry identity, immutable artifacts, restart | Passing | Runnable codelab, real-server transcript assertions, generated Rust, and byte/digest comparison |
| RW-02 | Commerce order-contract rollout | Protobuf; additive rollout, breaking rejection, generated producer/consumer bindings, stored revision/digest | Next | Old and new consumers decode their intended fixtures; protected breaking edit fails without a commit |
| RW-03 | Mobile telemetry evolution | FlatBuffers; defaults, deprecation, old/new readers, generated artifacts | Queued | Compatibility decisions match reader behavior before and after server restart |
| RW-04 | Human and agent edit concurrently | Both; stale ETags/bases, JJ conflicts, idempotent retries, crash recovery | Queued | No silent overwrite or duplicate publication; audit identities and receipts survive restart |
| RW-05 | Batch/stream producer-consumer handoff | Both; immutable serving, sidecar coordinates, digest verification, rollback | Queued | Historical data remains decodable after `main` advances and after a rollback |

The first scenario is documented in
`codelab-human-agent-schema-workflow.md`. Later scenarios should gain their own
fixture directory and codelab only when their real-server execution is
repeatable.

## Evidence Layout

Scenario automation should converge on this repository layout:

```text
scenarios/
  rw-02-commerce/
    README.md
    fixtures/
    run.sh
    assertions/
  rw-03-mobile-telemetry/
  rw-04-concurrent-editors/
  rw-05-data-pipeline/
```

`run.sh` is an intended interface, not permission to hide behavior in an opaque
script. A scenario README must list the exact Arrange, Act, and Assert phases,
the supported backend, runtime requirements, expected negative cases, and the
artifacts it leaves under an ignored temporary evidence directory.

## Bug Ledger

| Finding | Scenario | Class | Severity | State | Evidence or resolution |
|---|---|---|---|---|---|
| RV-HARNESS-001 | RW-01 site | Validation harness | Low | Fixed | Replaced invalid pnpm native-build placeholders with explicit `esbuild`, `sharp`, and `workerd` approvals; frozen install passes |
| RV-DOCS-001 | RW-01 site | Documentation | Low | Fixed | Corrected pnpm argument forwarding in the Tailscale development command; production browser smoke passes |
| RV-SITE-001 | RW-01 site | Deployment runtime | Medium | Fixed | Next 16 server bundles either retained a runtime `require()` or exceeded the 10 MiB Worker limit; the fully prerenderable demo now ships a static export behind a tiny assets-only Worker, and CI boots it in workerd |
| — | RW-01 server | Product | — | No defect found yet | Real codelab passed the guarded lifecycle, compilation, serving, and restart checks |

A product finding receives a stable `RW-<scenario>-<number>` identifier and one
of these severities:

- **Release blocker**: data loss/corruption, authorization or isolation breach,
  silent incompatible publication, immutable-byte drift, or duplicate Apply.
- **High**: documented safety behavior fails without a safe operational
  workaround.
- **Medium**: a supported workflow is incorrect or materially confusing but
  fails visibly and preserves data.
- **Low**: diagnostics, documentation, ergonomics, or harness behavior is
  misleading without changing SchemaHub state.

## GA Feedback Loop

For each scenario: run from clean fixtures, triage the ledger, add the smallest
reproduction, fix in scope, rerun the focused case, and then rerun the complete
portfolio. SchemaHub is scenario-ready for GA only when all five rows pass from
clean state, every release blocker and high-severity finding is resolved, and
the generated evidence still agrees with the documented 1.0 boundaries.
