#!/usr/bin/env bash

set -Eeuo pipefail

SCENARIO_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
source "${SCENARIO_DIR}/../lib/harness.sh"

schemahub_lab_init "rw-07-tenant-isolation" 50107 58107
schemahub_require_command curl

ADS_OWNER_TOKEN="ads-owner-token"
ADS_AGENT_TOKEN="ads-agent-token"

ads_owner_schemahub() {
  schemahub_cli_with_token "${ADS_OWNER_TOKEN}" "$@"
}

ads_agent_schemahub() {
  schemahub_cli_with_token "${ADS_AGENT_TOKEN}" "$@"
}

schemahub_write_config <<EOF
[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "finance-owner"
display = "Finance Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "finance-schema-agent"
display = "Finance Schema Agent"
kind = "agent"
delegated_by = "finance-owner"

[auth.tokens.codelab-producer-token]
id = "ledger-writer"
display = "Ledger Writer"
kind = "service"

[auth.tokens.codelab-consumer-token]
id = "finance-replay"
display = "Finance Replay"
kind = "service"

[auth.tokens.ads-owner-token]
id = "ads-owner"
display = "Ads Owner"
kind = "human"

[auth.tokens.ads-agent-token]
id = "ads-schema-agent"
display = "Ads Schema Agent"
kind = "agent"
delegated_by = "ads-owner"

[projects.finance]
visibility = "private"
owners = ["finance-owner"]
members = { finance-schema-agent = "Writer", ledger-writer = "Reader", finance-replay = "Reader" }

[projects.ads]
visibility = "private"
owners = ["ads-owner"]
members = { ads-schema-agent = "Writer" }

[repos."finance/ledger"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."finance/ledger".review]
required_approvals = 1
require_change_record = true

[repos."finance/ledger".serving]
source = true
descriptors = true
generated_code = true

[repos."ads/campaigns"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."ads/campaigns".review]
required_approvals = 1
require_change_record = true

[repos."ads/campaigns".serving]
source = true
descriptors = true
generated_code = true
EOF

schemahub_start
human_schemahub repo init finance/ledger \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-repo-init.txt"
ads_owner_schemahub repo init ads/campaigns \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-repo-init.txt"

schemahub_note "Arrange: publish one private schema in each business tenant"
agent_schemahub change note finance/ledger \
  --title "Publish the finance ledger envelope" \
  --description "Ledger writers and replay workers share an auditable contract" \
  --reference FINANCE-1001 \
  --id ledger-v1 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-01-note.json"
FINANCE_NAME="$(
  jq -r '.name' "${SCHEMAHUB_EVIDENCE_DIR}/finance-01-note.json"
)"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/finance-01-note.json")"
agent_schemahub change add-source "${FINANCE_NAME}" \
  --etag "${ETAG}" \
  --schema-path ledger.proto \
  --file "${SCENARIO_DIR}/fixtures/ledger.proto" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-02-source.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/finance-02-source.json")"
agent_schemahub change validate "${FINANCE_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-03-validate.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/finance-03-validate.json")"
jq -e '.validation.valid == true' \
  "${SCHEMAHUB_EVIDENCE_DIR}/finance-03-validate.json" >/dev/null
agent_schemahub change ready "${FINANCE_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-04-ready.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/finance-04-ready.json")"

schemahub_note "Assert: Reader cannot write and Writer cannot self-approve"
if consumer_schemahub change note finance/ledger \
  --title "Reader must not create changes" \
  --id reader-write-negative \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/unexpected-reader-write.json" \
  2>"${SCHEMAHUB_EVIDENCE_DIR}/reader-write-error.json"; then
  printf 'Reader unexpectedly created a ChangeRecord\n' >&2
  exit 1
fi
jq -e '.error.grpc_code == "PERMISSION_DENIED"' \
  "${SCHEMAHUB_EVIDENCE_DIR}/reader-write-error.json" >/dev/null

if agent_schemahub change approve "${FINANCE_NAME}" \
  --etag "${ETAG}" \
  --reason "Agent must not approve its own proposal" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/unexpected-self-approval.json" \
  2>"${SCHEMAHUB_EVIDENCE_DIR}/self-approval-error.json"; then
  printf 'Writer agent unexpectedly approved its own ChangeRecord\n' >&2
  exit 1
fi
jq -e '.error.grpc_code == "PERMISSION_DENIED"' \
  "${SCHEMAHUB_EVIDENCE_DIR}/self-approval-error.json" >/dev/null

human_schemahub change approve "${FINANCE_NAME}" \
  --etag "${ETAG}" \
  --reason "Finance owner reviewed the private ledger contract" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-05-approve.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/finance-05-approve.json")"
agent_schemahub change apply "${FINANCE_NAME}" \
  --etag "${ETAG}" \
  --request-id apply-finance-ledger-v1 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-06-apply.json"
FINANCE_COMMIT="$(
  jq -r '.apply_result.commit_id' \
    "${SCHEMAHUB_EVIDENCE_DIR}/finance-06-apply.json"
)"

ads_agent_schemahub change note ads/campaigns \
  --title "Publish the campaign impression contract" \
  --description "Ads ingestion uses a schema isolated from finance" \
  --reference ADS-1001 \
  --id campaign-v1 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-01-note.json"
ADS_NAME="$(jq -r '.name' "${SCHEMAHUB_EVIDENCE_DIR}/ads-01-note.json")"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/ads-01-note.json")"
ads_agent_schemahub change add-source "${ADS_NAME}" \
  --etag "${ETAG}" \
  --schema-path campaign.proto \
  --file "${SCENARIO_DIR}/fixtures/campaign.proto" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-02-source.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/ads-02-source.json")"
