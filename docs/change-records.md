<!-- agent-updated: 2026-07-30T04:16:42Z -->
# Change Record Design

## Scope

`ChangeRecord` is the durable bridge between a human or agent saying what
should change and SchemaHub recording what actually changed. It is a control
plane resource layered over the existing compiler, compatibility, JJ commit,
conflict, and operation-log primitives.

This design deliberately separates four concerns:

1. Recording intent, which is useful before executable edits exist.
2. Validating a concrete proposed state without mutating a bookmark.
3. Applying approved edits exactly once from the caller's perspective.
4. Serving the immutable revision produced by that application.

## Implementation Status

The D1 vertical slice is implemented:

- Human, agent, service, and anonymous actor kinds, including agent delegation.
- The domain resource, injected clock/ID providers, lifecycle ledger, and
  compare-and-set store contract.
- Memory, redb, and PostgreSQL-backed persistence. Redb is covered by a
  close/reopen test; PostgreSQL create/list/CAS behavior runs against isolated
  integration-test schemas.
- Repository-scoped creation-order and per-status indexes. Durable creates and
  status transitions update record/index state atomically; public pages use
  bounded backend ranges and validate every returned index target.
- Compiler-backed validation with immutable base resolution, deterministic edit
  digests, stored findings, protected-bookmark compatibility checks, and
  unresolved-base/conflict reporting.
- gRPC Create/Get/List/Update/Delete/Abandon plus Validate, MarkReady, Approve,
  Reject, and Apply.
- CLI note/get/list/update/add-source/add-mutation/delete-schema/validate/ready/
  approve/reject/apply/abandon, including stable `--json` output.
- Ordered external issue, incident, design, or automation references across
  durable storage, gRPC, CLI, HTTP, GUI, and repository search.
- Recoverable Apply leases, JJ correlation attributes, historical-operation
  receipt recovery, and idempotent retry behavior.
- Exact-final-tree publication policy under a backend repository guard. A known
  protected-conflict/reference rejection publishes no JJ operation and releases
  the Apply lease back to `READY`; ambiguous storage failures stay `APPLYING`
  for correlation recovery.

Direct schema mutations do not synthesize applied change records. A repository
can instead set `review.require_change_record=true`, which blocks those direct
publication paths and requires the auditable ChangeRecord workflow.

## Resource Name

```text
projects/{project}/repos/{repo}/changes/{change}
```

The server generates `change` by default. A caller may supply a valid
`change_id` during Create for deterministic automation. Reusing an existing id
currently returns `ALREADY_EXISTS`; persisted Create request-id idempotency is
part of D3 hardening.

## Resource Shape

```proto
message ChangeRecord {
  string name = 1;
  string target_bookmark = 2;
  string base_revision = 3;
  string title = 4;
  string description = 5;
  repeated ChangeEdit edits = 6;
  Actor created_by = 7;
  ChangeStatus status = 8;
  ValidationResult validation = 9;
  repeated Review reviews = 10;
  ApplyResult apply_result = 11;
  string etag = 12;
  google.protobuf.Timestamp create_time = 13;
  google.protobuf.Timestamp update_time = 14;
  ChangeApplyAttempt apply_attempt = 15;
  repeated string external_references = 16;
}
```

On Create, an omitted or empty `target_bookmark` is filled from the target
repository's configured default after authorization. The stored record always
contains the concrete bookmark, so later validation and Apply never depend on a
changing transport default.

`name`, actor, status, results, ETag, and timestamps are output-only. A client
cannot claim a different audit actor by putting an author in the request body.
`external_references` is an ordered input field: at most 32 trimmed, unique,
non-empty values of at most 2,048 bytes each. Values are intentionally opaque
so teams may use URLs, issue keys, incident IDs, design IDs, or agent-run
correlation keys without SchemaHub owning those external namespaces. Stored
records from before field 16 decode with an empty list.

### Actor

```proto
message Actor {
  string identity = 1;
  ActorKind kind = 2; // HUMAN, AGENT, SERVICE, or ANONYMOUS
  string display_name = 3;
  string delegated_by = 4;
}
```

The configured `AuthnProvider` resolves all actor fields. `delegated_by` is
meaningful for agent credentials issued on behalf of a human or service, but it
does not alter authorization.

### Change edits

```proto
message ChangeEdit {
  oneof edit {
    TransactionOp mutation = 1;
    ReplaceSchemaSource replace_source = 2;
    DeleteSchemaEdit delete_schema = 3;
  }
}
```

Typed mutations remain the preferred agent-facing representation. Full-source
replacement is retained for imports and human editor workflows. Edits are
ordered and repository-scoped; one record may touch several schema files but
must obey the same format and size limits as an atomic transaction.

A draft may have no edits. `MarkReady` rejects a note-only draft.

## Lifecycle

```text
              +-----------+
              |           v
DRAFT -> READY -> APPLYING -> APPLIED
  |       |  |       |
  |       |  |       +-> READY       (attempt failed before commit)
  |       |  +----------> REJECTED
  +-------+-------------> ABANDONED
```

`APPLYING` is externally visible for observability but is managed only by the
server. A record in that state is recoverable; clients retry `Apply` with the
same request ID or wait for reconciliation.

