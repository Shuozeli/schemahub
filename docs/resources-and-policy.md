<!-- agent-updated: 2026-07-21T07:37:56Z -->
# Durable Projects, Repositories, and Policy

This document describes the D3 control-plane resources as implemented. Project
and repository metadata are mutable resources stored beside ChangeRecords in
the selected redb or PostgreSQL database. Schema state remains immutable JJ
objects and history.

## Resource hierarchy

```text
projects/{project}
├── members/{identity}                  Role: Reader..Owner
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
| `schemahub.project_roles.v1` | Project/identity role records with tombstones |
| `schemahub.repositories.v1` | Repository metadata and effective policy |

`ObjectDb::create_records` creates a project and all initial membership records
in one transaction. If the project, owner, or any imported membership already
exists, nothing is written. Memory, redb, and PostgreSQL implement the same
all-or-nothing contract. Updates use byte-level compare-and-swap and advance
the resource ETag only after a successful commit.

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
- Project lists are name-ordered and use filter-bound opaque page tokens.
- Member changes remain Owner-only and cannot remove or downgrade the last
  Owner.

Once a project is archived, normal RBAC checks fail closed for every schema,
repository, ChangeRecord, and serving operation below it. The retained Owner
ACL remains available only for explicit audit reads and idempotent archive
retries.

## Repository lifecycle and policy

- `CreateRepo`, `GetRepo`, `UpdateRepo`, `ListRepos`, and `DeleteRepo` operate
  on persisted repository resources; none is an echo or empty-list stub.
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
```

Create, get, list, and update output includes the ETag needed by the next safe
mutation. Project archive intentionally reports that repositories and schema
history were retained.

## Verification contract

Release tests cover atomic create conflicts in memory and redb, redb restart,
PostgreSQL batch rollback, ETag winner/stale-writer behavior, cursor pagination,
legacy JSON import, force-gated archive, Owner-only archive reads, runtime
lockout, repository policy gates, and immutable artifact-kind enforcement.

D3 is closed. Duplicate tag creation now returns `ALREADY_EXISTS` without
retargeting the original pin. A supplied base revision must be a retained commit
from the target repository; stale bases are accepted as causal provenance and
are not branch-head CAS gates. Direct schema writes persist bounded receipts in
ObjectDb and correlate them with JJ operations for restart/crash replay. See
`idempotency.md` for the receipt protocol and retention contract.
