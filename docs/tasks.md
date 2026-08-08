<!-- agent-updated: 2026-07-30T04:23:54Z -->
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
- [x] Replace global `ListChanges` scans with atomically maintained
  repository/status indexes, bounded range pagination, fail-closed target
  validation, and one-time legacy backfill (2026-07-30).

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
- [x] Atomically append typed, actor-attributed project/member/repository audit
  events and expose Owner-only cursor pagination plus CLI JSON (2026-07-29).
- [x] Serialize administrative mutations per project across server instances
  so concurrent Owner removal/downgrade cannot leave zero Owners (2026-07-29).
- [x] Replace complete-history audit scans with an atomically maintained
  immutable order index, bounded backend range reads, project-bound cursors,
  and fail-closed event/index validation (2026-07-30).
- [x] Replace complete project/repository catalog scans with atomically
  maintained active/all project and per-project repository indexes, bounded
  prefix range reads, authorization-aware continuation, one-time durable
  backfill, and fail-closed target validation (2026-07-30).
- [x] Add project-bound `ListMembers` pagination over bounded existing
  project/hex-identity role-key ranges, tombstone-safe continuation, scoped
  corruption checks, complete CLI JSON traversal, and direct GUI caller-role
  lookup (2026-07-30).
- [x] Add bounded `ListBranches`/`ListTags` response pagination with
  ref-kind/project/repository/prefix-bound tokens, lazy ordered JJ-view
  iteration, complete CLI traversal, and direct `GetBranch` lookup
  (2026-07-30).

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
- [x] Build a responsive interactive companion that walks the real CLI
  lifecycle for Protobuf and FlatBuffers, exposes 1.0 boundaries, and passes
  production, OpenNext, and desktop/mobile browser smoke checks (2026-07-23).
- [x] Replace the GUI's hard-coded workspace with real resource navigation and repository-owned defaults (2026-07-21).
- [x] Add authenticated change drafting, validation, review, Apply, and actor/delegation display to the GUI (2026-07-21).
- [x] Add direct GUI authoring for whole-schema source replacements and
  deletions, including ETag-protected conversion of note-only drafts and
  validation invalidation after edits (2026-07-24).
- [x] Add a real Chromium smoke for GUI source creation, validation
  invalidation, executable-edit replacement, and schema deletion; run it in
  the pinned GUI CI job (2026-07-24).
- [x] Add a live Chromium acceptance backed by the real HTTP BFF, redb server,
  and release CLI for agent source authoring, rejected pre-review Apply,
  independent human approval, agent Apply, and restart-stable audit/artifact
  evidence (2026-07-24).
- [x] Split operator pages into lazy route chunks and enforce a 450,000-byte
  production entry budget in local and CI validation (2026-07-29).
- [x] Replace the runtime-CDN Monaco loader with a self-contained accessible
  read-only source viewer and reject known CDN references in the production
  bundle contract (2026-07-29).
- [x] Prevent the fixed-height operator header from wrapping over content at
  tablet/mobile widths and enforce its geometry in the mock Chromium
  acceptance (2026-07-29).
- [x] Replace unbounded GUI BFF project/repository arrays and the project
  summary's N+1 repository counts with bounded Core catalog pages,
  kind/project/prefix-bound opaque tokens, incremental React continuations,
  bounded repository deep-link lookup, OpenAPI tests, and Pwright/browser
  acceptance (2026-07-30).
- [x] Normalize Chrome's loopback discovery WebSocket onto the configured
  neutral Tailscale CDP host in GUI/demo browser smokes and cover HTTP, HTTPS,
  direct WebSocket, and malformed discovery behavior (2026-07-30).
- [x] Page repository-dashboard schema/branch/tag aggregates with one
  repository/ref-bound continuation and immutable schema snapshot, adapt the
  existing indexed ChangeRecord page with repository/status-bound tokens, and
  make every React consumer request continuations explicitly. Batch-load the
  selected schema page and repository-local inventory in one immutable tree
  traversal so summary cost no longer multiplies a full scan by page size
  (2026-07-30).
