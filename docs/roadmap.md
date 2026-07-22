<!-- agent-updated: 2026-07-21T23:31:27Z -->
# SchemaHub Deliverable Roadmap

## Outcome

Deliver a production-usable SchemaHub 1.0 centered on two product promises:

1. Humans and agents can durably record schema-change intent and safely turn it
   into versioned schema history.
2. Data producers and consumers can retrieve deterministic schema artifacts by
   immutable revision.

The roadmap is ordered by dependency and user-visible outcomes. A phase is not
complete until its workflow is runnable end to end and its acceptance tests pass.

## Delivery Sequence

```text
D0 Codegen foundation
        |
        v
D1 Change-record ledger
        |
        v
D2 Immutable schema serving
        |
        v
D3 Durable resources and policy
       / \
      v   v
D4 Format workflows   D5 Human and agent surfaces
       \ /
        v
D6 Production hardening and 1.0
```

## D0: Cross-File Codegen Foundation

**Release target:** 0.2

Complete the current in-progress work that makes an import closure identify its
requested root explicitly and resolves Protobuf named types across files.

Deliverables:

- Explicit `SchemaClosure` root semantics.
- Protobuf imported and nested type resolution before code generation.
- FlatBuffers root selection based on the requested schema, not map ordering.
- Multi-level Protobuf and FlatBuffers fixtures whose generated Rust compiles.
- Updated codegen documentation and capability matrix.

Exit criteria:

- `cargo fmt --all -- --check` passes.
- `cargo clippy --release --workspace --all-targets -- -D warnings` passes.
- `cargo test --release --workspace` passes.
- PostgreSQL-feature compilation passes.
- `pnpm run build` passes for the GUI.

## D1: First-Class Change-Record Ledger

**Release target:** 0.3

**Status (2026-07-21): complete.** The policy-neutral lifecycle works through
gRPC and CLI on memory, redb, and PostgreSQL: executable edits, deterministic
validation, readiness/review, recoverable correlated Apply, and idempotent
receipts. Redb process-restart recovery proves the post-JJ/pre-receipt window,
and 32 independent writers prove single-lease election and first-receipt
convergence.

Ordered external issue, incident, design, and automation references now travel
with the same durable record across every human and agent surface.

Introduce the durable resource that links human or agent intent to an applied
schema commit.

Deliverables:

- `ChangeRecord`, actor, status, validation, review, and application-result
  protobuf resources, including bounded external references.
- Standard Create/Get/List/Update/Delete-draft methods plus custom Validate,
  MarkReady, Approve, Reject, Abandon, and Apply methods.
- A storage trait with redb and PostgreSQL implementations.
- Server-derived actor identity and injected clock/ID providers.
- Atomic apply: one record transition, schema commit, and auditable operation.
- Compatibility reports and conflicts attached as structured data.
- CLI commands for humans and stable JSON output for agents.

Exit criteria:

- A note-only draft can be created and later made executable.
- A validated record can be applied exactly once despite request retries.
- Restart tests preserve records and their links to commits.
- Human and agent identities are distinguishable in audit output without
  accepting an untrusted client-supplied identity.

## D2: Immutable Schema Serving

**Release target:** 0.4

**Progress (2026-07-21):** implemented end to end through Core, gRPC, HTTP,
and CLI. Resolved revision names are repository-scoped, artifact reads are
immutable, source/descriptor/generated-code outputs carry SHA-256 and closure
digests, cache validators work over both transports, and redb restart plus
fresh downstream compilation are covered by release tests. The first successful
artifact materialization is now atomically persisted before response and reused
across restarts and renderer upgrades, including when the upgraded server has
no matching compiler installed.

Provide the read path used by data systems at build time and runtime.

Deliverables:

- `SchemaRevision` and `SchemaArtifact` resources.
- Resolve mutable bookmark/tag input to an immutable revision.
- Fetch canonical source, descriptor bundles, and supported generated code by
  revision.
- Deterministic content digests and dependency metadata.
- Cache-safe HTTP semantics in the BFF and equivalent gRPC metadata.
- CLI fetch and verify commands suitable for build pipelines.

