<!-- agent-updated: 2026-07-30T04:16:42Z -->
# SchemaHub Compatibility Policy

This policy defines what SchemaHub intends to freeze at 1.0 and what remains
allowed during the 0.x release-candidate period. It covers the public API,
stored data, immutable serving, CLI automation, and operational interfaces.

## Release Status

- `0.x` releases are previews. Breaking changes are allowed when documented in
  release notes and accompanied by a migration or explicit reset procedure.
- `0.9.x` release candidates are the API and storage freeze rehearsal. A
  candidate is not promoted to 1.0 while any acceptance blocker in
  `docs/tasks.md` remains open.
- Beginning with `1.0.0`, SchemaHub follows semantic versioning. Breaking
  public-contract changes require a new major version.

The Cargo package versions are implementation coordinates. Published binaries,
health/metrics metadata, archives, and container labels use the immutable
release-tag version injected by the release workflow.

## Public gRPC and HTTP APIs

The `schemahub.v1` protobuf package is the designated public API and becomes
stable at 1.0. The unversioned `/api/*` routes are a GUI-only BFF and are
explicitly excluded from the 1.x public API compatibility promise. They are
supported with the bundled GUI from the same release, carry
`x-schemahub-api-surface: gui-bff`, and may change together with that GUI.
Their generated OpenAPI document remains an exact-build contract and release
artifact, not a claim that the BFF is public REST v1.

Every native archive and release container carries that exact locked GUI.
Its `/`, `/projects/*`, `/admin`, favicon, and hashed-asset routes are
distribution surfaces rather than public APIs: HTML is revalidated, hashed
assets may be cached immutably, and unknown `/api/*` requests must remain BFF
errors instead of becoming SPA HTML.

The co-located `/healthz`, `/readyz`, and `/metrics` routes are supported
operational interfaces, not BFF routes. Any future public REST API must be
explicitly versioned, follow the project's accepted resource/method rules,
publish a separately identified OpenAPI contract, and receive an explicit
compatibility declaration. See ADR 0002.

Within a major version, SchemaHub may add methods, message fields, enum values,
optional query parameters, and new resource subcollections. It will not:

- reuse a protobuf field number or change an existing field's meaning;
- remove or rename a public method, field, or resource-name shape;
- change an existing success response into a long-running operation without a
  new method/version;
- silently broaden authorization or weaken repository policy;
- change documented idempotency scope or ETag precondition behavior.

Clients must tolerate unknown protobuf fields and enum values. New required
behavior uses a new method, a new resource version, or a new API major version.
The same additions/removals rule does not apply to GUI-only BFF routes; BFF
changes must instead update the generated document, bundled GUI, tests, and
release notes in the same release.

The current same-release pair represents project/repository navigation,
repository dashboards, and ChangeRecord lists as bounded page DTOs. Catalog
continuations bind kind/project/prefix; dashboard continuations bind
repository/ref and pin schema pages to the first immutable commit; proposal
continuations bind repository/status and consume the public Core index. These
are scalability invariants of the shipped 1.0 console, not a promise that
those unversioned JSON field names or token encodings remain unchanged across
1.x; server and GUI must change together. The same-release dashboard also
batch-loads selected schema objects and repository-local names so its tree
traversal count does not scale with the number of rows in a page.

Repository-local v1 reads preserve explicit branch/tag/commit meaning, resolve
mutable refs once, and expose the exact immutable commit used. Omitted refs use
the repository's configured default bookmark, and raw commits are scoped to the
named repository's retained history. The response commit fields and
`ListCommits` initial `x-schemahub-at-commit` metadata are part of that
observable snapshot contract.

`ExplorationService.ListDependencies` preserves compiler-reported import edges,
stored pin state/path, immutable importing and effective target commits, and
explicit unresolved leaves. Its 10,000-schema-snapshot traversal is fail-closed.
The 1.0 forward graph covers Protobuf imports, FlatBuffers includes, and
external OpenAPI component `$ref` values represented by the selected 1.0 AST.
Those OpenAPI source refs are live logical SchemaHub paths; arbitrary remote URI
resolution and component categories outside that AST are not compatibility
promises.

