<!-- agent-updated: 2026-07-22T15:59:19Z -->
# SchemaHub Tasks

This is the resumable execution checklist for `docs/roadmap.md`. It tracks
deliverables, not a complete issue backlog.

## Product Contract

- [x] Define SchemaHub as schema change collaboration plus immutable schema serving (2026-07-21).
- [x] Separate change intent, versioned state, and artifact serving in the product model (2026-07-21).
- [x] Record the architectural decision in ADR 0001 (2026-07-21).
- [x] Validate the first `ChangeRecord` API shape against a concrete human workflow and agent workflow (2026-07-21).

## D0: Cross-File Codegen Foundation

- [x] Review and preserve the existing uncommitted codegen work (2026-07-21).
- [x] Verify formatting and Clippy in release mode (2026-07-21).
- [x] Verify the PostgreSQL-feature build (2026-07-21).
- [x] Update codegen documentation and the support matrix (2026-07-21).
- [x] Prepare the milestone for an intentional user-approved commit (2026-07-21).

## D1: Change-Record Ledger

- [x] Write the `ChangeRecord` resource and lifecycle proto design (2026-07-21).
- [x] Define injected clock and resource-ID interfaces (2026-07-21).
- [x] Define the transactional `ChangeRecordStore` contract (2026-07-21).
- [x] Implement the in-memory fake and public behavior tests (2026-07-21).
- [x] Implement redb persistence and restart tests (2026-07-21).
- [x] Implement PostgreSQL persistence and transaction tests (2026-07-21).
- [x] Implement Create/Get/List/Update/Delete-draft RPCs (2026-07-21).
- [x] Implement Validate, MarkReady, Approve, Reject, and Apply RPCs (2026-07-21).
- [x] Implement draft-edit and full lifecycle CLI commands with stable JSON output (2026-07-21).
- [x] Prove idempotent application and post-JJ/pre-receipt crash recovery end to end (2026-07-21).
- [x] Add redb process-restart reconciliation plus 32-writer apply-lease and
  receipt-convergence stress tests (2026-07-21).
- [x] Carry ordered external issue/incident/design/automation references through
  storage, gRPC, CLI, HTTP, GUI, and search with bounded validation and legacy
  decode compatibility (2026-07-21).

## D2: Immutable Schema Serving

- [x] Define `SchemaRevision` and `SchemaArtifact` resources (2026-07-21).
- [x] Define deterministic closure-digest encoding (2026-07-21).
- [x] Implement mutable-ref resolution to immutable revisions (2026-07-21).
- [x] Implement source, descriptor, and generated-code artifact reads (2026-07-21).
- [x] Add cache validation to HTTP and gRPC responses (2026-07-21).
- [x] Add CLI fetch/verify commands (2026-07-21).
- [x] Add restart and downstream compilation tests (2026-07-21).

## D3: Durable Resources and Policy

- [x] Replace echo-only repository RPCs with a redb/PostgreSQL persisted repository store (2026-07-21).
- [x] Persist project and membership resources in redb/PostgreSQL (2026-07-21).
- [x] Make project creation plus its initial Owner one atomic database transaction (2026-07-21).
- [x] Implement ETag/field-mask update, pagination, and history-preserving archive for projects and repositories (2026-07-21).
- [x] Add repository review and serving policies and enforce them at publication/read time (2026-07-21).
- [x] Import legacy JSON project/role registries atomically and idempotently (2026-07-21).
- [x] Reject duplicate tag names without moving the original immutable pin (2026-07-21).
- [x] Validate supplied base revisions as retained commits from the target repository while accepting stale causal bases (2026-07-21).
- [x] Persist, bound, expire, prune, and crash-reconcile direct-write idempotency receipts (2026-07-21).
- [x] Reject malformed repository and bootstrap-project configuration at startup (2026-07-21).

## D4: Format Workflows

- [x] Publish a versioned capability RPC plus human/JSON CLI matrix (2026-07-21).
- [x] Implement the selected missing Protobuf workflows (2026-07-21).
- [x] Implement the selected missing FlatBuffers workflows (2026-07-21).
- [x] Define and implement the supported OpenAPI 1.0 transaction workflow (2026-07-21).
- [x] Complete import removal, immutable pinning, and final-state reference-integrity validation (2026-07-21).
- [x] Execute every advertised operation through exact-set gRPC conformance workflows and verify immutable descriptor artifacts (2026-07-21).
- [x] Centralize whole-schema lifecycle policy in Core; enforce explicit format
  matching, create/update existence semantics, protected-source compatibility,
  force RBAC/audit, nested-path discovery, and live-unpinned dependent deletion
  checks across direct writes and ChangeRecord final states (2026-07-21).
