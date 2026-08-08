<!-- agent-updated: 2026-07-30T04:16:42Z -->
# Durable Projects, Repositories, and Policy

This document describes the D3 control-plane resources as implemented. Project
and repository metadata are mutable resources stored beside ChangeRecords in
the selected redb or PostgreSQL database. Schema state remains immutable JJ
objects and history.

## Resource hierarchy

```text
projects/{project}
├── members/{identity}                  Role: Reader..Owner
├── auditEvents/{event}                 immutable administrative event
└── repos/{repo}
    ├── changes/{change}                durable intent and review record
    └── revisions/{commit}/artifacts    immutable serving plane
```

Projects and repositories carry `vN` ETags, create/update timestamps, and
archive state. Update and archive requests require the current ETag; stale
writers receive gRPC `ABORTED`. Archive is a soft delete: it never deletes
repositories, commits, operation history, ChangeRecords, or artifacts.

## Persistence and atomicity

The production stores use ObjectDb resource-record collections:

| Collection | Contents |
|---|---|
| `schemahub.projects.v1` | Project metadata, visibility, creator, ETag, timestamps, archive state |
| `schemahub.project_index.v1/{active,all}` | Name-ordered active/all project catalogs |
| `schemahub.project_index_migration.v1` | Durable project-catalog backfill completion marker |
| `schemahub.project_roles.v1` | Project/hex-identity ordered role records with inactive tombstones |
| `schemahub.repositories.v1` | Repository metadata and effective policy |
| `schemahub.repository_index.v1/projects/{hex(project)}/{active,all}` | Name-ordered active/all repository catalogs scoped to one project |
| `schemahub.repository_index_migration.v1` | Durable repository-catalog backfill completion marker |
| `schemahub.control_plane_audit_events.v1/projects/{hex(project)}` | Immutable project/member/repository events |
| `schemahub.control_plane_audit_event_index.v1/projects/{hex(project)}` | Immutable newest-first event-name index |

`ObjectDb::create_records` creates a project and all initial membership records
in one transaction. If the project, owner, or any imported membership already
exists, nothing is written. Memory, redb, and PostgreSQL implement the same
all-or-nothing contract. Updates use byte-level compare-and-swap and advance
the resource ETag only after a successful commit.

`ObjectDb::transact_records` applies distinct-key create, compare-and-swap, and
compare-and-delete operations in one transaction. Runtime project, member, and
repository mutations use it to couple the resource write to one immutable
audit-event create and one immutable ordered-index entry. If an ETag is stale,
an event/index key collides, or the database fails, none of the three records
commits. Memory, redb, and PostgreSQL implement the same contract.

Every runtime project, member, and repository mutation also holds a
project-keyed ObjectDb publication guard from authorization through invariant
checks and state/event commit. PostgreSQL maps it to a distributed advisory
lock, so two server instances cannot both observe two Owners and remove the
last two concurrently. Different project keys remain independently
coordinated.

Project and repository creates also create their all/active catalog entries in
the same transaction. Archive or unarchive compare-and-swap updates move the
active entry atomically with resource state and, for runtime mutations, the
audit event/index. A catalog-key collision therefore commits neither resource
nor audit evidence. Before the first indexed catalog operation, SchemaHub
validates every pre-index resource, creates missing entries, and writes one
durable completion marker in the same transaction. Once marked complete,
catalog pages never fall back to a global resource scan.

The former `[auth].data_dir/{projects.json,roles.json}` stores are migration
inputs, not production write targets. On startup, an authenticated deployment
atomically imports each legacy project and its complete ACL when the database
does not already contain that project. A legacy project without an Owner fails
startup rather than creating an unusable resource. Existing database records
always win, making the import restart-safe and idempotent.

## Project lifecycle

- `CreateProject` requires an authenticated caller and atomically makes that
  identity the first Owner.
- `GetProject` and `ListProjects` hide archived projects by default. Explicit
  archive reads are Owner-only.
- `UpdateProject` currently permits only `is_public` through a field mask and
  requires the current ETag.
- `DeleteProject` archives the project. It refuses projects containing any
  repository record unless `force=true`; forced archival retains those records
  but makes the project runtime-inert.
- Project lists are name-ordered and use filter-bound opaque page tokens over
  bounded active/all index ranges. Authorization filtering is scan-bounded, so
  a page may contain zero readable projects and still return a token; clients
  continue until the token is empty.
- The same-release GUI BFF adapts those pages without re-enumerating the
  catalog. Its project summary performs a direct caller-role lookup and omits
  repository counts, avoiding the former bounded-page-plus-unbounded-N+1
  behavior.
- Its repository dashboard separately pages the selected repository's schema,
  branch, and tag namespaces. The composite continuation binds repository and
  ref expression, and schema pages retain the immutable commit resolved by the
  first response. Selected schema objects and repository-local names are
  batch-read in one tree traversal; displayed dependency totals are unique
  declared direct imports, not a target traversal.
- Member changes remain Owner-only and cannot remove or downgrade the last
  Owner, including under concurrent removal or downgrade attempts.

Once a project is archived, normal RBAC checks fail closed for every schema,
repository, ChangeRecord, and serving operation below it. The retained Owner
ACL remains available only for explicit audit reads and idempotent archive
retries.

