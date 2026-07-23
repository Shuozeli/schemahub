<!-- agent-updated: 2026-07-23T03:23:20Z -->
# Changelog

All notable user-facing changes are recorded here. SchemaHub has not yet
published the 0.9 release candidate; current entries remain under Unreleased.

## Unreleased

### Added

- Durable human, delegated-agent, and service `ChangeRecord` intent with typed
  edits, validation, review, conflict resolution, idempotent Apply, and
  commit/operation audit links.
- Ordered, bounded external issue/incident/design references on ChangeRecords,
  shared by gRPC, CLI JSON/text, browser creation/detail, and search.
- Immutable repository-scoped revisions plus source, descriptor, and generated
  artifacts with closure/payload digests and HTTP/gRPC cache validation.
- Durable versioned first-materialization records: the first successful source,
  descriptor, or generated-code bytes win atomically and remain byte-identical
  across restarts and compiler/printer upgrades.
- Persistent projects, memberships, repositories, policies, ETags, pagination,
  archive behavior, redb restart support, and PostgreSQL transactions.
- Versioned Protobuf, FlatBuffers, and OpenAPI capability/conformance workflows.
- Live GUI workflows and stable CLI JSON/errors/exit codes for agents and CI.
- JSON structured events, request correlation, Prometheus metrics, HTTP and
  gRPC health, storage-aware readiness, and bounded graceful shutdown.
- Embedded PostgreSQL migrations, bounded query execution, distributed GC
  fencing, backup/restore drills, and cross-repository GC safety.
- Pinned GitHub CI/release workflows, tagged binary archives, a non-root
  multi-architecture distroless container, checksums, provenance, and auditable
  Rust SBOMs. Version-specific release notes now fail closed unless they state
  the migration, mixed-version, rollback, compatibility, known-issue, and exact
  multi-architecture image-digest contract.
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

### Changed

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
- The Protobuf and FlatBuffers compiler boundaries now resolve directly from
  immutable upstream commits
  `a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac` and
  `7dc2c76c08f452b9a208230057c0cb6327e65f24`. Normal CI, Cloud Build, and the
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

### Release blockers

- Run the 0.9 candidate and 1.0 acceptance/publishing gates.