- [x] Capture an immutable bookmark planning commit for direct mutation,
  transaction, and lifecycle publication so concurrent same-declaration edits
  become JJ conflicts instead of silent overwrites (2026-07-21).

## D5: Human and Agent Surfaces

- [x] Add a runnable codelab for delegated-agent proposal, human review,
  idempotent Apply, immutable artifact verification, and persisting schema
  coordinates with application data (2026-07-22).
- [x] Replace the GUI's hard-coded workspace with real resource navigation and repository-owned defaults (2026-07-21).
- [x] Add authenticated change drafting, validation, review, Apply, and actor/delegation display to the GUI (2026-07-21).
- [x] Add GUI conflict rendering/resolution and immutable source/descriptor/generated artifact download with digests (2026-07-21).
- [x] Add machine-readable change/artifact/capability output, JSON errors, and stable agent/CI exit codes (2026-07-21).
- [x] Add repository search over schemas, declarations, immutable revisions, and ChangeRecords (2026-07-21).

## D6: Production and Release

- [x] Remove PostgreSQL thread-per-call execution (2026-07-21).
- [x] Add migrations and backup/restore/rollback procedures (2026-07-21).
- [x] Add structured events, metrics, and health/readiness endpoints (2026-07-21).
- [x] Harden the browser HTTP boundary with same-origin defaults, an exact
  trusted-origin allowlist, and a validated request-body ceiling (2026-07-21).
- [x] Generate OpenAPI 3.1 from the registered HTTP handlers, serve it at
  runtime, print it without startup, and package the release-versioned document
  in native archives (2026-07-21).
- [x] Designate `schemahub.v1` gRPC/protobuf as the public 1.0 API and classify
  unversioned `/api/*` as a GUI-only BFF outside that compatibility promise;
  enforce the boundary in response headers, generated OpenAPI, tests, and ADR
  0002 (2026-07-21).
- [x] Make protected-bookmark conflict rejection atomic with the final JJ tree;
  an authorized force may bypass compatibility but must not publish an
  unresolved conflict to a protected bookmark (2026-07-21).
- [x] Make repository-wide live-import validation atomic with publication so a
  concurrent consumer cannot race a schema deletion after validation
  (2026-07-21).
- [x] Enforce the advertised 30-second transaction execution deadline in the
  server, in addition to the existing 100-operation/20-schema bounds
  (2026-07-21).
- [x] Freeze and test bounded direct cross-repository downstream discovery with
  Core Read filtering, pinned/live edges, per-repository immutable snapshot
  manifests, fail-closed limits, and no automatic propagation (2026-07-21).
- [x] Run GC recovery and concurrency drills (2026-07-21).
- [x] Build the complete CI and release artifact matrix, including a reusable
  full-CI tag gate and fail-closed immutable source provenance (2026-07-21).
- [x] Define pinned CI jobs for strict Rust quality, the full redb release
  suite, PostgreSQL 17 integration, GUI production build, and runtime-container
  smoke/drain (2026-07-21).
- [x] Build and locally verify tag-versioned native archives, a non-root
  PostgreSQL-capable distroless image, checksums/provenance inputs, and auditable
  Rust SBOM discovery (2026-07-21).
- [x] Resolve every public repository-local read ref once, return exact snapshot
  coordinates, honor configured defaults, and reject raw commits retained only
  by another repository (2026-07-21).
- [x] Make `ListCommits` honor its exclusive stop and schema-touch filters,
  expose the exact traversal root in initial metadata, bound its scan, and
  include conflicted-tree changes (2026-07-21).
- [x] Implement exact requested-field/property `FollowType` resolution with
  populated declaration detail, source/target commits, import path, pin state,
  cross-repository authorization, and explicit scalar/missing/ambiguous errors
  (2026-07-21).