## Membership catalog

`ListMembers` is readable by callers with project `Read` access and orders
active members by their opaque identity bytes. Its project-bound v1 token maps
to the existing primary role key
`projects/{project}/members/{hex(identity)}`. Each backend request starts at
that project prefix and reads at most `page_size + 1` records; it never loads
roles from an earlier project or decodes a later project's record.

Removal retains an inactive tombstone at the same key so re-adding an identity
is a compare-and-swap update and list order remains stable. A bounded page may
therefore contain no active members while still returning a continuation past
the tombstones. Clients continue until the token is empty. Scoped malformed
records, key/content mismatches, invalid identities, and malformed or
cross-project tokens fail closed. Because the ordered primary key predates this
pagination contract, no membership backfill is required.

## Control-plane audit

`ListControlPlaneAuditEvents` is Owner-only, including on public projects. It
lists children of `parent=projects/{project}` and returns newest-first events
through a parent-bound opaque cursor. Each event carries a server-generated
event ID, server-derived actor, event time, action, target resource name, and typed `ProjectInfo`,
`MemberAuditSnapshot`, or `RepoConfig` state before and after the mutation.
Create events omit `before`; member removal omits `after`.

The cursor advances through the immutable ordered index. Every page issues a
bounded stable range read (`page_size + 1`) rather than loading or sorting the
complete project history. The reader resolves each index target from the
immutable event collection and validates its project, resource name, action,
typed snapshot transition, timestamp, event ID, and index coordinate. A
malformed or mismatched index record, missing event target, or nonempty event
collection with no index fails the request instead of returning a partial
audit page.

The JJ operation log remains the history and undo substrate for schema commits,
bookmark/tag changes, merges, and GC. The control-plane audit log is separate:
administrative resources are mutable AIP-style records, and their immutable
events are evidence rather than an undo mechanism.

## Repository lifecycle and policy

- `CreateRepo`, `GetRepo`, `UpdateRepo`, `ListRepos`, and `DeleteRepo` operate
  on persisted repository resources; none is an echo or empty-list stub.
- Repository lists use bounded prefix ranges in the requested project's
  active/all catalog and never scan repositories from another project.
- The GUI repository selector preserves those bounds and uses an opaque token
  tied to the repository catalog, project, and name prefix. Deep links perform
  a size-one prefix read followed by an exact-name check.
- Repository-local branch and tag RPCs return bounded stable name pages with
  opaque continuations bound to ref kind, repository, and prefix. This pages
  response materialization over the single immutable JJ view; it is distinct
  from the ObjectDb resource-catalog indexes above.
- Repository update uses a field mask and ETag. Archive retains JJ history and
  requires `force=true` when bookmarks or tags exist.
- Startup `[repos."project/repo"]` sections seed missing repository resources
  and never overwrite runtime changes.

The effective repository policy contains:

```toml
[repos."acme/commerce"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main", "release/*"]

[repos."acme/commerce".review]
required_approvals = 2
require_change_record = true

[repos."acme/commerce".serving]
source = false
descriptors = true
generated_code = true
```

`required_approvals` counts distinct authenticated reviewers before Apply.
`require_change_record` blocks direct SchemaService publication while allowing
the validated ChangeRecord Apply path. Serving flags independently control
canonical source, descriptor, and generated-code artifact reads. Compatibility
direction and protected bookmark patterns are also read from the persisted
resource at operation time.

## CLI

```bash
schemahub project create acme --public
schemahub project get acme
schemahub project list --page-size 50
schemahub project set-visibility acme private --etag v1
schemahub project archive acme --etag v2 --force
schemahub project get acme --include-archived
schemahub project member list acme --page-size 50 --json
schemahub project audit acme
schemahub project audit acme --json
```

Create, get, list, and update output includes the ETag needed by the next safe
mutation. Project archive intentionally reports that repositories and schema
history were retained.

## Verification contract

Release tests cover atomic create conflicts in memory and redb, redb restart,
PostgreSQL batch rollback, atomic resource-plus-audit commit/rollback across
all backends, typed snapshots, actor attribution, Owner-only audit reads, audit
cursor pagination and CLI JSON, ETag winner/stale-writer behavior, project
and repository catalog prefix pagination, legacy catalog backfill, active-index
archive transitions, collision rollback, redb catalog restart, missing-target
and corrupt-primary handling, authorization-hidden project continuations,
project-scoped membership pages, tombstone continuations, scoped corruption,
cross-project exclusion, project-bound tokens, CLI JSON traversal,
GUI project/repository continuation plus cross-kind/project/prefix rejection,
deterministic concurrent last-Owner preservation, legacy JSON import,
force-gated archive, runtime lockout,
repository policy gates, and immutable artifact-kind enforcement.

D3 is closed. Duplicate tag creation now returns `ALREADY_EXISTS` without
retargeting the original pin. A supplied base revision must be a retained commit
from the target repository; stale bases are accepted as causal provenance and
are not branch-head CAS gates. Direct schema writes persist bounded receipts in
ObjectDb and correlate them with JJ operations for restart/crash replay. See
`idempotency.md` for the receipt protocol and retention contract.
