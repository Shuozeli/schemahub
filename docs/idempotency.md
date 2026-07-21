<!-- agent-updated: 2026-07-21T16:26:52Z -->
# Durable Write Idempotency

SchemaHub treats a client idempotency key as a short-lived receipt for one
literal write request. It prevents a network retry from creating a second JJ
operation; it does not replace immutable commits, stable change IDs, or the
operation log as the durable schema history.

## Covered writes

The contract applies to every direct schema publication surface:

- whole-document CreateSchema, UpdateSchema, and DeleteSchema;
- granular ApplyMutation;
- atomic ApplyTransaction; and
- branch Merge.

ChangeRecord Apply has its own durable apply-request lease and correlation
protocol because its receipt is part of the long-lived ChangeRecord resource.

## Receipt identity

Each receipt is scoped to `(operation kind, project, repository)`. The client
key is combined with that scope and SHA-256 hashed before storage, so raw client
keys are never persisted. A second endpoint or repository may safely use the
same client key.

The receipt also stores a length-delimited SHA-256 fingerprint of every semantic
request field, including the target bookmark, schema or operations, force flag,
base revision, resolved author, and commit message. Credentials and the client
key are excluded. Reusing a key in the same scope with a different fingerprint
returns `FAILED_PRECONDITION`; it never aliases the earlier response.

## Publication protocol

```text
authenticate and authorize
        |
        v
observe existing receipt ---------------------> replay completed response
        |                                      or reconcile correlated JJ op
        v
validate base, parse/apply, compatibility/policy checks
        |
        v
atomically claim pending receipt + 30 s lease
        |
        v
under the repository publication guard, merge and validate the exact final tree
        | policy/deadline rejection: compare-delete pending receipt and return error
        v
publish one JJ operation carrying receipt + attempt attributes
and durable workflow audit attributes (for example schemahub.force=true)
        |
        v
CAS pending receipt to completed response
```

The early observation is read-only. A missing key is not claimed until all
state-dependent validation succeeds, so invalid requests do not leave poisoned
pending entries. Receipt access happens only after authorization, preventing it
from exposing commit identifiers to an unauthorized caller.

If the process stops after JJ publication but before receipt completion, the
next retry searches the repository operation log for both correlation
attributes. It reconstructs the historical bookmark response from that exact
operation, completes the receipt, and returns it without publishing another
commit. A live, uncorrelated attempt returns `FAILED_PRECONDITION` as in
progress. An expired, uncorrelated lease may be reclaimed with the same attempt
identity.

Final-tree policy runs after the receipt claim because it must inspect the
latest serialized publication candidate. Its rejection is a known
pre-publication outcome, so SchemaHub compare-deletes that pending receipt and
an identical retry can acquire immediately. This cleanup is intentionally not
used for an operational JJ error: the operation may have become durable before
the error surfaced, and retaining the receipt enables correlation recovery.

`ApplyTransaction` shares its server-owned monotonic deadline with Core. If the
timer expires during planning or while queued for the repository publication
guard, Core rejects before commit and removes any receipt it already claimed.
If the request crossed the final atomic publication boundary just before the
timer fired, the receipt remains the recovery marker: retrying the same key
returns or reconstructs the one durable result.

## Persistence and bounds

Receipts use the `schemahub.idempotency.v1` ObjectDb collection, so the selected
redb or PostgreSQL backend is also the receipt backend. The current server
defaults are:

| Setting | Value |
|---|---:|
| Completed receipt TTL | 24 hours |
| Maximum retained receipts | 1,024 |
| Active-attempt lease | 30 seconds |
| Abandoned pending retention | 7 days |

Cleanup first removes expired entries, then evicts the oldest completed
receipts to reserve capacity. Pending work is never evicted merely to admit a
new request; if pending entries occupy the bound, admission fails closed.
Admission across server instances is serialized by a short ObjectDb-backed
capacity lease, so simultaneous new keys cannot overshoot the configured bound.
Deletes use compare-and-delete so cleanup cannot remove a concurrently changed
receipt. Expiration is checked lazily for a same-key retry as well as during
global pruning. Non-dry-run `RunGC` invokes receipt cleanup and reports
`idempotency_entries_cleaned`; `GetServerConfig` reports the 24-hour TTL.

## Failure mapping

| Condition | gRPC status |
|---|---|
| Empty/oversized/control-character key or invalid fingerprint | `INVALID_ARGUMENT` |
| Same scoped key with different request fingerprint | `FAILED_PRECONDITION` |
| Matching request already live without a correlated JJ operation | `FAILED_PRECONDITION` |
| Capacity occupied by pending requests | `FAILED_PRECONDITION` |
| Transaction exceeded the server execution deadline | `DEADLINE_EXCEEDED` |
| ObjectDb or receipt encoding failure | `INTERNAL` |

Release tests cover same-process replay, changed-request rejection, bounded
eviction, lazy TTL expiry, concurrent multi-instance admission, cross-store
receipt visibility, redb process restart, and simulated post-JJ/pre-receipt
crash recovery. Deadline tests cover cancellation before publication and
compare-delete cleanup while queued at the publication guard. PostgreSQL
integration tests cover the compare-and-delete primitive used by cleanup.
