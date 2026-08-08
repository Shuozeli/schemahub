#!/usr/bin/env bash

set -Eeuo pipefail

SCENARIO_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
source "${SCENARIO_DIR}/../lib/harness.sh"

schemahub_lab_init "rw-04-concurrent-editors" 50104

schemahub_write_config <<EOF
[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "collaboration-owner"
display = "Collaboration Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "schema-agent"
display = "Schema Agent"
kind = "agent"
delegated_by = "collaboration-owner"

[auth.tokens.codelab-producer-token]
id = "order-writer"
display = "Order Writer"
kind = "service"

[auth.tokens.codelab-consumer-token]
id = "order-reader"
display = "Order Reader"
kind = "service"

[projects.retail]
visibility = "private"
owners = ["collaboration-owner"]
members = { schema-agent = "Writer", order-writer = "Reader", order-reader = "Reader" }

[repos."retail/collaboration"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."retail/collaboration".review]
required_approvals = 0
require_change_record = true

[repos."retail/collaboration".serving]
source = true
descriptors = true
generated_code = true
EOF

schemahub_start
human_schemahub repo init retail/collaboration \
  >"${SCHEMAHUB_EVIDENCE_DIR}/repo-init.txt"

schemahub_note "Arrange: publish a shared causal base through a ChangeRecord"
BASE_NOTE="$(
  agent_schemahub change note retail/collaboration \
    --title "Publish the shared order base" \
    --description "Human and agent work will fork from this immutable commit" \
    --reference COLLAB-BASE \
    --id collaboration-base \
    --json
)"
BASE_NAME="$(jq -r '.name' <<<"${BASE_NOTE}")"
BASE_ETAG="$(jq -r '.etag' <<<"${BASE_NOTE}")"
printf '%s\n' "${BASE_NOTE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/base-01-note.json"

BASE_SOURCE="$(
  agent_schemahub change add-source "${BASE_NAME}" \
    --etag "${BASE_ETAG}" \
    --schema-path schemas/order.proto \
    --file "${SCENARIO_DIR}/fixtures/order-base.proto" \
    --json
)"
BASE_ETAG="$(jq -r '.etag' <<<"${BASE_SOURCE}")"
printf '%s\n' "${BASE_SOURCE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/base-02-source.json"
BASE_VALIDATED="$(
  agent_schemahub change validate "${BASE_NAME}" \
    --etag "${BASE_ETAG}" \
    --json
)"
BASE_ETAG="$(jq -r '.etag' <<<"${BASE_VALIDATED}")"
jq -e '.validation.valid == true' <<<"${BASE_VALIDATED}" >/dev/null
printf '%s\n' "${BASE_VALIDATED}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/base-03-validate.json"
BASE_READY="$(
  agent_schemahub change ready "${BASE_NAME}" \
    --etag "${BASE_ETAG}" \
    --json
)"
BASE_ETAG="$(jq -r '.etag' <<<"${BASE_READY}")"
printf '%s\n' "${BASE_READY}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/base-04-ready.json"
BASE_APPLIED="$(
  agent_schemahub change apply "${BASE_NAME}" \
    --etag "${BASE_ETAG}" \
    --request-id apply-collaboration-base \
    --json
)"
BASE_COMMIT="$(jq -r '.apply_result.commit_id' <<<"${BASE_APPLIED}")"
printf '%s\n' "${BASE_APPLIED}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/base-05-apply.json"
test -n "${BASE_COMMIT}"

human_schemahub branch create retail/collaboration collab --from main \
  >"${SCHEMAHUB_EVIDENCE_DIR}/branch-create.txt"

schemahub_note "Arrange: human and agent draft from the same immutable base"
HUMAN_NOTE="$(
  human_schemahub change note retail/collaboration \
    --title "Add the fulfillment note" \
    --description "Human-authored edit from the shared base" \
    --reference COLLAB-HUMAN \
    --target-bookmark collab \
    --base-revision "${BASE_COMMIT}" \
    --id human-order-edit \
    --json
)"
HUMAN_NAME="$(jq -r '.name' <<<"${HUMAN_NOTE}")"
HUMAN_ETAG="$(jq -r '.etag' <<<"${HUMAN_NOTE}")"
printf '%s\n' "${HUMAN_NOTE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/human-01-note.json"
HUMAN_SOURCE="$(
  human_schemahub change add-source "${HUMAN_NAME}" \
    --etag "${HUMAN_ETAG}" \
    --schema-path schemas/order.proto \
    --file "${SCENARIO_DIR}/fixtures/order-human.proto" \
    --json
)"
HUMAN_ETAG="$(jq -r '.etag' <<<"${HUMAN_SOURCE}")"
printf '%s\n' "${HUMAN_SOURCE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/human-02-source.json"

