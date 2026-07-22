<!-- agent-updated: 2026-07-21T17:11:37Z -->
# SchemaHub Product Contract

## Purpose

SchemaHub is the shared place where humans and software agents record, review,
apply, and understand schema changes, and where data producers and consumers
retrieve the exact schema artifacts needed to store and read data safely.

SchemaHub governs schemas. It does not store application records, events, or
other user data. Producers store a SchemaHub revision identifier or digest with
their data; consumers use that immutable identifier to retrieve the matching
schema or descriptor bundle.

## Product Model

SchemaHub has three cooperating planes:

```text
Human or agent
      |
      v
Change control plane  ->  versioned schema store  ->  schema serving plane
(intent and policy)       (commits and history)       (immutable artifacts)
```

### Change control plane

The control plane records why a schema should change before or alongside the
mechanical edit. Its primary resource is a `ChangeRecord`.

A change record contains:

- The project, repository, target bookmark, and base revision.
- A human-readable title, rationale, and optional external references.
- An ordered set of typed schema mutations or a full-source replacement.
- The authenticated actor and actor kind: human, agent, or service.
- Validation and compatibility results.
- Review decisions when repository policy requires them.
- The resulting immutable revision after application.

A record may begin as a note-only draft. It must contain an executable change
before it can be applied.

The initial lifecycle is:

```text
DRAFT -> READY -> APPLIED
   |       |
   +-------+-> ABANDONED
           +-> REJECTED
```

`READY` means the change is based on a resolvable revision and passes structural
validation. Compatibility findings are stored on the record; repository policy
decides whether a violation blocks application or requires an elevated override.

### Versioned schema store

Applying a change record produces one SchemaHub commit and operation-log entry.
The existing Jujutsu model remains authoritative for content history:

- Commits are immutable and content-addressed.
- Change IDs identify logical edits across rewrites.
- Bookmarks provide mutable collaboration targets.
- Tags and commit IDs provide immutable serving targets.
- Concurrent edits create declaration-level conflicts instead of losing work.
- The operation log provides audit and undo.

The applied change record links intent and review history to its resulting
commit and stable change ID.

### Schema serving plane

The serving plane is read-oriented and revision-addressed. It serves:

- Canonical reconstructed schema source.
- Native descriptor bundles, including transitive dependencies.
- Generated source where the selected compiler supports it.
- Format, content digest, dependency digests, and resolved commit metadata.

A mutable bookmark may be resolved to a revision, but artifact reads use the
resolved immutable revision. Every artifact has a deterministic digest so it
can be cached, compared, and stored with encoded data.

## First-Class Resources

| Resource | Responsibility |
|---|---|
| `Project` | Ownership, visibility, and membership boundary. |
| `Repository` | Compatibility, review, retention, and protected-bookmark policy. |
| `Schema` | A logical Protobuf, FlatBuffers, or OpenAPI schema within a repository. |
| `ChangeRecord` | Durable intent, operations, validation, review, and application result. |
| `SchemaRevision` | Immutable schema state resolved at a specific commit. |
| `SchemaArtifact` | Deterministic source, descriptor, or generated-code representation of a revision. |

Resource names are stable and server-assigned. Authenticated identity and audit
timestamps are server truth, not client-supplied claims.

## Primary Workflows

### Human-authored change

1. A human creates a draft and explains the intent.
2. The human or UI adds typed mutations or replacement source.
3. SchemaHub validates syntax, references, and compatibility.
4. Required reviewers approve or reject the ready change.
5. SchemaHub applies it atomically and records the resulting revision.

### Agent-authored change

1. An agent creates the same resource through the public API or CLI.
2. Its authenticated identity records that the actor is an agent and, when
   applicable, which human or service delegated the work.
3. The agent uses incremental exploration and validation APIs to refine the
   draft without bypassing policy.
4. The same review and application rules used for humans apply to the agent.

### Data producer and consumer

1. A producer resolves an approved tag or commit to a `SchemaRevision`.
2. It obtains the descriptor or generated binding and records the immutable
   revision name or digest alongside stored data.
3. A consumer retrieves the artifact by that immutable identity and verifies
   the digest before decoding.

## Product Principles

1. Intent and effect are both durable. A commit alone does not explain why a
   change was requested; a note alone does not prove what was applied.
2. Humans and agents share one API and one policy model. Agent identity is
   explicit, but it is not a privilege bypass.
3. Serving is immutable by default. Mutable refs are conveniences for
   resolution, not durable data contracts.
4. Compatibility and conflicts are visible data, not flattened log messages.
5. Format-specific behavior remains behind the `Compiler` trait.
6. All externally visible writes are idempotent and auditable.
7. SchemaHub never becomes the application-data database.

## Version 1 Success Criteria

SchemaHub 1.0 is successful when:

- A human and an authenticated agent can independently create, validate, and
  apply change records through documented public interfaces.
- Every applied record can be traced to one commit, one stable change ID, and
  one operation-log entry.
- Protobuf and FlatBuffers producers can retrieve transitive descriptor bundles
  by immutable revision and compile generated Rust in end-to-end tests.
- A stored revision identifier continues to resolve after bookmarks move and
  after the server restarts.
- Repository policy, compatibility checks, and conflict state cannot be
  bypassed accidentally by choosing a different client.
- A human or agent can discover direct downstream imports across every
  repository visible to its identity, distinguish immutable pins from live
  edges, and retain the exact per-repository snapshots used for coordination.
- Redb and PostgreSQL deployments have tested backup, restore, and upgrade
  procedures.

## Deferred Beyond Version 1

- Storing application data or running generated applications.
- SQL DDL and database-migration execution.
- Automatic cross-repository rename propagation.
- Multi-region active-active serving.
- A general workflow engine unrelated to schema changes.
