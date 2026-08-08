<!-- agent-updated: 2026-07-30T04:23:54Z -->
# Changelog

All notable user-facing changes are recorded here. SchemaHub has not yet
published the 0.9 release candidate; current entries remain under Unreleased.

## Unreleased

### Added

- Durable human, delegated-agent, and service `ChangeRecord` intent with typed
  edits, validation, review, conflict resolution, idempotent Apply, and
  commit/operation audit links.
- Repository-scoped creation-order and lifecycle-status indexes for
  `ListChanges`. Creates and status transitions update them atomically,
  pre-index records are backfilled once behind a durable marker, and every
  public page is a bounded redb/PostgreSQL range read with fail-closed target
  validation while preserving existing v1 page tokens.
- Ordered, bounded external issue/incident/design references on ChangeRecords,
  shared by gRPC, CLI JSON/text, browser creation/detail, and search.
- Immutable repository-scoped revisions plus source, descriptor, and generated
  artifacts with closure/payload digests and HTTP/gRPC cache validation.
- Durable versioned first-materialization records: the first successful source,
  descriptor, or generated-code bytes win atomically and remain byte-identical
  across restarts and compiler/printer upgrades.
- Persistent projects, memberships, repositories, policies, ETags, pagination,
  archive behavior, redb restart support, and PostgreSQL transactions.
- Active/all project catalogs and active/all repository catalogs partitioned
  by project. Resource creation and archive transitions maintain catalog
  entries atomically, pre-index resources backfill once behind durable
  markers, and public project/repository pages use bounded prefix range reads
  with fail-closed target validation while preserving existing v1 page tokens.
- Bounded `ListMembers` pagination and
  `schemahub project member list [--json]`. Pages traverse only the requested
  project's existing identity-ordered role keys, use project-bound opaque
  tokens, advance through inactive tombstones, and fail closed on malformed
  scoped records. GUI summaries now read only the caller's role.
- Bounded `ListBranches` and `ListTags` responses with opaque continuations
  bound to the ref kind, project, repository, and prefix. Each page lazily
  materializes at most `page_size + 1` names from the repository-local ordered
  JJ view, both CLI list commands follow every page, and `GetBranch` now uses a
  direct named lookup.
- Bounded GUI BFF project and repository pages over the existing Core catalog
  indexes, with continuations bound to catalog kind, project scope, and name
  prefix. The React console loads pages incrementally, deep repository links
  use one bounded exact-prefix lookup, and project summaries no longer perform
  an N+1 full repository scan to calculate counts.
- Bounded repository-dashboard and browser ChangeRecord pages. One
  repository/ref-bound dashboard token advances schema, branch, and tag
  cursors together, pins schema summaries and conflict counts to the first
  immutable commit, and avoids materializing the complete conflict namespace.
  Selected schema objects and the repository-local name inventory now load in
  one tree traversal, eliminating the remaining per-schema full-tree scans
  while retaining compiler validation and unique declared-import counts.
  Browser ChangeRecord tokens bind repository and lifecycle status while
  reading the existing atomic Core index; the console requests both
  continuations explicitly.
- Remote GUI/demo browser smokes now normalize Chrome's loopback
  `webSocketDebuggerUrl` onto the configured neutral Tailscale CDP host, with
  fail-closed resolver tests for HTTP, HTTPS, and direct WebSocket endpoints.
- Live GUI acceptance now targets the identity button's exact accessible name
  and closes both local and remote browser connections, so the governed
  author/review/Apply/restart run terminates cleanly over remote CDP.
- Immutable, project-partitioned control-plane audit events for every
  project/member/repository mutation. Resource state and its typed before/after
  event commit atomically across memory, redb, and PostgreSQL; Owners can page
  them through gRPC or `schemahub project audit [--json]`. An immutable
  newest-first index makes each page a bounded backend range read and fails
  closed on malformed cursors or corrupt event/index relationships.
- Project-keyed cross-instance coordination for administrative mutations,
  closing the concurrent last-Owner removal/downgrade race while keeping
  different projects independent on PostgreSQL.
- Versioned Protobuf, FlatBuffers, and OpenAPI capability/conformance workflows.
- Live GUI workflows and stable CLI JSON/errors/exit codes for agents and CI.
- Direct GUI authoring for executable whole-schema source replacements and
  schema deletions, including note-to-executable draft conversion, exact-format
  validation, ETag-protected edit replacement, and validation invalidation.