ads_agent_schemahub change validate "${ADS_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-03-validate.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/ads-03-validate.json")"
ads_agent_schemahub change ready "${ADS_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-04-ready.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/ads-04-ready.json")"
ads_owner_schemahub change approve "${ADS_NAME}" \
  --etag "${ETAG}" \
  --reason "Ads owner reviewed the campaign contract" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-05-approve.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/ads-05-approve.json")"
ads_agent_schemahub change apply "${ADS_NAME}" \
  --etag "${ETAG}" \
  --request-id apply-ads-campaign-v1 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-06-apply.json"

schemahub_note "Assert: project discovery only returns visible private tenants"
consumer_schemahub project list \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-visible-projects.txt"
ads_agent_schemahub project list \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-visible-projects.txt"
grep -q '^finance' "${SCHEMAHUB_EVIDENCE_DIR}/finance-visible-projects.txt"
if grep -q '^ads' "${SCHEMAHUB_EVIDENCE_DIR}/finance-visible-projects.txt"; then
  printf 'finance reader unexpectedly discovered the ads project\n' >&2
  exit 1
fi
grep -q '^ads' "${SCHEMAHUB_EVIDENCE_DIR}/ads-visible-projects.txt"
if grep -q '^finance' "${SCHEMAHUB_EVIDENCE_DIR}/ads-visible-projects.txt"; then
  printf 'ads agent unexpectedly discovered the finance project\n' >&2
  exit 1
fi

schemahub_note "Act: authorized finance consumer fetches and executes its binding"
FINANCE_REVISION_JSON="$(
  consumer_schemahub artifact resolve finance/ledger --at main --json
)"
FINANCE_REVISION="$(jq -r '.name' <<<"${FINANCE_REVISION_JSON}")"
schemahub_assert_revision_commit "${FINANCE_REVISION_JSON}" "${FINANCE_COMMIT}"
printf '%s\n' "${FINANCE_REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-revision.json"
consumer_schemahub artifact fetch "${FINANCE_REVISION}" \
  --schema-path ledger.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/ledger.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ledger-generated.json"
LEDGER_DIGEST="$(
  jq -r '.artifact_digest' "${SCHEMAHUB_EVIDENCE_DIR}/ledger-generated.json"
)"
export SCHEMAHUB_TENANT_PROTO_RS="${SCHEMAHUB_EVIDENCE_DIR}/ledger.rs"
schemahub_run_consumer protobuf_tenant \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/consumer.txt"

schemahub_note "Assert: HTTP search is repository-scoped and enforces tenant access"
HTTP_READY=0
for _ in $(seq 1 30); do
  if curl --fail --silent --show-error \
    "${SCHEMAHUB_HTTP_SERVER_URL}/readyz" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/http-ready.json" 2>/dev/null; then
    HTTP_READY=1
    break
  fi
  sleep 1
done
test "${HTTP_READY}" = "1"

curl --fail --silent --show-error \
  --header "Authorization: Bearer ${SCHEMAHUB_CONSUMER_TOKEN}" \
  "${SCHEMAHUB_HTTP_SERVER_URL}/api/projects/finance/repos/ledger/search?q=Ledger" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-search-ledger.json"
jq -e '
  any(.results[];
    .kind == "declaration"
    and .declarationName == "LedgerEntry"
    and .schemaPath == "ledger.proto"
  )
' "${SCHEMAHUB_EVIDENCE_DIR}/finance-search-ledger.json" >/dev/null

curl --fail --silent --show-error \
  --header "Authorization: Bearer ${SCHEMAHUB_CONSUMER_TOKEN}" \
  "${SCHEMAHUB_HTTP_SERVER_URL}/api/projects/finance/repos/ledger/search?q=Campaign" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/finance-search-campaign.json"
jq -e '.results | length == 0' \
  "${SCHEMAHUB_EVIDENCE_DIR}/finance-search-campaign.json" >/dev/null

OUTSIDER_STATUS="$(
  curl --silent --show-error \
    --output "${SCHEMAHUB_EVIDENCE_DIR}/finance-to-ads-search-error.json" \
    --write-out '%{http_code}' \
    --header "Authorization: Bearer ${SCHEMAHUB_CONSUMER_TOKEN}" \
    "${SCHEMAHUB_HTTP_SERVER_URL}/api/projects/ads/repos/campaigns/search?q=Campaign"
)"
test "${OUTSIDER_STATUS}" = "403"
jq -e '.error | contains("no role on project")' \
  "${SCHEMAHUB_EVIDENCE_DIR}/finance-to-ads-search-error.json" >/dev/null

curl --fail --silent --show-error \
  --header "Authorization: Bearer ${ADS_AGENT_TOKEN}" \
  "${SCHEMAHUB_HTTP_SERVER_URL}/api/projects/ads/repos/campaigns/search?q=Campaign" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/ads-search-campaign.json"
jq -e '
  any(.results[];
    .kind == "declaration"
    and .declarationName == "CampaignImpression"
  )
' "${SCHEMAHUB_EVIDENCE_DIR}/ads-search-campaign.json" >/dev/null

jq -n \
  --arg finance_revision "${FINANCE_REVISION}" \
  --arg ledger_digest "${LEDGER_DIGEST}" \
  '{
    scenario: "RW-07",
    status: "passed",
    tenants: ["finance", "ads"],
    authorization: {
      reader_write: "denied",
      writer_self_approval: "denied",
      private_project_discovery: "isolated",
      cross_tenant_search: "HTTP 403"
    },
    search: {
      scope: "one explicit repository",
      finance_ledger_found: true,
      finance_campaign_results: 0,
      ads_campaign_found: true
    },
    consumer: {
      revision: $finance_revision,
      artifact_digest: $ledger_digest,
      generated_binding: "encoded and decoded"
    }
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: private projects, role boundaries, scoped search, and serving stayed isolated"