- [x] Implement normalized direct/transitive forward dependency edges over
  immutable `(schema, commit)` nodes, with same-repo/live/pinned semantics,
  per-target repository snapshots, explicit unresolved leaves, and fail-closed
  bounds/errors (2026-07-21).
- [x] Make compiler import discovery whole-schema-aware and surface supported
  external OpenAPI component `$ref` values through forward/reverse discovery,
  immutable closure serving, exact property following, and live-import
  deletion guards (2026-07-21).
- [x] Prove external OpenAPI dependency snapshots, descriptor closure,
  `FollowType`, reverse discovery, and deletion protection through the public
  gRPC boundary after the provider advances (2026-07-21).
- [x] Apply repository-configured defaults to omitted ChangeRecord targets and
  reject malformed browser Authorization headers instead of treating them as
  anonymous (2026-07-21).
- [x] Prove bearer forwarding through real CLI processes for repository and
  schema commands; fail closed on unreadable/malformed CLI configuration and
  require an explicit server coordinate (2026-07-21).
- [x] Fail closed when OpenAPI input would otherwise become an empty/default
  declaration, and when persisted JJ objects contain missing fields, malformed
  IDs, unsupported submodules, or non-NotFound storage faults (2026-07-21).
- [x] Add one release-mode GA acceptance journey that applies delegated-agent
  Protobuf and FlatBuffers changes after human review, materializes immutable
  descriptors and generated Rust, restarts redb, and proves durable actor,
  review, receipt, byte, and digest identity (2026-07-21).
- [x] Serve bounded operation-log tails without scanning the complete normal
  linear history, while preserving full traversal semantics for branched JJ
  histories (2026-07-21).
- [x] Replace the Protobuf compiler path dependencies with immutable Git
  revision `a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac` and make the release gate
  verify compiler sources in `Cargo.lock` (2026-07-21).
- [x] Publish and pin the coordinated FlatBuffers compiler commit, replace its
  final cross-repository path dependencies with immutable Git revision
  `7dc2c76c08f452b9a208230057c0cb6327e65f24`, and prove its default and
  all-feature matrix in GitHub Actions plus SchemaHub's full matrix against the
  remote revision (2026-07-21).
- [x] Make the normal CI compiler-lock check transition-safe across the
  temporary all-path FlatBuffers state and the canonical immutable Git state,
  while preserving the strict no-path release gate (2026-07-21).
- [x] Require version-specific release notes with migration, mixed-version,
  rollback, compatibility, known-issue, source/compiler provenance, and exact
  multi-architecture image-digest fields before publication (2026-07-21).
- [x] Configure `PROTOBUF_RS_REF` and `FLATBUFFERS_RS_REF` as exact repository
  variables matching `Cargo.lock`: Protobuf
  `a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac` and FlatBuffers
  `7dc2c76c08f452b9a208230057c0cb6327e65f24` (2026-07-21).
- [x] Move the pinned Rust cache action to v2.9.1 commit
  `e18b497796c12c097a38f9edb9d0641fb99eee32`, whose action runtime is Node
  24, before publishing a candidate (2026-07-22).
- [x] Persist first-materialized artifact bytes under a versioned canonical
  request identity before response; prove first-writer convergence, corruption
  rejection, and descriptor/generated retrieval after redb restart with an
  empty compiler registry (2026-07-21).
- [x] Integrate production external JWT/JWKS identity with explicit issuer,
  audience, token type, asymmetric algorithms, injected time, atomic key
  rotation, stale-key fail-closed readiness, and human/agent/service audit
  claims; verify the JWT subject through project-owner creation over gRPC
  (2026-07-21).
- [ ] Publish a SchemaHub 0.9 release candidate. The 2026-07-22 GitHub audit
  found no configured staging environment, deployment, repository secret, or
  existing package, so the exact-digest real-provider staging gate still needs
  a deployment target and intended issuer configuration.
- [ ] Complete the 1.0 acceptance workflow and publish SchemaHub 1.0.

## Latest Verification Evidence

- [x] The human/agent usage codelab was executed against a release server bound
  to Tailscale with static RBAC and redb: unreviewed Apply failed closed,
  human approval enabled one agent Apply, descriptor/generated Rust digests
  verified, and actor/review/artifact identity survived restart (2026-07-22).
