<!-- agent-updated: 2026-07-30T04:16:42Z -->
# RW-07: Private Tenant Isolation

This lab hosts private `finance` and `ads` projects in one SchemaHub process.
Each team publishes a reviewed Protobuf contract with a separate owner and
delegated agent.

```bash
./codelabs/real-world/rw-07-tenant-isolation/run.sh
```

The runner proves that a Reader cannot create changes, a Writer cannot approve
its own proposal, project listing hides the other private tenant, and the HTTP
search endpoint rejects cross-tenant access. It also demonstrates the intended
search boundary: a query runs against one explicit repository and does not
become global discovery.

An authorized finance replay service resolves an immutable ledger revision,
downloads generated Rust, and executes it against a real record. Evidence
includes both tenant lifecycles, structured permission errors, project lists,
HTTP search responses, served bindings, consumer output, and `result.json`.

Follow the guided version in
[`docs/codelab-private-tenant-isolation.md`](../../../docs/codelab-private-tenant-isolation.md).