- [x] Repair live browser acceptance to query the identity control by its exact
  `Identity:`-prefixed accessible name and close remote CDP connections during
  unconditional teardown (2026-07-30).
- [x] Add GUI conflict rendering/resolution and immutable source/descriptor/generated artifact download with digests (2026-07-21).
- [x] Add machine-readable change/artifact/capability output, JSON errors, and stable agent/CI exit codes (2026-07-21).
- [x] Add repository search over schemas, declarations, immutable revisions, and ChangeRecords (2026-07-21).

## Real-World Validation Portfolio

- [x] Define the execution, evidence, finding-severity, and completion contract
  for scenario-driven GA hardening (2026-07-23).
- [x] Execute the delegated-agent/human-review scenario against the real server
  and compiler path, including rejected pre-approval Apply, immutable
  descriptors/generated Rust, restart, and digest identity (2026-07-22).
- [x] Exercise a Protobuf commerce rollout with additive and breaking edits,
  generated producer/consumer bindings, and data-side revision/digest pins
  (2026-07-23).
- [x] Exercise FlatBuffers mobile telemetry evolution with defaults,
  deprecation, old/new readers, and byte-stable generated artifacts
  (2026-07-23).
- [x] Exercise concurrent human/agent editing with stale bases and ETags,
  conflict resolution, idempotent retries, and process restart recovery
  (2026-07-23).
- [x] Exercise a producer/consumer data-pipeline handoff with immutable
  serving, digest verification, rollback, and retained historical decoding
  (2026-07-23).
- [x] Add an authoritative structured finding ledger and fail-closed GA report
  that requires the exact seven-scenario set, rejects credential material,
  records normalized result digests and source/run provenance, requires clean
  candidate source, and becomes a retained release input (2026-07-24).
- [ ] Resolve every release-blocking finding, rerun all scenarios from clean
  fixtures on a pushed candidate commit, and publish its evidence-backed GA
  readiness report. After also closing `RV-CATALOG-001`, `RV-MEMBER-001`,
  `RV-REF-001`, `RV-BFF-CATALOG-001`, `RV-BFF-AGGREGATE-001`, and
  `RW-GUI-002`, the local fresh-fixture portfolio is green with zero open
  findings, report digest
  `sha256:20b4575efdf4a95411d6213e573949da80bbd7dc7da536076c971cf1b12761b4`,
  and byte-reproducible deterministic archive digest
  `sha256:93c203bfaff0491f72fe12dd8964729309d33b7d1ad9015eb1763490afc83aba`;
  all 699 release-workspace tests, strict all-target/all-feature Clippy, 28
  PostgreSQL tests, GUI build/bundle/CDP checks, the remote mock continuation
  smoke, the live governed browser/restart acceptance, and an independent
  neutral-Pwright dashboard-continuation check pass. Clean-checkout Actions
  evidence remains required.

## D6: Production and Release

- [x] Remove PostgreSQL thread-per-call execution (2026-07-21).
- [x] Add migrations and backup/restore/rollback procedures (2026-07-21).
- [x] Add structured events, metrics, and health/readiness endpoints (2026-07-21).
- [x] Harden the browser HTTP boundary with same-origin defaults, an exact
  trusted-origin allowlist, and a validated request-body ceiling (2026-07-21).
- [x] Reject a symlinked GUI assets root or any symbolic link below `gui_dir`
  before HTTP startup, preventing the static-file service from escaping the
  configured filesystem boundary (2026-07-29).
- [x] Restrict one-year immutable GUI caching to Vite-style content-hashed
  asset filenames and serve successful unhashed assets with `no-cache`
  (2026-07-29).