AGENT_NOTE="$(
  agent_schemahub change note retail/collaboration \
    --title "Add the automated risk note" \
    --description "Agent-authored edit from the same shared base" \
    --reference COLLAB-AGENT \
    --target-bookmark collab \
    --base-revision "${BASE_COMMIT}" \
    --id agent-order-edit \
    --json
)"
AGENT_NAME="$(jq -r '.name' <<<"${AGENT_NOTE}")"
AGENT_STALE_ETAG="$(jq -r '.etag' <<<"${AGENT_NOTE}")"
printf '%s\n' "${AGENT_NOTE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-01-note.json"
AGENT_SOURCE="$(
  agent_schemahub change add-source "${AGENT_NAME}" \
    --etag "${AGENT_STALE_ETAG}" \
    --schema-path schemas/order.proto \
    --file "${SCENARIO_DIR}/fixtures/order-agent.proto" \
    --json
)"
AGENT_ETAG="$(jq -r '.etag' <<<"${AGENT_SOURCE}")"
printf '%s\n' "${AGENT_SOURCE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-02-source.json"

schemahub_note "Assert: an editor holding the old ETag cannot overwrite the draft"
if agent_schemahub change update "${AGENT_NAME}" \
  --etag "${AGENT_STALE_ETAG}" \
  --description "stale overwrite attempt" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-stale-unexpected.json" \
  2>"${SCHEMAHUB_EVIDENCE_DIR}/agent-stale-error.json"; then
  printf 'stale ChangeRecord ETag unexpectedly succeeded\n' >&2
  exit 1
fi
jq -e '.error.grpc_code == "ABORTED"' \
  "${SCHEMAHUB_EVIDENCE_DIR}/agent-stale-error.json" >/dev/null

schemahub_note "Act: validate and ready both immutable-base proposals"
HUMAN_VALIDATED="$(
  human_schemahub change validate "${HUMAN_NAME}" \
    --etag "${HUMAN_ETAG}" \
    --json
)"
HUMAN_ETAG="$(jq -r '.etag' <<<"${HUMAN_VALIDATED}")"
jq -e \
  --arg base "${BASE_COMMIT}" \
  '.validation.valid == true and .validation.resolved_base_commit == $base' \
  <<<"${HUMAN_VALIDATED}" >/dev/null
printf '%s\n' "${HUMAN_VALIDATED}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/human-03-validate.json"
HUMAN_READY="$(
  human_schemahub change ready "${HUMAN_NAME}" \
    --etag "${HUMAN_ETAG}" \
    --json
)"
HUMAN_ETAG="$(jq -r '.etag' <<<"${HUMAN_READY}")"
printf '%s\n' "${HUMAN_READY}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/human-04-ready.json"

AGENT_VALIDATED="$(
  agent_schemahub change validate "${AGENT_NAME}" \
    --etag "${AGENT_ETAG}" \
    --json
)"
AGENT_ETAG="$(jq -r '.etag' <<<"${AGENT_VALIDATED}")"
jq -e \
  --arg base "${BASE_COMMIT}" \
  '.validation.valid == true and .validation.resolved_base_commit == $base' \
  <<<"${AGENT_VALIDATED}" >/dev/null
printf '%s\n' "${AGENT_VALIDATED}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-03-validate.json"
AGENT_READY="$(
  agent_schemahub change ready "${AGENT_NAME}" \
    --etag "${AGENT_ETAG}" \
    --json
)"
AGENT_ETAG="$(jq -r '.etag' <<<"${AGENT_READY}")"
printf '%s\n' "${AGENT_READY}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-04-ready.json"

schemahub_note "Act: publish the human side, then the agent side from the same base"
HUMAN_APPLIED="$(
  human_schemahub change apply "${HUMAN_NAME}" \
    --etag "${HUMAN_ETAG}" \
    --request-id apply-human-order-edit \
    --json
)"
HUMAN_COMMIT="$(jq -r '.apply_result.commit_id' <<<"${HUMAN_APPLIED}")"
printf '%s\n' "${HUMAN_APPLIED}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/human-05-apply.json"

AGENT_APPLIED="$(
  agent_schemahub change apply "${AGENT_NAME}" \
    --etag "${AGENT_ETAG}" \
    --request-id apply-agent-order-edit \
    --json
)"
AGENT_COMMIT="$(jq -r '.apply_result.commit_id' <<<"${AGENT_APPLIED}")"
AGENT_OPERATION="$(
  jq -r '.apply_result.operation_id' <<<"${AGENT_APPLIED}"
)"
printf '%s\n' "${AGENT_APPLIED}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-05-apply.json"
jq -e '
  .status == "applied"
  and (.apply_result.conflicted_declarations | index("OrderRecord") != null)
' <<<"${AGENT_APPLIED}" >/dev/null