Exit criteria:

- Moving a bookmark cannot change an already resolved artifact.
- A producer can persist a revision ID and a fresh consumer can retrieve the
  same bytes after restart.
- Equal canonical schema closures have equal closure digests; artifact digests
  identify the exact served bytes.
- Generated Protobuf and FlatBuffers Rust compiles in downstream test crates.

## D3: Durable Resources and Repository Policy

**Release target:** 0.5

**Status (2026-07-21): complete.** Project, membership, and repository resources
persist in redb/PostgreSQL with atomic bootstrap ownership, ETags, field-mask
updates, stable pagination, and history-preserving archive behavior. Review and
serving policies are enforced dynamically, malformed startup configuration is
rejected, and legacy JSON ACLs import transactionally. Tag names are immutable;
base revisions are repository-owned causal inputs with stale bases accepted;
all direct schema writes use bounded, persistent, JJ-correlated receipts that
survive restart and post-publication crashes.

Replace resource stubs with a truthful, persistent control plane.

Deliverables:

- Persisted project and repository stores for redb and PostgreSQL.
- Authorized project/repository Create, Get, List, Update, and archive/delete
  behavior using Google AIP request shapes.
- Repository compatibility, protected-bookmark, review, retention, and serving
  policies.
- Immutable tag behavior and explicit base-revision semantics under JJ.
- Persistent bounded idempotency records.
- Startup rejection of malformed configuration.

Exit criteria:

- No echo-only or silently successful project/repository RPC remains.
- All database reads and writes execute inside transactions.
- Authorization and pagination contract tests cover every resource method.
- Protected targets reject unreviewed or conflicted publication according to
  repository policy.
- Literal retries across every direct write surface return the original commit
  after restart, while changed requests cannot reuse the scoped key.

## D4: Supported Format Workflows

**Release target:** 0.6

**Status:** Complete (2026-07-21), including exact-final-tree publication
validation delivered in D6. The versioned capability RPC/CLI, selected
Protobuf and FlatBuffers gaps, OpenAPI transactions, immutable import
pins/removal, and final-state reference validation are implemented. Exact-set
gRPC conformance workflows execute every advertised operation and materialize
immutable descriptor artifacts; compiler suites cover compatibility, conflict,
round-trip, and invalid-reference behavior.

Make the advertised format matrix honest and complete for selected workflows.

Deliverables:

- A versioned support matrix for Protobuf, FlatBuffers, and OpenAPI.
- Import removal/update and reference-integrity validation.
- Missing high-value message, enum, service, table, and union mutation paths.
- OpenAPI transaction support for the operations declared supported in 1.0.
- Round-trip, compatibility, conflict, and artifact-serving tests per supported
  operation.

Exit criteria:

- Every advertised operation is reachable through the API and at least one
  maintained client.
- Unsupported operations return explicit structured errors and are not listed
  as supported.
- Format compatibility behavior is captured in executable matrix tests.

## D5: Human and Agent Surfaces

**Release target:** 0.7

**Status:** Complete (2026-07-21). The live BFF is now the GUI default and
persisted project/repository context drives every route and default bookmark.
Humans and delegated agents share ChangeRecords across CLI/gRPC/browser;
browser actions cover note creation, validation, readiness, review, Apply,
abandonment, conflict resolution, immutable artifact downloads, and
repository-scoped search. ETags, server-derived identities, real auth mode,
stable CLI JSON, JSON errors, and classified exit codes are surfaced and
covered by release tests.

Expose the same trustworthy workflows through interfaces optimized for each
actor without creating separate policy paths.

Deliverables:

- GUI project/repository selection backed by real persisted resources.
- GUI change drafting, validation, review, diff, conflict resolution, and
  artifact download.
- CLI resource names, machine-readable JSON, non-interactive operation, and
  stable exit codes for agents and CI.
- Search over schemas, revisions, and change records.
- Authentication and delegated-agent identity display.

Exit criteria:

