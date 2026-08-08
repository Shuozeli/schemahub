<!-- agent-updated: 2026-07-30T04:16:42Z -->
# Codelab: Isolate Private Schema Tenants

This lab runs private `finance` and `ads` projects in one SchemaHub process.
It exercises the same gRPC control/serving plane and HTTP search BFF used by
real clients while keeping each team's schemas and project discovery isolated.

## What you will prove

- Reader, Writer, and Owner roles enforce distinct actions;
- a Reader cannot create a ChangeRecord;
- a Writer cannot approve its own proposal;
- private project listing does not reveal another tenant;
- an authorized service can execute a generated binding;
- HTTP search requires access to the explicit project/repository;
- search stays repository-scoped rather than becoming global discovery.

## 1. Run the complete lab

```bash
./codelabs/real-world/rw-07-tenant-isolation/run.sh
```

To keep the evidence:

```bash
export SCHEMAHUB_CODELAB_EVIDENCE_DIR=/tmp/schemahub-tenants-evidence
./codelabs/real-world/rw-07-tenant-isolation/run.sh
jq . /tmp/schemahub-tenants-evidence/result.json
```

The harness binds both listeners to the current Tailscale IP and uses the full
MagicDNS host from clients. CI supplies an explicit local-only address. The
default scenario ports are gRPC `50107` and HTTP `58107`.

## 2. Understand the identities

| Tenant | Identity | Kind | Role |
|---|---|---|---|
| finance | `finance-owner` | human | Owner |
| finance | `finance-schema-agent` | delegated agent | Writer |
| finance | `ledger-writer` | service | Reader |
| finance | `finance-replay` | service | Reader |
| ads | `ads-owner` | human | Owner |
| ads | `ads-schema-agent` | delegated agent | Writer |

Both repositories require one approval and a ChangeRecord before protected
`main` can move. The tokens are static codelab credentials scoped to the
disposable process; production should use the JWT configuration documented in
[Authentication](authentication.md).

## 3. Publish one contract per tenant

The finance agent publishes `LedgerEntry`; the ads agent publishes
`CampaignImpression`. Each proposal is validated, made Ready, approved by its
own human owner, and applied by its originating agent.

The runner deliberately executes two unauthorized calls before finance
publication:

```bash
consumer_schemahub change note finance/ledger \
  --title "Reader must not create changes" --json

agent_schemahub change approve "$FINANCE_CHANGE" \
  --etag "$ETAG" \
  --reason "Agent must not approve its own proposal" --json
```

Both calls return structured `PERMISSION_DENIED` errors and leave the
ChangeRecord unchanged. The independent human approval then succeeds.

## 4. Check private project discovery

The finance replay service and ads agent independently run:

```bash
consumer_schemahub project list
ads_agent_schemahub project list
```

The first output contains only `finance`; the second contains only `ads`.
Private projects are filtered from list results rather than exposed as
unauthorized records.

## 5. Execute an authorized served binding

The finance replay service resolves `finance/ledger@main` once, then fetches
generated Rust from the immutable revision:

```bash
consumer_schemahub artifact fetch "$FINANCE_REVISION" \
  --schema-path ledger.proto \
  --kind generated-code --language rust \
  --output "$EVIDENCE/ledger.rs" --json
```

`codelabs/real-world/consumers/src/bin/protobuf_tenant.rs` compiles that exact
file and encodes/decodes a real `LedgerEntry`. This verifies that isolation did
not break the authorized data path.

## 6. Exercise repository-scoped HTTP search

An authorized finance query finds `LedgerEntry`:

```bash
curl --fail --silent --show-error \
  --header "Authorization: Bearer $SCHEMAHUB_CONSUMER_TOKEN" \
  "$SCHEMAHUB_HTTP_SERVER_URL/api/projects/finance/repos/ledger/search?q=Ledger"
```

The same caller searching its finance repository for `Campaign` receives an
empty result. It is not an implicit cross-repository or cross-project query.

When the finance token explicitly targets the ads repository, the endpoint
returns HTTP `403`:

```bash
curl --header "Authorization: Bearer $SCHEMAHUB_CONSUMER_TOKEN" \
  "$SCHEMAHUB_HTTP_SERVER_URL/api/projects/ads/repos/campaigns/search?q=Campaign"
```

The ads agent can issue that query successfully and receives
`CampaignImpression`. Search therefore applies repository authorization before
aggregating schemas, declarations, revisions, and ChangeRecords.

## 7. Read the evidence and boundary

Evidence includes both tenant lifecycles, role-denial JSON, visible-project
lists, all four HTTP search responses, the generated ledger binding, consumer
output, and `result.json`.

Repository-scoped search is the current 1.0 boundary. SchemaHub does not offer
global cross-project search or a global transaction, and this codelab makes
that limitation explicit rather than simulating either capability.

Continue with
[the producer/consumer data-pipeline codelab](codelab-data-pipeline-handoff.md).