- [x] Protect the bearer-token operator console with a self-only CSP, framing
  denial, restricted browser permissions, MIME-sniffing protection, and
  same-origin referrers; assert the policy in the router, exact production
  image, and a real Pwright render (2026-07-29).
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
- [x] Withhold semantic-version container tags until their required release
  gates succeed: push only a run-unique candidate ref before stable staging,
  assemble and checksum every release asset, promote the accepted digest
  afterward, refuse a conflicting existing tag, and make GitHub publication
  depend on verified promotion (2026-07-29).
- [x] Treat the retained release assembly as an exact supply-chain boundary:
  reject unsafe/duplicate names, non-file entries, missing files,
  unchecksummed extras, malformed checksum lines, and byte drift before upload
  and after download; retain the Actions artifact digest and publish only the
  reverified file set (2026-07-29).
- [x] Enforce locked dependency security as a release boundary: run pinned
  `cargo-audit` from a checksummed source archive and a separately reviewed,
  self-audited tool lock; require a zero-vulnerability and exact
  reviewed-warning SchemaHub contract; audit both frozen pnpm graphs at Low
  severity; replace the vulnerable JWT RSA backend with AWS-LC; lock patched
  Rust and web dependency versions; scope the sole React Router exception to
  unused RSC APIs; verify cargo-auditable's checksummed source and exact clean
  lock before forcing an isolated release install; pin every external container
  stage, the Dockerfile frontend, and CI helper image to a multi-architecture
  manifest digest; use an exact Node runtime; make image build-tool versions
  non-overridable; and test that all tool/source identities, base images, helper
  images, action commits, runtime versions, backend, lockfiles, allowlist,
  auditor version, and CI policy cannot drift silently (2026-07-29).
- [x] Define pinned CI jobs for strict Rust quality, the full redb release
  suite, PostgreSQL 17 integration, GUI production build, and runtime-container
  smoke/drain (2026-07-21).
- [x] Build and locally verify tag-versioned native archives, a non-root
  PostgreSQL-capable distroless image, checksums/provenance inputs, and auditable
  Rust SBOM discovery (2026-07-21).
- [x] Ship the exact locked GUI in every native archive and release container;
  serve it from the same HTTP origin with fail-fast bundle validation, explicit
  SPA routes, immutable hashed-asset caching, API-fallback isolation, native
  package contract tests, no runtime CDN dependency, and container smoke
  coverage (2026-07-29).
- [x] Make container CI write through a real named volume as UID/GID `65532`,
  create and serve a Protobuf schema, materialize descriptor and generated-Rust
  artifacts, replace the container while retaining only that volume, and prove
  exact schema/revision/digest identity plus graceful exit for both processes
  (2026-07-29).
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
- [x] Publish the warning-clean FlatBuffers follow-up and update SchemaHub's
  immutable Git revision. Commit
  `59756d23993538b722f68675c35129c3cebb7aa1` uses associated `Type::create`
  constructors; makes deprecated fields read-only across builders, Args,
  Object API, `Debug`, and serde; and fixes typed struct-union accessors through
  the fully qualified `Follow` trait and public `Table` accessors. Its complete
  default/all-feature release, strict production Clippy, formatting,
  generated-code, and downstream warning contracts passed locally and in
  FlatBuffers main Actions run `30481753669`; all three SchemaHub compiler
  crates now resolve from that exact remote revision (2026-07-29).
- [x] Make the normal CI compiler-lock check transition-safe across the
  temporary all-path FlatBuffers state and the canonical immutable Git state,
  while preserving the strict no-path release gate (2026-07-21).
- [x] Require version-specific release notes with migration, mixed-version,
  rollback, compatibility, known-issue, source/compiler provenance, and exact
  multi-architecture image-digest fields before publication (2026-07-21).
- [x] Add the version-specific SchemaHub 1.0.0 contract covering upgrade,
  migration, mixed-version prohibition, rollback, production staging,
  frozen API/BFF boundaries, current limitations, evidence assets, and exact
  provenance; test missing stable sections and boundaries fail closed
  (2026-07-24).