- The GUI has no hard-coded workspace or mock-only production path.
- The same change can be created by CLI and reviewed/applied in the GUI.
- An agent can complete the workflow without scraping human-formatted output.

## D6: Production Hardening and SchemaHub 1.0

**Release targets:** 0.9 release candidate, then 1.0

**Progress (2026-07-21):** SchemaHub now uses a bounded long-lived PostgreSQL
executor, applies checksum-verified embedded migrations before readiness, and
exposes correlated structured events, Prometheus metrics, HTTP probes, standard
gRPC health, and coordinated graceful shutdown. Cross-repository GC reachability
and mutation fencing are enforced for redb and PostgreSQL. Release tests cover
load, compare-and-swap contention, restart/undo, and redb/PostgreSQL
backup/restore drills. Pinned CI/release definitions, tagged archives, the
non-root multi-architecture image, provenance inputs, and auditable SPDX SBOMs
are locally rehearsed. The Protobuf and FlatBuffers compilers are now immutable
Git dependencies, and normal/release gates reject lockfile/compiler-coordinate
drift or any cross-repository path dependency. Production external identity is
now delivered as strict JWT verification over a startup-loaded and rotating
HTTPS/file JWKS,
with durable subject mapping, fail-closed explicit configuration, and stale-key
readiness propagated into container health. Durable first-materialization now
closes the cross-version artifact-byte gate through atomic first-writer-wins
storage, verified restart-without-renderer retrieval, and fail-closed record
validation. The browser BFF now defaults to same-origin access, validates an
exact trusted-origin allowlist, and rejects oversized requests before handlers
run. Its 22-path/23-operation OpenAPI 3.1 contract is generated from registered
handlers, served at runtime, printable without startup, and embedded in native
release archives. ADR 0002 now designates `schemahub.v1` gRPC/protobuf as the
public 1.0 API and classifies unversioned `/api/*` as a same-release GUI BFF
outside that compatibility promise. Runtime headers, per-path OpenAPI metadata,
and integration tests enforce the distinction; operational routes remain a
separate supported contract. Cross-repository coordination now has a public,
bounded `ListDependents` contract: it returns direct pinned/live edges and a
per-repository immutable snapshot manifest after Core Read filtering, without
claiming a global transaction or automatic propagation. Publishing clean
compiler pins, the 0.9 candidate, and final 1.0 acceptance remain open.
Exact-final-tree policy is now enforced under
a backend repository publication guard: protected conflicts and live-import
deletion races reject before JJ publication, including across PostgreSQL
instances, with receipt/Apply-lease release on known policy rejection. The
transaction RPC now has an independent 30-second server timer and cooperative
Core cancellation through the final publication gate.
Repository reads now share the same immutable-coordinate discipline: mutable
refs resolve once, every response reports its exact snapshot, raw commits are
repository-owned at the JJ boundary, repository-wide diffs report both sides,
and bounded commit streams honor their stop/schema filters. Field traversal is
now exact for the requested Protobuf/FlatBuffers field or OpenAPI property, and
forward import traversal reports normalized live/pinned target snapshots and
explicit unresolved leaves instead of silently skipping them.
The compiler import boundary now accepts whole schema objects, so supported
external OpenAPI component `$ref` values participate in forward/reverse
discovery, immutable descriptor closure assembly, exact property following,
and final-tree deletion policy rather than being hidden in declaration blobs.
The same historical-provider behavior is now exercised through the public gRPC
surface after the provider advances. Release automation accepts the temporary
all-path FlatBuffers development state or one canonical immutable Git revision
without weakening the exact-coordinate tag gate. Version-specific release
notes are validated and rendered with exact source/compiler coordinates and the
multi-architecture image digest before SBOM/checksum assembly and publication.
Stored JJ records now validate required fields and exact object identities at
the persistence boundary, bounded audit-log reads avoid complete linear-history
scans, and malformed OpenAPI structures fail parsing instead of entering the
immutable history as empty declarations.
The local GA acceptance journey now executes the product promise as one
release-mode test for both native compiler integrations: a delegated agent
creates a Protobuf change and a FlatBuffers change, a human owner approves
each, the agent applies them through protected-repository policy, immutable
descriptor and Rust artifacts are materialized at the exact resulting commits,
and a redb server restart returns the same records, bytes, and digests.