- [x] SchemaHub pull-request Actions run `29878830277` and post-merge `main`
  run `29879390452` passed all five clean-checkout jobs: strict Rust quality,
  the full 586-test release suite plus generated OpenAPI, PostgreSQL 17, GUI,
  and the production container smoke/drain contract (2026-07-22).
- [x] FlatBuffers pull-request and post-merge `main` GitHub Actions passed the
  Rust test, formatting, and strict-Clippy matrix at immutable compiler commit
  `7dc2c76c08f452b9a208230057c0cb6327e65f24` (2026-07-21).
- [x] SchemaHub's standalone Docker context fetched both immutable compiler Git
  revisions and produced non-root distroless rehearsal image manifest
  `sha256:20e3807e1c0dd152cb622ecbcca913139031a3744cb27bf7ba3fa8817338f65f`
  without sibling-repository source (2026-07-21).
- [x] Release workspace suite: 586 tests passed after the immutable-read,
  CLI-auth/config, storage-corruption, bounded-op-log, whole-schema OpenAPI
  external-reference, and dual-compiler GA acceptance increments (2026-07-21).
- [x] Coordinated FlatBuffers default- and all-feature release workspace suites,
  normal and all-feature production-target strict Clippy, locked metadata,
  format/diff checks, runtime safety contract, cycle-free optional gRPC
  generation with downstream compile proof, and all ten SchemaHub generated-code
  compilation workflows passed locally (2026-07-21).
- [x] Versioned capability matrix and selected Protobuf, FlatBuffers, OpenAPI direct/transaction API workflows (2026-07-21).
- [x] Strict release Clippy and compilation across the workspace, all features,
  and all targets (2026-07-21).
- [x] Strict PostgreSQL-feature release Clippy plus server feature compilation (2026-07-21).
- [x] JWT security/config unit tests, rotating-key and stale-cache tests, HTTP
  auth-readiness contract, and signed-token-to-durable-owner gRPC acceptance
  passed under the all-feature release profile (2026-07-21).
- [x] Pinned Rust 1.95 distroless image rebuilt with production JWT support and
  passed a MagicDNS runtime drill: signed access, live `kid` replacement,
  old-key rejection, last-known-good retention, stale-key request/readiness
  failure, Docker healthy/unhealthy/recovered transitions, fail-closed explicit
  config, recovery, non-root runtime, and graceful exit 0 (2026-07-21).
- [x] Twenty-five database-backed PostgreSQL tests plus two bounded-executor
  tests, including load, compare-and-swap contention, migrations, distributed
  GC fencing, repository-scoped publication locking, atomic initial-ref
  creation, and restart recovery (2026-07-21).
- [x] Online PostgreSQL 17 backup/restore drill reproduced immutable revision
  `4a3429a4d4a8de0ce8e4133a6b58fb5eeb969f261cdcf6110d763286678931c7ed6cdf66a010fd6e280dab077bdd172ef2ad2660ba9825537ca50a6b94a08b30`
  byte-for-byte with artifact digest
  `sha256:efed26e27286a6e7f5c3b2dd66ba8396efae05da0977565bf5cd21d3cc30e052`
  (2026-07-21).
- [x] redb offline backup/restore and cross-repository GC/restart/undo drills
  (2026-07-21).
- [x] HTTP liveness/readiness/metrics, standard gRPC health, correlated request
  events, and bounded graceful shutdown process verification (2026-07-21).
- [x] GitHub workflow validation with actionlint plus YAML, Dockerfile, shell,
  tag-version, archive-layout, OCI-label, non-root, readiness, capability, and
  graceful-stop checks (2026-07-21).
- [x] Auditable container SBOM rehearsal discovered 453 packages, including 443
  Rust crates, 10 Debian packages, and every SchemaHub workspace crate
  (2026-07-21).
- [x] Four PostgreSQL resource-record create/list/CAS/compare-delete integration tests against isolated schemas (2026-07-21).
- [x] PostgreSQL atomic project/owner batch rollback integration test (2026-07-21).
- [x] Correlated Apply retry and post-JJ/pre-receipt recovery tests (2026-07-21).
- [x] Post-JJ/pre-receipt recovery reopens redb and proves one correlated
  operation; 32 independent ledger instances elect exactly one lease and 32
  concurrent reconcilers return one persisted receipt (2026-07-21).