- [x] Add per-finding `must_fix_before` release deadlines and make the tag
  workflow reject 1.0.0 or later while the warning-clean FlatBuffers pin
  finding remains open, without preventing 0.9 or 1.0 prerelease validation
  (2026-07-24).
- [x] Require stable releases to pass a protected-environment staging
  attestation bound to the exact source, image digest, and GA-readiness
  archive; validate PostgreSQL, real-provider identity, bundled-GUI
  same-origin serving, artifact, dependency, restore, evidence-safety, and
  freshness claims; independently read and validate the environment's sole
  `v*.*.*` deployment policy; and include the normalized result in release
  checksums/assets (2026-07-29).
- [x] Keep `PROTOBUF_RS_REF` and `FLATBUFFERS_RS_REF` as exact repository
  variables matching `Cargo.lock`: Protobuf is
  `a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac` and FlatBuffers is
  `59756d23993538b722f68675c35129c3cebb7aa1` (2026-07-30).
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
- [ ] Publish a SchemaHub 0.9 release candidate. The prior GitHub audit found
  no staging environment, deployment, repository secret, release, or tag.
  Both compiler repository variables now match `Cargo.lock`, but the current
  SchemaHub tree still needs an explicitly authorized commit and push before a
  clean candidate run. The exact-digest real-provider gate still needs a
  deployment target, intended issuer configuration, and an independent
  reviewer. The workflow contract fails stable publication closed until a
  protected `schemahub-production-staging` environment supplies the matching
  attestation; that environment is not yet configured.
- [ ] Complete the 1.0 acceptance workflow and publish SchemaHub 1.0.

## Latest Verification Evidence

- [x] Live GitHub repository variables now match the current lock exactly:
  `PROTOBUF_RS_REF=a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac` and
  `FLATBUFFERS_RS_REF=59756d23993538b722f68675c35129c3cebb7aa1`.
  Both immutable compiler-lock checks, release metadata for `0.9.0-rc.1`, and
  the release-finding gates for the candidate and `1.0.0` pass. The repository
  still has no release/tag/environment, and its sole collaborator cannot
  satisfy the documented independent-review staging contract alone
  (2026-07-30).
- [x] `RV-RELEASE-003` made the native distribution byte-reproducible:
  OpenAPI HTTP/CLI output now shares recursively key-sorted bytes, packaging
  normalizes sorted ustar membership, UID/GID, modes, UTC timestamps, and gzip
  headers, and the three-platform release matrix packages twice before upload.
  The eight-process regression passed; two real auditable host-target archives
  were byte-identical at
  `sha256:9291e80bf47cdc44de06090fe43f45d54bf33d22e512481457558e154cee74eb`,
  and repeated execution of the extracted server matched the embedded OpenAPI
  digest
  `sha256:ef8876a87dc66a8be2839d5700b639f203a7a247dde9c0c058fc7a87dc025026`
  exactly. Strict all-target/all-feature workspace Clippy, the full 609-test
  default release workspace, and the separate 25-test PostgreSQL release slice
  passed
  (2026-07-29).
- [x] `RV-SEC-001` closed a runtime static-file root escape: `gui_dir`
  validation now rejects a symlinked `assets` directory and every symbolic
  link nested anywhere in the bundle. Both Arrange-Act-Assert failure contracts
  passed, followed by all 607 locked release-workspace tests, formatting, and
  strict all-target/all-feature Clippy. The exact-base non-root image
  `sha256:380c649e600fa39a90d43a9846fcc87d502ddbf51d3946ba4cb37a77fc6a69d7`
  accepted the shipped symlink-free GUI and passed named-volume replacement
  persistence with both immutable artifact digests unchanged (2026-07-29).
- [x] `RV-HTTP-001` aligned runtime caching with the documented 1.0 boundary:
  the positive hashed-asset test still receives a one-year immutable policy,
  while a new successful unhashed-asset test receives `no-cache`. The focused
  five-test bundled-GUI HTTP group, all 607 locked release-workspace tests, and
  strict all-target/all-feature Clippy passed. The rebuilt production image
  passed the persistence acceptance (2026-07-29).