Deliverables:

- PostgreSQL concurrency and load testing without thread-per-query execution.
- Database migrations and documented backup, restore, upgrade, and rollback.
- Structured event logs, metrics, health/readiness endpoints, and graceful
  shutdown.
- A fail-closed browser boundary with explicit trusted origins and bounded
  request bodies (delivered 2026-07-21).
- Generated, version-matched OpenAPI discovery through the running server,
  binary output, release archives, and route/schema drift tests (delivered
  2026-07-21).
- A documented and enforced 1.0 boundary: `schemahub.v1` is public and current
  unversioned HTTP convenience routes remain a GUI-only BFF (delivered
  2026-07-21).
- GC safety and recovery drills.
- Supported authentication integration beyond development static tokens:
  explicit JWT trust policy, human/agent/service claims, atomic JWKS rotation,
  and fail-closed readiness (delivered 2026-07-21).
- Versioned binaries and container image, SBOM, release notes, deployment
  codelab, and compatibility policy.
- Cross-release byte stability for artifacts already returned from an immutable
  revision through durable, versioned first-materialization storage (delivered
  2026-07-21).
- Atomic final-tree policy validation: protected bookmarks never publish
  unresolved conflicts, and a concurrent consumer cannot race a provider
  deletion after reference validation (delivered 2026-07-21).
- A server-enforced 30-second transaction execution deadline alongside the
  existing 100-operation and 20-schema limits (delivered 2026-07-21).
- Centralized whole-schema lifecycle orchestration with truthful format,
  existence, force, compatibility, dependency, and immutable-base semantics
  (delivered 2026-07-21).
- Bounded direct cross-repository dependency discovery with authorization
  filtering, immutable snapshot manifests, stable CLI JSON, explicit pin state,
  and fail-closed limits (delivered 2026-07-21).
- Immutable repository-local exploration, history, diff, and codegen reads with
  exact response coordinates and repository-owned raw commits (delivered
  2026-07-21).
- Exact field/property type traversal and bounded, snapshot-safe forward
  dependency closure semantics for compiler-reported imports (delivered
  2026-07-21).
- Whole-schema compiler import discovery for supported external OpenAPI
  component refs, including round-trip, immutable closure, reverse discovery,
  exact `FollowType`, and deletion-guard semantics (delivered 2026-07-21).

Exit criteria:

- CI covers redb, PostgreSQL, all compilers, generated-code compilation, CLI,
  HTTP BFF, and GUI builds.
- A clean environment can deploy, create and apply a change, store a revision
  identifier, retrieve the artifact, restart, and retrieve identical bytes.
- The automated GA journey performs that lifecycle for both Protobuf and
  FlatBuffers while preserving delegated-agent authorship and human review.
- The public v1 API (including the explicit REST/BFF boundary) and data
  migration policy are documented and frozen.
- Deterministic race tests prove protected conflict rejection and delete/import
  integrity at the exact publishing boundary.
- Deadline tests prove timeout status, no late pre-publication commit, and
  idempotency-receipt cleanup while waiting at the publication guard.
- Direct reverse-discovery tests prove hidden repositories do not leak, each
  reported edge is tied to an immutable importing commit, and live/pinned state
  remains explicit without implying cross-repository atomicity.
- Snapshot tests prove mutable refs are resolved once, configured defaults are
  honored, foreign commits cannot cross repository boundaries, history filters
  are real, and forward dependency/type traversal returns exact source/target
  commits or explicit failure state.

## Scheduling Assumption

For one primary engineer, plan approximately 14–18 weeks. With two engineers,
D4 and D5 can proceed in parallel after D3, reducing the critical path to about
10–12 weeks. Scope is controlled by the exit criteria, not the calendar: a
partially wired resource does not count as a delivered phase.