- Version-matched production GUI assets in every native archive and release
  container, with fail-fast same-origin serving, deep-link routing, immutable
  hashed-asset caching, strict separation from unknown BFF routes, and
  symlink-free tree validation that prevents static-file root escape. Unhashed
  assets now receive `no-cache` instead of an incorrect one-year immutable
  policy. Successful console responses also enforce a self-only content
  security policy, deny framing and privileged browser features, prevent MIME
  sniffing, and restrict referrers to the same origin.
- Live Chromium acceptance against the real HTTP BFF and redb server for
  delegated-agent source authoring, rejected pre-review Apply, independent
  human approval, agent Apply, schema rendering, durable audit identity, and
  byte-identical descriptor serving after restart.
- JSON structured events, request correlation, Prometheus metrics, HTTP and
  gRPC health, storage-aware readiness, and bounded graceful shutdown.
- Embedded PostgreSQL migrations, bounded query execution, distributed GC
  fencing, backup/restore drills, and cross-repository GC safety.
- Pinned GitHub CI/release workflows, tagged binary archives, a non-root
  multi-architecture distroless container, checksums, provenance, and auditable
  Rust SBOMs. Version-specific release notes now fail closed unless they state
  the migration, mixed-version, rollback, compatibility, known-issue, and exact
  multi-architecture image-digest contract.
- A checksummed, reproducible RustSec auditor bootstrap that replaces
  cargo-audit 0.22.2's stale published lock with a repository-reviewed lock,
  installs that exact graph, and self-audits it before auditing SchemaHub.
- A release-tool gate that verifies cargo-auditable 0.7.5's exact source and
  clean published lock before every tag build, then forces an isolated install
  and invokes that exact binary for release instrumentation.
- Exact multi-architecture manifest pins for the Node GUI builder, Rust 1.95
  builder, and non-root distroless runtime, with a CI contract that also rejects
  a mutable Dockerfile frontend, overridable pnpm coordinate, mutable
  Node workflow selector or PostgreSQL/curl acceptance helpers, or
  non-commit-pinned external GitHub actions.
- Production external JWT authentication with explicit issuer/audience/type and
  asymmetric algorithms, bounded HTTPS/file JWKS loading, atomic rotation,
  human/agent/service claims, deterministic time tests, and stale-key
  fail-closed readiness.
- Generated OpenAPI 3.1 documentation sourced from the live HTTP handlers,
  available through `/api/openapi.json`, `schemahub-server --print-openapi`,
  and every native release archive.
- Public bounded `ExplorationService.ListDependents` discovery and
  `schemahub schema dependents [--json]`, with direct pinned/live import edges,
  Core Read filtering, and per-repository immutable snapshot manifests.
- Exact field/property type traversal across local, live cross-repository, and
  pinned imports, with populated declaration details and immutable source/target
  coordinates.
- Normalized direct/transitive forward dependency edges with importing and
  target commits, explicit pin/path state, unresolved external leaves, and a
  fail-closed 10,000-schema-snapshot bound.
- Supported external OpenAPI schema, parameter, response, and request-body
  component `$ref` values as live dependency edges, including canonical
  round-trip, relative-path normalization, forward/reverse discovery, exact
  property following, immutable closure serving, deletion guards, and public
  gRPC snapshot acceptance after a provider advances.
- A runnable human-and-agent workflow codelab plus an interactive
  Protobuf/FlatBuffers companion site. The site walks the guarded
  ChangeRecord-to-artifact lifecycle, explains data-side revision/digest
  pinning, states the 1.0 boundaries, and indexes a real-world validation
  portfolio for finding reproducible product defects.
- Seven executable real-world codelabs with isolated release-mode servers,
  deterministic fixtures, generated Rust producers/consumers, negative policy
  cases, restart/rollback checks, normalized evidence, and one-command CI
  coverage for commerce, mobile telemetry, concurrent editing, and
  batch/stream handoff workflows, plus the primary human/agent lifecycle,
  multi-file dependency closure, and private-tenant isolation.
- A fail-closed GA readiness report with an authoritative structured finding
  ledger, exact seven-scenario completeness checks, secret rejection,
  normalized result digests, source/run provenance, clean-candidate
  enforcement, CI retention, and release-asset assembly.