- [x] `RV-HTTP-002` hardened the bearer-token console with a self-only CSP,
  framing denial, browser-feature restrictions, MIME protection, and
  same-origin referrers. The focused router checks, all 607 locked release
  tests, strict Clippy, exact image
  `sha256:79d333b36b10bdee47641062d54e09223b11f326231ed56188e85be1574d4e78`,
  named-volume replacement acceptance, and real Pwright render all passed. A
  fresh RW-01 through RW-07 run then reported zero open findings
  (2026-07-29).
- [x] `RV-HARNESS-006` removed a nondeterministic release-package test failure:
  archive membership no longer uses an early-closing `tar | grep -q` pipeline
  under `pipefail`. The corrected complete-listing assertion passed 120
  consecutive runs and ShellCheck (2026-07-29).
- [x] A live GitHub audit reconfirmed draft PR 4 remains mergeable at its last
  pushed source `a8b3c5f29d6aa91f5cd0e4ab9ad5c4fea7b1e844`, where all six jobs in
  Actions run `30019762296` passed. Both current workflow files and the local
  1.0 hardening remain unpushed, so that historical run does not cover the
  present candidate. The repository has no environments, deployments, secret
  names, releases, or tags;
  `PROTOBUF_RS_REF` matches the lock while `FLATBUFFERS_RS_REF` still names
  `7dc2c76c08f452b9a208230057c0cb6327e65f24` (2026-07-29).
- [x] The complete optimized workspace suite, formatting, and strict
  all-target/all-feature Clippy passed after moving RS256 verification to
  AWS-LC. The checksummed cargo-audit source built from its reviewed tool lock,
  and that exact graph self-audited with zero vulnerabilities or denied
  warnings. It then reported zero SchemaHub vulnerabilities and exactly the
  documented unmaintained `paste` warning plus inactive yanked `spin` lock
  entry. Deterministic gate tests reject a vulnerability, new/missing warning,
  malformed output, command failure, auditor-version drift, invalid source
  override, stale source archive, or tool-lock drift. Both frozen pnpm audits
  passed under policy, both production web builds passed, and the dependency
  policy contract rejected backend, version, allowlist, RSC-scope, auditor
  supply-chain, release-instrumentation tool, or CI-step drift (2026-07-29).
  The exact cargo-auditable 0.7.5 archive and 48-package lock passed the
  verified zero-warning audit, and the release matrix now forces an isolated
  install and exact binary invocation. The Node, Rust, and distroless
  multi-architecture manifests are now digest-pinned. The Dockerfile frontend,
  PostgreSQL integration service, and curl acceptance helper are pinned too,
  workflow GUI builds use Node 24.18.0 exactly, and pnpm cannot be overridden
  at image build time; the policy contract rejects drift in any coordinate and
  every non-local action without a 40-character commit. Actionlint 1.7.12 and
  an independent YAML parse accepted both
  workflows; ShellCheck accepted the new policy scripts. A fresh exact-base
  build produced local image digest
  `sha256:380c649e600fa39a90d43a9846fcc87d502ddbf51d3946ba4cb37a77fc6a69d7`;
  both embedded binaries contain cargo-auditable `.dep-v0`, the image runs as
  `65532:65532`, and the pinned-curl persistence acceptance passed across
  process replacement. A follow-up upstream audit confirmed released
  `utoipa-axum` 0.2.0 still uses `paste`, while the
  upstream `pastey` switch also requires Axum 0.8; the yanked `spin` package is
  absent from SchemaHub's all-feature tree and retained only by SQLx 0.9's
  supported facade lock closure. Neither warning justifies an in-tree framework
  or semver-exempt SQLx facade fork.