`ExplorationService.ListDependents` is part of that public gRPC contract. Its
1.0 semantics are direct edges, Core Read visibility filtering, explicit
pinned/live state, deterministic ordering, per-repository immutable snapshots,
and fail-closed 1,000-repository/10,000-schema bounds. A successful result does
not promise a globally atomic snapshot, transitive reverse closure, automatic
downstream rewriting, or a cross-repository transaction. Compatible 1.x
implementations may optimize the scan with an index only if they preserve those
observable authorization, completeness, and snapshot semantics.

`ProjectService.ListControlPlaneAuditEvents` is the public Owner-only
administrative history contract. Event resource names, server-derived actor
and time, action meanings, typed before/after snapshots, newest-first order,
and project-bound opaque cursor semantics are stable in 1.x. These events are
immutable evidence; they do not promise administrative undo. Each public page
uses a bounded ordered-index range read. Every returned index record must be
well formed and resolve to a matching immutable event; a missing target,
mismatch, malformed record, or nonempty event collection with no index fails
closed. Cursor contents and audit index keys are internal storage details:
clients must return page tokens unchanged, must not synthesize them, and must
not treat them as durable event identifiers.

`ChangeService.ListChanges` preserves ascending creation-time/name order,
optional status filtering, the default/maximum page sizes, and
parent/filter-bound opaque continuation semantics. Each page uses a bounded
repository/status-index range and validates returned index relationships.
Cursor encoding and storage index keys remain internal; clients must return
tokens unchanged. A compatible 1.x implementation may replace the index layout
only if it preserves those observable results and bounds.

`ProjectService.ListProjects` and `ListRepos` preserve stable name order,
prefix/archive filters, default/maximum page sizes, and filter-bound opaque
continuation semantics. Repository results remain scoped to the requested
project. Project authorization filtering is scan-bounded, so a successful page
may contain no resources while returning a continuation token; clients must
continue until that token is empty. Each page uses a bounded catalog range and
validates every target. Catalog key and cursor encodings are internal and may
change in compatible 1.x implementations that preserve these observable
semantics and bounds.

`ProjectService.ListMembers` preserves ascending opaque-identity byte order,
project scoping, default/maximum page sizes, and project-bound opaque
continuations. Inactive tombstones are not returned, but a bounded
tombstone-only page may carry a continuation; clients must continue until it
is empty. A page validates only its bounded scoped primary-key range and fails
closed on malformed records or tokens. Existing clients compiled before the
pagination fields must be updated to follow `next_page_token` when a project
can contain more than the default page size.

`RefService.ListBranches` and `ListTags` preserve ascending ref-name order,
their existing prefix filter, default/maximum page sizes, and opaque
continuations bound to ref kind, project, repository, and prefix. A client must
continue until `next_page_token` is empty and must not interpret token bytes.
Each response materializes at most one page plus lookahead from the
repository-local immutable JJ view. Existing clients compiled before the
pagination fields must be updated to follow continuations when a ref namespace
can exceed the default page size.

## Stored Data and Migrations

Redb and PostgreSQL state created by a supported 1.x release must be readable by
later 1.x releases after documented forward migrations. Migrations are
append-only and checksum-verified. SchemaHub uses expand/migrate/contract
ordering and declares the mixed-version and rollback window in release notes.

Database rollback is forward-only by default. If the prior binary cannot read
an applied schema, restore a verified pre-upgrade backup into a new database
and cut over after validation. Never delete migration ledger rows or apply
destructive down SQL to the only live copy.

JJ commit IDs, change IDs, operation history, immutable tag targets,
ChangeRecord-to-commit links, repository-scoped revision names, and
control-plane audit-event resource names are durable data contracts.

