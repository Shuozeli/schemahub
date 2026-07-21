<!-- agent-updated: 2026-07-21T20:46:16Z -->
# Cross-Repository Dependency Discovery

SchemaHub exposes two deliberately different dependency reads:

- `ListDependencies` walks forward from one schema at one requested ref and is
  used to understand or build that schema's closure.
- `ListDependents` scans backward across repositories visible to the caller and
  is used to coordinate downstream changes.

Neither method maintains a durable reverse index or rewrites another repository
automatically.

## Forward Closure Contract

`ExplorationService.ListDependencies` resolves the root ref once and returns
that `at_commit`. Each direct or transitive edge includes:

- importing project, repository, schema, and exact commit;
- the compiler-stored logical `import_path` and imported declaration;
- normalized target project, repository, and schema;
- the stored `resolved_commit` and `pinned` state;
- the effective immutable `target_commit`; and
- `resolved`, which distinguishes a usable target from an unavailable leaf.

Traversal keys are `(schema, commit)`, so distinct historical versions are not
collapsed. A same-repository live import stays at its importing commit. A live
cross-repository import resolves the target repository's configured default
bookmark once per call and reuses that snapshot. A pin is ownership-checked
against its target repository and never drifts.

Unreadable, archived, absent, or builtin external targets remain explicit
`resolved=false` edges and are not traversed; this preserves what the source
actually imports without disclosing unreadable contents. Invalid or foreign
pins, corrupt storage, compiler-decoding failures, unknown formats, and a
closure above 10,000 schema snapshots fail the entire call. A successful
response is therefore complete within its declared visibility and bounds.

The 1.0 forward graph is authoritative for compiler-reported imports:
Protobuf `import`, FlatBuffers `include`, and external OpenAPI component `$ref`
values represented by the selected 1.0 AST. The compiler import boundary
receives the complete schema object set, so declaration-level OpenAPI refs are
included alongside metadata-level imports. Supported OpenAPI source refs use
`<schema-path>#/components/{schemas|parameters|responses|requestBodies}/<name>`
and are live/unpinned. Network URLs, absolute paths, query-bearing refs,
arbitrary fragments, `$ref` sibling fields, and standalone reference shapes the
selected AST cannot preserve fail at ingest instead of becoming misleading
registry edges or losing constraints. Other OpenAPI component categories are
outside the 1.0 dependency guarantee.
Explicit `./` and `../` paths resolve against the importing schema directory;
traversal beyond the repository root fails closed.

## Reverse Discovery Contract

`ExplorationService.ListDependents` accepts one logical target:

```text
project + repo + schema_path
```

It returns:

- every direct import edge to that target found in repositories visible to the
  caller;
- the importing schema, its configured default bookmark, and the exact commit
  inspected;
- the stored import path and imported declaration;
- whether the import is immutable (`pinned=true` plus `resolved_commit`) or
  live/unpinned;
- a manifest of every visible, non-empty repository snapshot inspected; and
- the total number of schema files scanned.

Results are sorted by importing project, repository, schema, declaration, and
pin. The operation is direct-only. A transitive reverse graph would have to
honor each intermediate import's historical pin, so 1.0 does not pretend that
walking current default bookmarks is an equivalent answer.

## Snapshot and Concurrency Semantics

The service first captures the ObjectDb repository inventory. For each visible
repository it then:

1. reads the repository's configured default bookmark;
2. resolves that bookmark once to an immutable commit;
3. reads every schema and import from that commit; and
4. records the bookmark/commit pair in the response manifest.

Every individual repository result is internally consistent and reproducible
from its returned commit. There is no atomic instant shared by all repositories:
a bookmark can move before or after its own snapshot, and a new repository can
appear after inventory capture. Callers should persist the manifest with a
planned migration and rerun discovery immediately before coordinated
publication when freshness matters.

SchemaHub does not provide a cross-repository transaction. The caller creates
and applies explicit ChangeRecords in each downstream repository. Immutable
pins remain safe when a provider's mutable bookmark changes or removes a
schema; live/unpinned edges require coordination and can race an independent
publisher. For data that must remain decodable, store immutable revision and
artifact digests and prefer pinned cross-repository imports.

## Authorization and Completeness

The caller must be able to read the target repository. Candidate repositories
are checked against the same Core `Read` policy as every other schema API.
Unreadable and archived repositories are omitted from both edges and the
snapshot manifest, so no private resource name is leaked.

Consequently, “no dependents” means no direct dependents in the repositories
visible to that identity at the returned snapshots. Organization-wide release
automation should use a narrowly scoped inventory identity that can read every
repository whose imports it governs.

## Bounds and Failure Behavior

The 1.0 implementation is an authoritative bounded scan, not an eventually
consistent index:

- at most 1,000 visible repositories;
- at most 10,000 schema files; and
- no partial success when either limit, storage, compiler decoding, or snapshot
  loading fails.

Limit failures return `RESOURCE_EXHAUSTED`. The limits are advertised by
`AdminService.GetServerConfig`. The server performs the synchronous ObjectDb
work on its blocking executor so the async gRPC runtime remains responsive.
A future durable reverse index may optimize this operation, but it must preserve
the same authorization, snapshot-manifest, sorting, and fail-closed semantics.

## CLI

Human-readable output:

```bash
schemahub schema dependents acme/provider/types.proto
```

Stable agent/automation output:

```bash
schemahub schema dependents acme/provider/types.proto --json \
  | jq '{target, schemasScanned, snapshots, dependents}'
```

Automation should inspect `pinned`, retain `importingCommit`, and treat the
snapshot manifest as part of the decision record rather than consuming only
the list of schema names.

## Non-Goals for 1.0

- automatic downstream source rewriting;
- a global transaction or lock spanning repositories;
- disclosure of unreadable private repositories;
- transitive reverse traversal with historical-pin semantics; and
- project-wide declaration search (which remains separate from dependency
  discovery).