- [x] The AWS-LC graph compiled inside the hermetic Rust 1.95 builder with the
  PostgreSQL feature and ran in the non-root distroless image
  `sha256:6945243c617d971294b475e88097c668570653a88e7f5ba811dedb95baa6ad07`.
  The image served the locked GUI, persisted a schema and both immutable
  artifact kinds across container replacement, and reverified descriptor
  digest
  `sha256:25a7372f253d6a6b4d93d22eb78dd22943aecacca8dfd49f2ae5fc1ced7b5e9e`
  plus generated-code digest
  `sha256:c507375f3602e3c2bc0aecc761ef8cfb9b5e25a1b59fedae29d517f90e1e47c3`
  (2026-07-29).
- [x] All seven Tailscale-bound real-world codelabs passed against the hardened
  dependency graph. The normalized dirty-worktree report digest is
  `sha256:959ad689d8409363360be90874e032343103c7eaa0f874cef1dfb2835d1e8643`;
  its deterministic archive is
  `sha256:52b6750f64ea2785b98900c51f79f4a4ecacbb621d1b97c28f0f78dae7e5760e`.
  The release verifier rejected it as intended because only a clean pushed
  candidate can authorize release evidence (2026-07-29).
- [x] FlatBuffers main commit
  `59756d23993538b722f68675c35129c3cebb7aa1` and Actions run `30481753669`
  passed the default/all-feature release suites, formatting, and both strict
  production Clippy contracts. SchemaHub's lock validator resolves
  `flatc-rs-parser`, `flatc-rs-schema`, and `flatc-rs-codegen` from that exact
  canonical Git coordinate (2026-07-29).
- [x] The release workspace exposed and fixed `RV-CODEGEN-001`: a
  three-package Protobuf closure previously flattened generator modules and
  left `cloudbuild::...` references unresolved. Focused unit contracts now
  require the deterministic package tree, explicit multi-file root, and root
  re-export; the same real three-package output compiles in an isolated Cargo
  project. RW-06 additionally compiles old and new served closures for distinct
  `payments.capture.v1` and `payments.money.v1` packages inside versioned
  consumer modules (2026-07-29).
- [x] An earlier complete locked release workspace and strict
  all-target/all-feature
  Clippy passed against the published FlatBuffers pin plus the Protobuf package
  tree fix, before the dependency-hardening increment. All seven release-mode
  codelabs then passed over Tailscale with
  zero open findings at every severity; the normalized machine report digest
  is `sha256:2f5bb0063c3cf8ecd1a0103d1559164747abafd5298f2d09aeb241db124c2382`
  and its deterministic archive digest is
  `sha256:df6aaec38f864bb7143144f82bb9ef135e42b306e922ab99668c065e63111171`.
  The worktree is intentionally dirty, so the release verifier rejected the
  local archive as non-candidate evidence (2026-07-29).
- [x] Release-assembly contract tests accepted an exact three-file manifest and
  rejected tampered bytes, an extra unchecksummed file, a missing file, a
  duplicate checksum name, a parent-path checksum, and a nested entry.
  Workflow contracts require the exact-set verifier both immediately before
  upload and immediately after download, and record the `upload-artifact`
  SHA-256 output while the post-download exact-set check remains fail-closed
  (2026-07-29).
- [x] Container-tag promotion contract tests accepted creation from an exact
  already-pushed digest and an idempotent matching tag, while rejecting an
  unavailable candidate, a conflicting existing version tag, post-creation
  digest drift, and malformed versions. Release workflow parsing confirms the
  stable semantic-version tag is now downstream of the protected staging job
  and retained checksummed release assembly instead of being pushed by the
  pre-staging build (2026-07-29).
- [x] The reusable runtime-container acceptance passed against the corrected
  same-release image with a fresh Docker named volume. An authenticated
  non-root process created `acceptance/registry/user.proto`, materialized
  descriptor digest
  `sha256:25a7372f253d6a6b4d93d22eb78dd22943aecacca8dfd49f2ae5fc1ced7b5e9e`
  and generated-Rust digest
  `sha256:756b89439d576bbbe7fa7a1959bc8064ec1b3c8d6f063bbb120bd3a2454b7b59`,
  exited cleanly, and was replaced by a new container that recovered the exact
  schema coordinate and verified both immutable byte digests. The replacement
  also exited with code 0, and the test removed its containers, network, and
  volume (2026-07-29).