| From | Operation | To | Preconditions |
|---|---|---|---|
| absent | Create | `DRAFT` | Caller can write the repository; title is non-empty. |
| `DRAFT` | Update | `DRAFT` | Matching ETag; only mutable input fields change. |
| `DRAFT` | Validate | `DRAFT` | Validation snapshot is replaced, but no state transition occurs. |
| `DRAFT` | MarkReady | `READY` | At least one edit; current validation passes; base is resolvable. |
| `READY` | Approve | `READY` | Reviewer is authorized; decision is appended. |
| `READY` | Reject | `REJECTED` | Reviewer is authorized; reason is required. |
| `DRAFT`/`READY` | Abandon | `ABANDONED` | Caller is author or repository maintainer. |
| `READY` | Apply | `APPLYING` | Review policy satisfied; matching ETag and request ID. |
| `APPLYING` | Reconcile | `APPLIED` | Matching JJ operation/commit exists. |
| `APPLYING` | Reconcile | `READY` | No commit exists and the attempt is safely retryable. |

`APPLIED`, `REJECTED`, and `ABANDONED` records are immutable.

## Public API

The service follows Google AIP resource shapes:

```proto
service ChangeService {
  rpc CreateChange(CreateChangeRequest) returns (ChangeRecord);
  rpc GetChange(GetChangeRequest) returns (ChangeRecord);
  rpc ListChanges(ListChangesRequest) returns (ListChangesResponse);
  rpc UpdateChange(UpdateChangeRequest) returns (ChangeRecord);
  rpc DeleteChange(DeleteChangeRequest) returns (google.protobuf.Empty);
  rpc ValidateChange(ValidateChangeRequest) returns (ChangeRecord);
  rpc MarkChangeReady(MarkChangeReadyRequest) returns (ChangeRecord);
  rpc ApproveChange(ApproveChangeRequest) returns (ChangeRecord);
  rpc RejectChange(RejectChangeRequest) returns (ChangeRecord);
  rpc ApplyChange(ApplyChangeRequest) returns (ChangeRecord);
  rpc AbandonChange(AbandonChangeRequest) returns (ChangeRecord);
}
```

`DeleteChange` is a compatibility-shaped soft delete: it transitions a draft to
`ABANDONED`. It does not erase the audit record.

List uses parent/filter-bound opaque page tokens, stable creation-time/name
ordering, and optional status filtering. The transport delegates each page to
the matching repository/status index and reads at most `page_size + 1` ordered
entries from `ObjectDb`; malformed entries, missing targets, scope/status
mismatches, and key/record mismatches fail closed. Existing redb/PostgreSQL
stores are indexed once before their first post-upgrade ledger operation, with
every legacy record validated before all missing all/status entries and a
durable completion marker commit atomically. Because a pre-index deployment
does not maintain those indexes, mixed old/new processes against one database
are unsupported.

Actor, target-bookmark, and update-time filters remain planned. Update uses a
field mask and ETag; mutable metadata includes `external_references`, and stale
ETags return `ABORTED`. Custom mutation methods require a request ID.

## Validation Snapshot

Validation is data stored on the record, not only an RPC error. A snapshot
includes:

- The base commit resolved during validation.
- A deterministic digest of ordered edits.
- Structural/compiler validation errors.
- Reference-integrity findings.
- Compatibility direction and violations.
- Conflicts already present at the target.
- Validation time and validator version.

Changing any edit, target, or base clears the snapshot and returns the record to
an unvalidated draft.

## Apply and Crash Recovery

The existing JJ write path performs several object-store transactions, so the
record and commit cannot be honestly described as one database transaction
without first changing that persistence boundary. Version 1 therefore uses a
recoverable state machine with a durable correlation marker:

1. Compare-and-set `READY` to `APPLYING`, recording `apply_attempt_id` and the
   caller's request ID.
2. Search the repository operation log for
   `schemahub.change_record=<resource name>` and the attempt ID.
3. If absent, build the final merged schema tree while holding the backend
   repository publication guard, reject protected conflicts or dangling live
   imports, and otherwise stamp the correlation attributes on the one JJ
   operation.
4. Compare-and-set the record to `APPLIED` with commit ID, change ID, operation
   ID, conflict list, and artifact digest.
5. On retry after a process restart, step 2 recovers a commit written before a
   crash in step 4. If no correlated operation exists, the live lease owner (or
   the same request after lease expiry) rebuilds the validated plan and safely
   performs the one correlated write; observers return the durable `APPLYING`
   state and retry.

If exact-final-tree policy rejects in step 3, no operation exists and the
current lease owner compare-and-sets the record back to `READY` with
`apply_attempt` cleared. A later attempt must refetch the new ETag. JJ/storage
errors do not take this shortcut because their publication outcome can be
ambiguous.

Concurrent Apply calls serialize through the record ETag/lease. PostgreSQL uses
a row lock inside a transaction; redb uses one write transaction. A process
mutex is not sufficient for multi-instance deployments.

## Direct Mutation Compatibility

Existing `SchemaService` mutation methods remain available during migration.
When repository review policy is disabled, they can continue to create an
implicit applied change record. When review is required, direct writes must
either reference an approved record or fail with `FAILED_PRECONDITION`.

This prevents a client from bypassing review simply by selecting the older RPC.

## Incremental Delivery

1. **Done:** add identity kinds, the domain resource, injected clock/ID
   interfaces, an in-memory fake store, and lifecycle tests.
2. **Done:** add note-oriented Create/Get/List/Update/Abandon gRPC and CLI
   workflows.
3. **Done:** add redb and PostgreSQL stores with restart and transactional
   compare-and-swap tests. Thirty-two independent ledger instances elect one
   apply lease and concurrent reconcilers converge on one persisted receipt.
4. **Done:** add validation snapshots and executable edits.
5. **Done:** add maintainer review methods, required-approval policy, apply
   correlation metadata, leases, and reconciliation.
6. **Done:** repository policy may require ChangeRecords and reject direct
   publication. Automatic draft synthesis remains a possible surface-level
   convenience rather than a storage prerequisite.