- A stable-release staging attestation contract and codelab that bind
  production-like PostgreSQL, real-provider identity, artifact durability,
  dependency visibility, and restore evidence to the exact source, image
  digest, and GA-readiness archive.
- A version-specific SchemaHub 1.0 release contract that freezes the public
  API/BFF boundary, states the supported limitations, and records the stable
  staging and immutable provenance requirements.

### Changed

- The live `PROTOBUF_RS_REF` and `FLATBUFFERS_RS_REF` GitHub repository
  variables now match the exact immutable compiler revisions in `Cargo.lock`,
  removing the remaining compiler-coordinate configuration drift before a
  clean candidate run.
- JWT signature verification now uses jsonwebtoken's AWS-LC backend. Release CI
  requires zero Rust vulnerabilities and exactly the reviewed warning set, then
  audits both frozen web graphs at Low severity; policy contracts prevent
  backend, patched-version, audit-step, or advisory-allowlist drift.
- Operator-console pages now load as independent route chunks, reducing the
  initial production JavaScript payload and enforcing its size budget in CI.
- Read-only source and generated-code views are now fully self-contained,
  preserving line numbers, selection, and horizontal scrolling without
  fetching Monaco or another runtime CDN asset.
- The operator header now stays within its fixed height at tablet and mobile
  widths; search, environment, mode, and identity controls shrink or hide at
  explicit breakpoints instead of wrapping over page content.
- Compiler import discovery now receives complete schema objects, allowing
  metadata imports and declaration-level references to share one
  format-agnostic Core contract.
- Multi-file code generation now uses an explicit closure root; Protobuf named
  types resolve across imported/nested files and FlatBuffers root selection no
  longer depends on map ordering.
- Release-tag versions are reported consistently by binaries, health payloads,
  gRPC server configuration, Prometheus build metadata, archives, and OCI
  labels.
- API code generation uses a vendored platform-specific `protoc`, removing the
  host package dependency from clean builds.
- Server configuration and operations surfaces distinguish `noop`,
  `static-bearer-rbac`, and `jwt-rbac`; `/readyz` now reports authentication
  key freshness in addition to storage and lifecycle state.
- The distroless container health check now consumes aggregate `/readyz`
  status, including database and JWT-key freshness, through the bounded native
  `schemahub-server --check-ready` probe.
- Version-tag publication now depends on the complete reusable CI workflow and
  fails before building when compiler coordinates are mutable or archive
  provenance is missing.
- Stable version-tag publication now also waits on the protected
  `schemahub-production-staging` environment, validates a fresh exact-digest
  attestation, and includes the normalized attestation in release checksums and
  assets. The attestation includes same-origin bundled-GUI serving from the
  exact image. The gate independently reads the environment's deployment
  policies and rejects anything other than one `v*.*.*` release-tag policy.
  Prerelease tags remain deployable candidates and skip this promotion gate.
- Findings can declare a stable `must_fix_before` deadline. Release metadata
  permits prereleases at the deadline but rejects that stable version and every
  later release while the finding remains open; the warning-clean FlatBuffers
  pin exercised that enforced 1.0 deadline before its resolution.
- Release container builds now push only a run-unique candidate tag before
  stable staging. The semantic-version image tag is created from the accepted
  digest only after protected approval and complete checksummed release
  assembly, refuses conflicting existing tags, and must verify before the
  GitHub release can publish.
- Release assembly transfer now retains the Actions artifact digest and
  verifies the safe exact file set plus every `SHA256SUMS` entry both before
  upload and after download, preventing extra unchecksummed files from reaching
  the GitHub release.
- The Protobuf and FlatBuffers compiler boundaries now resolve directly from
  immutable upstream commits
  `a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac` and
  `59756d23993538b722f68675c35129c3cebb7aa1`. Normal CI, Cloud Build, and the
  runtime-image build are hermetic SchemaHub checkouts; compiler-lock and tag
  gates reject every cross-repository path or mutable dependency.
- The browser HTTP boundary is same-origin by default. Operators can opt into
  exact trusted origins, and every JSON request is subject to a validated,
  configurable body-size ceiling.
- Whole-schema create/update/delete now share Core policy with granular and
  ChangeRecord writes: explicit format matching, truthful existence errors,
  protected compatibility, Maintainer-only audited force, immutable planning
  bases, and live-unpinned import protection.