- [x] A live Pwright run through the current Tailscale browser data path exposed
  and verified the `RW-GUI-001` responsive-header fix at a 945-pixel viewport:
  the fixed header remained 55 pixels tall with every visible control inside
  its bounds. The same run verified the self-contained Protobuf source viewer
  has keyboard focus, 11 rendered lines, visible line numbering and horizontal
  overflow, and no third-party resource request. CI browser acceptance now
  enforces header geometry at 930-pixel and 390-pixel viewports (2026-07-29).
- [x] The same-release GUI distribution increment passed strict
  all-target/all-feature release Clippy and the complete 600-test optimized
  workspace. The production GUI entry is 419,109 bytes, and positive/negative
  bundle tests enforce its 450,000-byte ceiling plus runtime-CDN rejection.
  Native-package contracts passed. Distroless image
  `schemahub:bundled-gui-local`
  (`sha256:a6a65e766cb2d35988cacc9132077978f2f6e103442882177f634695508f44ac`,
  25,383,542 bytes) ran as UID/GID `65532`, became healthy, served the corrected
  GUI root, nested route, and immutable hashed asset from its Tailscale-bound
  HTTP listener, preserved a
  non-HTML unknown-BFF `404`, returned all three compiler capabilities, and
  drained on `SIGTERM` with exit code 0. The source remains dirty local
  rehearsal state and is not release provenance (2026-07-29).
- [x] The current SchemaHub source built through the hermetic Rust 1.95
  PostgreSQL-capable Docker path with the immutable compiler coordinates
  embedded as OCI labels. Local image
  `schemahub:local-goal-20260725`
  (`sha256:07513bdae4b1dc7ed59a34e84fa6d1a286d9a61983f6449aae7d017a454e6279`,
  25,025,337 bytes) ran as UID/GID `65532`, became healthy, served readiness
  and build metrics, returned the compiler capability matrix, reported the
  requested version from both binaries, and stopped on `SIGTERM` with exit
  code 0. Syft 1.48 then emitted a valid SPDX 2.3 document containing 457
  package records (446 Cargo, 10 Debian, and the OCI image), including every
  SchemaHub workspace crate and both compiler stacks; the local SBOM digest is
  `sha256:eb4c6ed3786f1f5404bc5c76861b8a7aa136e1510b589c23337cc56bea76122b`
  (2026-07-25).
- [x] An auditable host-target `0.9.0-rc.1` archive rehearsal passed the real
  packaging script. Both binaries report the requested version and contain
  `.dep-v0` dependency metadata; the embedded OpenAPI reports the same version
  with 22 paths/24 operations; and `BUILD-METADATA.txt` records the exact
  SchemaHub, Protobuf, and current pinned FlatBuffers revisions. The local
  archive digest is
  `sha256:673e5d6f39485df96aecb17003ba8d9d3442ea7697f1b59bfbd61caf772f1b49`.
  Because the source is dirty and the FlatBuffers follow-up is not pinned, this
  is packaging rehearsal evidence rather than a publishable candidate
  (2026-07-25).
- [x] The unpublished FlatBuffers follow-up passed formatting, the complete
  default and all-feature release workspaces, normal and all-feature
  production-target strict Clippy, all 24 generated-code compile tests (one
  additional case ignored by design), and the `#![deny(warnings)]` downstream
  codegen test. The final fix makes struct-union accessors compile through the
  fully qualified `Follow` trait and public `Table` accessors (2026-07-25).