schemahub_note "Assert: retry identity returns the same conflict publication"
AGENT_RETRIED="$(
  agent_schemahub change apply "${AGENT_NAME}" \
    --etag "${AGENT_ETAG}" \
    --request-id apply-agent-order-edit \
    --json
)"
printf '%s\n' "${AGENT_RETRIED}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-06-apply-retry.json"
jq -e \
  --arg commit "${AGENT_COMMIT}" \
  --arg operation "${AGENT_OPERATION}" \
  '.apply_result.commit_id == $commit
    and .apply_result.operation_id == $operation' \
  <<<"${AGENT_RETRIED}" >/dev/null

schemahub_note "Act: render and explicitly resolve the first-class conflict"
human_schemahub resolve \
  retail/collaboration/schemas/order.proto \
  OrderRecord \
  --branch collab \
  >"${SCHEMAHUB_EVIDENCE_DIR}/conflict-rendered.proto"
grep -q 'human_note' "${SCHEMAHUB_EVIDENCE_DIR}/conflict-rendered.proto"
grep -q 'agent_note' "${SCHEMAHUB_EVIDENCE_DIR}/conflict-rendered.proto"

human_schemahub resolve \
  retail/collaboration/schemas/order.proto \
  OrderRecord \
  --branch collab \
  --from "${SCENARIO_DIR}/fixtures/order-resolved.proto" \
  --author collaboration-owner \
  --message "Resolve human and agent order notes" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/conflict-resolution.txt"

human_schemahub schema pull \
  retail/collaboration/schemas/order.proto \
  --branch collab \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-resolved.proto"
grep -q 'human_note' "${SCHEMAHUB_EVIDENCE_DIR}/order-resolved.proto"
grep -q 'agent_note' "${SCHEMAHUB_EVIDENCE_DIR}/order-resolved.proto"

RESOLVED_REVISION_JSON="$(
  consumer_schemahub artifact resolve retail/collaboration --at collab --json
)"
RESOLVED_REVISION="$(jq -r '.name' <<<"${RESOLVED_REVISION_JSON}")"
RESOLVED_COMMIT="$(jq -r '.commit_id' <<<"${RESOLVED_REVISION_JSON}")"
printf '%s\n' "${RESOLVED_REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/resolved-revision.json"
consumer_schemahub artifact fetch "${RESOLVED_REVISION}" \
  --schema-path schemas/order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order-resolved.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-resolved-generated.json"
consumer_schemahub artifact fetch "${RESOLVED_REVISION}" \
  --schema-path schemas/order.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order-resolved.desc" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-resolved-descriptor.json"
RESOLVED_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/order-resolved-descriptor.json"
)"

schemahub_note "Assert: the explicitly merged binding is executable"
export SCHEMAHUB_CONCURRENT_RS="${SCHEMAHUB_EVIDENCE_DIR}/order-resolved.rs"
schemahub_run_consumer protobuf_concurrent \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/consumer.txt"

schemahub_note "Assert: audit actors, receipts, and resolved bytes survive restart"
schemahub_restart
human_schemahub change get "${HUMAN_NAME}" --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/human-after-restart.json"
human_schemahub change get "${AGENT_NAME}" --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/agent-after-restart.json"
jq -e '
  .status == "applied"
  and .created_by.kind == "human"
  and (.apply_result.commit_id | length > 0)
' "${SCHEMAHUB_EVIDENCE_DIR}/human-after-restart.json" >/dev/null
jq -e \
  --arg operation "${AGENT_OPERATION}" \
  '.status == "applied"
    and .created_by.kind == "agent"
    and .apply_result.operation_id == $operation' \
  "${SCHEMAHUB_EVIDENCE_DIR}/agent-after-restart.json" >/dev/null
AFTER_RESTART_REVISION="$(
  consumer_schemahub artifact resolve retail/collaboration --at collab --json
)"
schemahub_assert_revision_commit "${AFTER_RESTART_REVISION}" "${RESOLVED_COMMIT}"
consumer_schemahub artifact verify "${RESOLVED_REVISION}" \
  --schema-path schemas/order.proto \
  --kind descriptors \
  --digest "${RESOLVED_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

jq -n \
  --arg scenario "RW-04" \
  --arg base_commit "${BASE_COMMIT}" \
  --arg human_commit "${HUMAN_COMMIT}" \
  --arg agent_commit "${AGENT_COMMIT}" \
  --arg resolved_commit "${RESOLVED_COMMIT}" \
  --arg revision "${RESOLVED_REVISION}" \
  --arg digest "${RESOLVED_DIGEST}" \
  '{
    scenario: $scenario,
    status: "passed",
    base_commit: $base_commit,
    human_commit: $human_commit,
    agent_conflict_commit: $agent_commit,
    resolved_commit: $resolved_commit,
    pinned_revision: $revision,
    descriptor_digest: $digest,
    concurrency: {
      stale_etag: "rejected",
      same_declaration: "retained as first-class conflict",
      apply_retry: "same commit and operation",
      resolution: "human and agent fields preserved"
    },
    restart: "actors, receipts, bookmark, and artifact survived"
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: stale write failed, conflict was explicit, resolution retained both sides"