- Every repository publisher now holds a backend-level publication guard from
  operation-head load through exact-final-tree validation and JJ commit.
  Protected conflicts and consumer/provider deletion races fail before an
  operation is published; PostgreSQL coordinates the guard across instances.
- `ApplyTransaction` now runs synchronous compiler/storage work on the blocking
  executor and enforces the advertised 30-second server deadline. A shared
  monotonic cancellation token is rechecked inside the atomic publication gate;
  pre-publication expiry releases the pending idempotency receipt.
- The 1.0 API boundary is now explicit: `schemahub.v1` gRPC/protobuf is the
  public compatibility surface, while unversioned `/api/*` is a same-release
  GUI BFF. Runtime responses and per-path OpenAPI metadata identify the BFF;
  health, readiness, and metrics remain separate supported operations routes.
- Repository-local exploration, history, diff, and codegen calls now resolve a
  mutable ref once and report the exact immutable commit(s) used. Omitted refs
  honor repository configuration, and commit streams apply their documented
  stop/schema filters with snapshot metadata and finite bounds.
- Bounded operation-log reads now walk only the requested recent suffix on the
  normal linear JJ history. Branched histories retain the complete traversal's
  ordering and deduplication semantics.

### Fixed

- Native release archives are now byte-reproducible for identical platform
  inputs. HTTP discovery, CLI output, and packaging share canonical OpenAPI
  bytes; tar membership and metadata plus gzip headers are normalized; and
  every release platform packages twice before upload.
- Release-package archive assertions no longer race `tar` against an
  early-exiting `grep` under `pipefail`, eliminating intermittent SIGPIPE
  status 141 failures.
- Removed the vulnerable RustCrypto RSA path and vulnerable or unsound
  `crossbeam-epoch`/`anyhow` locks. Patched Vite, esbuild, PostCSS, Next.js, and
  Sharp versions replace the vulnerable web resolutions; the remaining React
  Router advisory is restricted to server-side RSC APIs that SchemaHub does not
  import.
- GC no longer deletes globally deduplicated objects that remain reachable from
  another repository.
- PostgreSQL calls no longer create one operating-system thread per query, and
  concurrent compare-and-swap retries preserve every successful increment.
- Change application recovers the post-JJ/pre-receipt crash window without
  publishing twice, including after a redb process restart; concurrent server
  instances elect one apply lease and converge on one receipt.
- An explicitly supplied server config path no longer falls back to anonymous
  defaults when it is missing or unreadable; startup fails closed.
- Nested schema paths are preserved by schema discovery, and concurrent direct
  mutations/transactions use the commit they actually validated instead of
  re-resolving a moved bookmark and overwriting the racing edit.
- Top-level declaration removal is now rejected by protected-bookmark
  compatibility unless an authorized force is used.
- Pre-publication policy rejection now releases direct-write idempotency
  receipts and ChangeRecord Apply leases immediately, while ambiguous storage
  failures retain their correlation state for recovery.
- Raw commit references can no longer read or publish objects through a
  different repository's coordinates, even though the backend deduplicates
  content globally.
- ChangeRecords with no target now use the repository's configured default
  bookmark; malformed HTTP Authorization bytes now return `401` instead of
  falling through to anonymous access.
- Every CLI command now has process-level bearer-token coverage against an
  authenticated server. Unreadable or malformed CLI config fails closed, and a
  missing server coordinate no longer silently targets a loopback endpoint.
- Persisted JJ commits, trees, views, and operations now reject missing fields,
  malformed object IDs, unsupported submodules, and backend faults instead of
  panicking, synthesizing IDs, or reporting storage failures as absence.
- OpenAPI parsing now rejects malformed declaration shapes, keys, references,
  parameter locations, and JSON Schema types instead of silently storing empty
  names/default declarations.
- Monolithic Protobuf Rust generation no longer flattens distinct package
  modules and leaves fully qualified dependency types unresolved. SchemaHub now
  emits a deterministic nested module tree for the exact closure, imports only
  resolved cross-package roots, and re-exports the requested root package.

### Release blockers

- Publish the current SchemaHub tree containing the immutable FlatBuffers pin
  and obtain a clean candidate Actions run for the exact coordinates.
- Configure the protected staging environment and exact release-tag policy,
  deploy the candidate digest with the intended external issuer, and retain the
  real-provider acceptance attestation.
- Run the explicitly authorized 0.9 candidate and 1.0 publication gates.
