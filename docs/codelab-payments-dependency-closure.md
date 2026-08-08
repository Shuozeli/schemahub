<!-- agent-updated: 2026-07-30T04:16:42Z -->
# Codelab: Evolve a Shared Payments Schema

This lab models two payment services that share a Protobuf `Money` type. The
capture event in `payments.capture.v1` imports that type from the distinct
`payments.money.v1` package, so the contract served for the event is a
multi-file, cross-package dependency closure rather than one isolated source
file.

## What you will prove

- one reviewed ChangeRecord can publish a consistent multi-file schema tree;
- reverse discovery identifies a schema that imports a shared type;
- changing only the dependency changes the root artifact and closure digests;
- served Rust preserves both package module trees and remains relocatable;
- old and new generated closures interoperate on real Protobuf bytes;
- deleting a still-imported schema fails validation and cannot move `main`;
- both immutable closure versions remain retrievable and verifiable.

## 1. Run the complete lab

From the SchemaHub repository root:

```bash
./codelabs/real-world/rw-06-dependency-closure/run.sh
```

To retain the evidence at a predictable path:

```bash
export SCHEMAHUB_CODELAB_EVIDENCE_DIR=/tmp/schemahub-payments-evidence
./codelabs/real-world/rw-06-dependency-closure/run.sh
jq . /tmp/schemahub-payments-evidence/result.json
```

The runner starts a release-mode SchemaHub server with a disposable redb
database, a human payments owner, a delegated schema agent, and separate
producer/consumer services.

## 2. Inspect the schema closure

The root fixture is
`codelabs/real-world/rw-06-dependency-closure/fixtures/payment.proto`:

```proto
syntax = "proto3";
package payments.capture.v1;

import "payments/contracts/payments/money.proto";

message PaymentCaptured {
  string payment_id = 1;
  .payments.money.v1.Money amount = 2;
}
```

SchemaHub logical imports use `project/repository/schema-path`. The root above
therefore imports `payments/money.proto` from the `payments/contracts`
repository. Version 1 of `Money` contains `units` and `nanos`; version 2 adds
`currency_code` at field 3. Both versions declare
`package payments.money.v1`, making the import a real cross-package reference.

## 3. Publish both files atomically

The agent creates one ChangeRecord and appends two ordered source edits:

```bash
agent_schemahub change note payments/contracts \
  --title "Publish the payment capture contract" \
  --id payment-contract-v1 --json

agent_schemahub change add-source "$CHANGE_NAME" \
  --etag "$ETAG" \
  --schema-path payments/money.proto \
  --file codelabs/real-world/rw-06-dependency-closure/fixtures/money-v1.proto \
  --json

agent_schemahub change add-source "$CHANGE_NAME" \
  --etag "$ETAG" \
  --schema-path payments/payment.proto \
  --file codelabs/real-world/rw-06-dependency-closure/fixtures/payment.proto \
  --json
```

Validation compiles the proposed final tree. The human reviews that snapshot,
then the agent applies it with a stable request ID. `v1-04-validate.json` and
`v1-07-apply.json` preserve the validation and publication coordinates.

## 4. Discover direct consumers before editing the shared type

Run reverse discovery against the shared schema:

```bash
human_schemahub schema dependents \
  payments/contracts/payments/money.proto \
  --json
```

The stored response names `payments/payment.proto`, its importing bookmark and
commit, the import string, and the immutable repository snapshot scanned.
Discovery is bounded and visibility-filtered; it does not grant access to a
repository the caller cannot already read.

## 5. Evolve only the dependency

The v2 ChangeRecord replaces `payments/money.proto` but does not touch
`payments/payment.proto`. After approval and Apply, fetch the generated root
artifact at both revisions:

```bash
producer_schemahub artifact fetch "$V1_REVISION" \
  --schema-path payments/payment.proto \
  --kind generated-code --language rust \
  --output "$EVIDENCE/payment-v1.rs" --json

producer_schemahub artifact fetch "$V2_REVISION" \
  --schema-path payments/payment.proto \
  --kind generated-code --language rust \
  --output "$EVIDENCE/payment-v2.rs" --json
```

Both metadata responses include
`payments/contracts/payments/money.proto` in `dependency_schemas`. Their
`closure_digest` and `artifact_digest` values differ because the imported type
changed even though the requested root source did not.

The permanent consumer
`codelabs/real-world/consumers/src/bin/protobuf_dependency.rs` compiles the
served v1 and v2 files under separate Rust `v1` and `v2` modules. Each generated
file contains nested `payments::capture::v1` and `payments::money::v1` modules,
a relocatable relative import between them, and a
`pub use payments::capture::v1::*` root re-export. It verifies:

1. v1 payment bytes decode with v2 and receive an empty default currency;
2. v2 payment bytes decode with v1 while preserving every known field.

## 6. Try to remove a live dependency

The negative ChangeRecord deletes `payments/money.proto` from the v2 base:

```bash
agent_schemahub change delete-schema "$DELETE_NAME" \
  --etag "$ETAG" \
  --schema-path payments/money.proto --json
agent_schemahub change validate "$DELETE_NAME" --etag "$ETAG" --json
```

Validation stores one or more blocking issues because the remaining payment
schema cannot resolve its import. `change ready` then returns
`FAILED_PRECONDITION`. Resolving `main` still yields the v2 commit, and both
historical root artifact digests continue to verify.

## 7. Read the evidence and boundary

`result.json` summarizes both immutable revisions, payload and closure digests,
dependency discovery, wire interoperability, and the rejected deletion.
Generated Rust, encoded payment bytes, lifecycle responses, validation issues,
and server events remain in the printed evidence directory.

This lab coordinates one repository. SchemaHub can discover direct visible
dependents across repositories, but it does not automatically rewrite them or
provide a global multi-repository transaction.

Continue with
[the private tenant isolation codelab](codelab-private-tenant-isolation.md).