- [x] An isolated coordinated SchemaHub build passed its 591-test release
  workspace, strict all-target/all-feature release Clippy, the exact
  22-path/24-operation OpenAPI contract, and all seven real-world codelabs.
  The GA report recorded seven passing normalized results and machine-report
  digest
  `sha256:4726f75cd9583780d0fba068e8227c5217849e893d27cc3f015a79e3baa05931`.
  GUI production build, demo typecheck/static build/workerd smoke, and all 25
  PostgreSQL integration tests also passed. The report correctly says
  `provenance_status: dirty` and `release_authorized: false`; it is local
  coordination evidence, not candidate authorization (2026-07-25).
- [x] A fresh GitHub audit confirmed draft PR 4 remains clean at pushed source
  `a8b3c5f29d6aa91f5cd0e4ab9ad5c4fea7b1e844`, whose six-job Actions run
  `30019762296` passed. Repository variables still name the immutable Protobuf
  and pre-follow-up FlatBuffers revisions, while environments, deployments,
  repository secrets, tags, and releases remain empty (2026-07-25).
- [x] The live GUI acceptance passed over the full Tailscale data path:
  delegated agent `gui-agent` authored an executable Protobuf ChangeRecord,
  pre-review Apply returned `412`, independent human `gui-owner` approved,
  the agent applied, the schema detail rendered `LiveBrowserRecord`, and the
  audit plus descriptor bytes/digest remained identical after redb restart
  (2026-07-24).
- [x] The GA reporter accepted a fresh seven-scenario release-mode run with
  zero open release-blocker/high findings and emitted normalized human/JSON
  evidence; contract tests rejected an injected high finding, a missing
  scenario, credential material, dirty candidate provenance, and a tampered
  normalized result (2026-07-24). The local report correctly records dirty
  development provenance, so clean candidate Actions evidence remains open.
- [x] Stable-staging contract tests accepted a complete matching attestation
  and rejected coordinate drift, development credentials, incomplete identity
  and product checks, stale evidence, and credential material; both CI and
  release workflows parse after the protected promotion gate was wired
  (2026-07-24).
- [x] The 1.0 release-note contract rendered exact source/compiler/image
  coordinates; negative tests rejected a missing staging section, a missing
  frozen boundary, and an unresolved marker. Release-deadline tests inject an
  open `RW-03-001` to prove 0.9 and 1.0 prereleases remain allowed while stable
  1.0 and later fail; the authoritative fixed ledger allows 1.0
  (2026-07-29).
- [x] Seven isolated release-mode codelabs passed against real redb servers:
  the guarded human/agent lifecycle with persisted bytes; Protobuf old/new
  decode plus breaking rejection; FlatBuffers defaults/deprecation plus
  restart identity; human/agent conflict resolution plus Apply replay;
  two-repository sidecar replay plus explicit rollback; multi-file dependency
  evolution/deletion; and private-tenant RBAC/search isolation (2026-07-24).
- [x] A temporary Cargo override proved the warning-clean FlatBuffers sibling
  against SchemaHub without weakening the immutable dependency contract:
  591 release-workspace tests passed, all generated-code compile fixtures used
  `#![deny(warnings)]`, the RW-05 legacy constructor migration was caught and
  fixed, and the complete seven-codelab GA gate then passed (2026-07-24).
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
- [x] GUI TypeScript checks, production Vite build, and real Chromium
  executable-edit smoke, plus locked Cargo metadata and rebuilt CLI/server
  release-binary smoke checks (2026-07-24).
- [x] HTTP BFF resource, identity, ChangeRecord lifecycle, search, conflict resolution, and artifact-cache integration tests (2026-07-21).
- [x] External-reference normalization/bounds/legacy decode, gRPC create/update,
  HTTP create/search, CLI help/JSON, and GUI production-build coverage
  (2026-07-21).
- [x] HTTP boundary tests cover canonical startup policy, trusted and unlisted
  origins, preflight headers, same-origin defaults, and pre-handler `413`
  rejection without mutation (2026-07-21).
- [x] Generated OpenAPI validation covers 22 path templates, 24 operations,
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