The `schemahub.change_record_index.v1` collections and migration marker are
internal derived state, not public identifiers. Upgrading a pre-index database
performs a one-time atomic backfill before the first ChangeRecord ledger
operation completes. Mixed old/new processes are unsupported because the old
binary cannot maintain those indexes; rollback therefore restores the verified
pre-upgrade backup rather than running the candidate against migrated live
state.

The `schemahub.project_index.v1` and
`schemahub.repository_index.v1` collections and their migration markers are
also internal derived state. A pre-index database receives one atomic,
validated backfill before its first project/repository catalog operation.
Project/repository creates and archive transitions must thereafter update
resource and relevant catalog entries atomically. Mixed pre-index/index-aware
processes are unsupported for the same reason; rollback restores the
pre-upgrade backup.

Membership pagination derives its order directly from the existing
`schemahub.project_roles.v1` project/hex-identity primary keys and therefore
adds no stored-data migration. That physical key and cursor encoding remain
internal; compatible 1.x implementations may replace them if they preserve the
observable order, project isolation, tombstone, authorization, and bounded-page
semantics.

## Schema and Artifact Stability

A `SchemaRevision` permanently identifies one retained repository commit. The
canonical `schemahub-closure-v1` digest is stable for equal stored schema input.
An artifact's SHA-256 digest always identifies its exact response bytes, and
clients should persist and verify it.

SchemaHub closes the cross-release renderer gap with durable
first-materialization storage. Before any artifact response succeeds, its exact
bytes and verified metadata are atomically inserted into the versioned
`schemahub.artifacts.v1` collection. Concurrent or mixed-renderer instances use
first-writer-wins semantics. Every later request with the same canonical
identity returns the stored bytes, including after restart or a
compiler/printer upgrade; a corrupt stored record fails closed and is not
silently rerendered.

Canonical identity includes the revision, schema path, artifact kind,
generated-code language, and relevant codegen options. Adding an input that can
change output requires a new identity field or request-key version. An
intentional new rendering of the same schema therefore uses a distinct request
identity instead of replacing bytes already served.

An artifact name returned by a 1.x server continues to identify the same bytes
throughout the supported retention window. Artifact records are currently kept
with the database and included in backup/restore; JJ garbage collection does
not sweep them. Operators still persist and verify both revision and artifact
digest at deployment boundaries. Servers predating this storage contract are
not supported participants in a rolling upgrade or downgrade window.

## Format and Code-Generation Contract

`GetFormatCapabilities` is the executable source of truth. A 1.x server will
not remove a supported operation or language from the 1.0 matrix. New
operations may be added. Behavior currently advertised as rejected remains an
explicit structured error rather than a silent no-op.

Compatibility classifications may become more precise, but a change that was
accepted by a protected repository under the same stored policy will not be
retroactively rewritten. Release notes identify any rule correction that could
change future decisions.

## CLI and Automation

Human-formatted CLI output may improve within a major release. Commands marked
with `--json`, JSON error envelopes, resource names, digest fields, and the
documented agent/CI exit-code classes are stable automation contracts. New JSON
fields may be added; existing fields are not removed or repurposed in 1.x.

## Operations and Telemetry

`/healthz`, `/readyz`, standard gRPC health, graceful-drain ordering, and the
documented Prometheus metric names are supported operational interfaces.
Additional labels or metrics may be added. Labels are kept low-cardinality;
credentials and schema source never become labels or log fields.

Structured event field additions are compatible. Removal or semantic changes
to documented audit/correlation fields require release-note notice and at least
one minor-release deprecation window.

## Deprecation and Support Window

A 1.x public feature is deprecated in documentation and runtime metadata before
removal in the next major version. Security fixes may disable demonstrably
unsafe behavior sooner, with a release-note migration path. Each release note
states supported upgrade origins, storage requirements, known mixed-version
limits, and rollback procedure.
