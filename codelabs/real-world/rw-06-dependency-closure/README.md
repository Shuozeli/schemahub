<!-- agent-updated: 2026-07-30T04:16:42Z -->
# RW-06: Payments Dependency Closure

This lab models payment capture and settlement services that share a Protobuf
`Money` type across distinct `payments.capture.v1` and `payments.money.v1`
packages. One reviewed ChangeRecord publishes both source files; a second
changes only the shared dependency while the root payment schema stays
unchanged.

```bash
./codelabs/real-world/rw-06-dependency-closure/run.sh
```

The runner verifies reverse-dependency discovery, immutable closure metadata,
different closure/artifact digests after the dependency evolves, and real
old/new decoding using Rust generated from the served root artifacts. The
generated bundles must contain their package module tree, relocatable
cross-package imports, and a root-package re-export, and both bundles are
compiled inside version modules in one consumer. The runner then tries to
delete the imported schema and proves the invalid proposal cannot become Ready
or move `main`.

Expected negative state is a failed validation plus
`FAILED_PRECONDITION` when the still-imported `Money` file is removed.
Evidence includes both immutable closure artifacts, dependency snapshots,
generated bindings, encoded payment bytes, validation issues, and
`result.json`.

Follow the guided version in
[`docs/codelab-payments-dependency-closure.md`](../../../docs/codelab-payments-dependency-closure.md).
