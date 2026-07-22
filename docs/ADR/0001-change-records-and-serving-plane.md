<!-- agent-updated: 2026-07-21T04:11:52Z -->
# ADR 0001: Separate Change Records From Immutable Schema Serving

## Status

Accepted on 2026-07-21.

## Context

SchemaHub already stores structured declarations in Jujutsu commits and exposes
mutation, history, exploration, and code-generation APIs. A direct mutation,
however, captures the mechanical effect more reliably than the actor's intent.
The product also needs to serve exact schemas to systems that store and read
data; resolving a moving bookmark during every read is not a durable data
contract.

Both humans and software agents must be able to note a proposed change, refine
it, validate it, and connect it to the resulting version. Producers and
consumers must be able to use an immutable identifier after the change is
applied.

## Decision

SchemaHub will expose two first-class product surfaces over the existing
versioned store:

1. A change control plane centered on durable `ChangeRecord` resources. A
   record stores intent, typed edits or replacement source, authenticated actor
   metadata, validation, compatibility, optional review, lifecycle state, and
   the applied commit/change IDs.
2. A schema serving plane centered on immutable `SchemaRevision` and
   `SchemaArtifact` resources. Mutable refs may be resolved, but artifacts are
   fetched and cached by immutable revision and deterministic digest.

Applying a record is one atomic domain operation. It must not leave a record in
`APPLIED` without its commit, or create a commit without linking the record to
the result. The authenticated server identity is audit truth. Human and agent
clients use the same authorization and repository policies.

The existing `Compiler` boundary and JJ commit/op-log model remain intact.
Change records orchestrate those primitives; they do not replace them.

## Consequences

Positive consequences:

- Intent, review, and mechanical history can be queried together.
- Agent-authored changes are explicit and accountable without a privileged
  side channel.
- Data systems can store a stable revision identifier and reproduce decoding.
- Existing granular mutations, compatibility checks, conflicts, and undo remain
  reusable.

Costs and constraints:

- Change records require durable transactional storage in both redb and
  PostgreSQL.
- Atomic application spans record state and JJ-backed schema state, so the
  persistence design must define a real transaction boundary or a recoverable
  state machine before implementation.
- Artifact identity requires a canonical digest definition for transitive
  closures.
- Existing direct mutation RPCs become a lower-level compatibility surface;
  policy must prevent them from bypassing review where review is required.

## Rejected Alternatives

### Treat commit messages as change records

Commit messages cannot represent draft intent, structured validation, review,
or an abandoned proposal. They are retained as content-history summaries.

### Let clients store change notes externally

This breaks the trace from intent to applied revision and gives agents no
portable, queryable workflow.

### Serve mutable bookmarks as durable schema identities

Bookmarks move. Persisted data must refer to an immutable revision or digest so
future consumers receive exactly the schema used by the producer.