- [x] Durable artifact tests cover atomic first-writer-wins across changed
  renderers, canonical option-aware request keys, corruption fail-closed
  behavior, and exact descriptor/generated bytes after redb restart with no
  renderer registered (2026-07-21).
- [x] Rebuilt non-root image
  `sha256:fe043c236f856e2900fe4f2e2d081caf6d19e9470b76d3102c13f67b00f9f81c`
  passed MagicDNS readiness and a real descriptor first-materialization/restart
  comparison over a persistent redb volume; both reads were
  `d872b6d1aa02e5803cfda100b9943f215da8aa5241d85e25210f43a1ca9221bf`
  (2026-07-21).
- [x] Immutable serving pinning, repository isolation, digest, ETag/304, redb restart, and downstream compilation tests (2026-07-21).
- [x] Project/repository ETag, pagination, archive, RBAC lockout, redb restart, and legacy migration tests (2026-07-21).
- [x] Immutable-tag, stale/foreign base-revision, key-reuse, bounded receipt, redb restart, and post-JJ receipt-recovery tests (2026-07-21).
- [x] GUI TypeScript checks and production Vite build, plus locked Cargo
  metadata and rebuilt CLI/server release-binary smoke checks (2026-07-21).
- [x] HTTP BFF resource, identity, ChangeRecord lifecycle, search, conflict resolution, and artifact-cache integration tests (2026-07-21).
- [x] External-reference normalization/bounds/legacy decode, gRPC create/update,
  HTTP create/search, CLI help/JSON, and GUI production-build coverage
  (2026-07-21).
- [x] HTTP boundary tests cover canonical startup policy, trusted and unlisted
  origins, preflight headers, same-origin defaults, and pre-handler `413`
  rejection without mutation (2026-07-21).
- [x] Generated OpenAPI validation covers 22 path templates, 23 operations,
  release-tag version metadata, bearer auth, ChangeRecord external references,
  live discovery, no-startup binary output, CI parsing, and archive packaging
  from the exact release binary (2026-07-21).
- [x] HTTP API-boundary tests verify `/api/*` response classification,
  operational-route separation, per-path OpenAPI compatibility metadata,
  public `schemahub.v1` metadata, and CORS exposure (2026-07-21).
- [x] Reverse-dependency Core and gRPC tests prove direct live/pinned edge
  discovery, deterministic ordering, configured-default snapshots, hidden-repo
  non-disclosure, and stable CLI JSON metadata; server config advertises the
  1,000-repository/10,000-schema bounds (2026-07-21).
- [x] Immutable-read tests prove exact response commits, configured non-main
  defaults, repository-wide diff coordinates, real commit-stream stop/schema
  filtering plus metadata, and public rejection of foreign commits across read
  and ref-publication RPCs (2026-07-21).
- [x] Compiler/Core/gRPC tests prove exact requested-field/property resolution,
  populated details, same/cross-repository live snapshots, immutable pins,
  normalized forward edges, explicit unresolved leaves, and fail-closed search
  and traversal behavior (2026-07-21).
- [x] Browser resource tests prove omitted note targets resolve to the
  repository's configured bookmark and malformed Authorization bytes return
  `401` rather than anonymous access (2026-07-21).
- [x] Whole-schema lifecycle tests cover required/matching formats,
  ALREADY_EXISTS/NOT_FOUND behavior, protected compatibility, Maintainer-only
  force, durable force audit metadata, nested schema discovery, force-resistant
  live-import rejection, final-state ChangeRecord migrations, and immutable
  concurrency snapshots (2026-07-21).
- [x] Deterministic publication-boundary races prove repository-wide
  serialization across shared backends, protected conflict rejection for
  direct writes, merges, bookmark moves, and undo, both consumer/delete race
  orderings, atomic initial operation-head creation, and immediate safe retry
  after receipt or Apply-lease cleanup on policy rejection (2026-07-21).
- [x] Transaction-deadline tests prove the server returns
  `DEADLINE_EXCEEDED`, cooperative cancellation prevents a late bookmark move,
  and expiry while queued at the publication guard removes the claimed receipt
  before a retry (2026-07-21).
- [x] Formatting and `git diff --check` (2026-07-21).
- [x] Release workflow actionlint validation, immutable-ref positive/negative
  checks, missing-provenance rejection, and a versioned archive rehearsal whose
  metadata contains all three exact source revisions (2026-07-21).
