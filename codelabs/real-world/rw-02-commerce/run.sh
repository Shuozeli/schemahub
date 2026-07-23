#!/usr/bin/env bash

set -Eeuo pipefail

SCENARIO_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
source "${SCENARIO_DIR}/../lib/harness.sh"

schemahub_lab_init "rw-02-commerce" 50102

schemahub_write_config <<EOF
[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "commerce-owner"
display = "Commerce Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "schema-agent"
display = "Schema Agent"
kind = "agent"
delegated_by = "commerce-owner"

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
owners = ["commerce-owner"]
members = { schema-agent = "Writer", order-writer = "Reader", order-reader = "Reader" }

[repos."retail/orders"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."retail/orders".review]
required_approvals = 1
require_change_record = true

[repos."retail/orders".serving]
source = true
descriptors = true
generated_code = true
EOF

schemahub_start
human_schemahub repo init retail/orders \
  >"${SCHEMAHUB_EVIDENCE_DIR}/repo-init.txt"

CHANGE_NAME=""
APPROVED_ETAG=""
APPLIED_JSON=""
APPLIED_COMMIT=""

apply_reviewed_source() {
  local change_id="$1"
  local title="$2"
  local fixture="$3"
  local base_revision="$4"
  local note_file="${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-01-note.json"
  local source_file="${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-02-source.json"
  local validate_file="${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-03-validate.json"
  local ready_file="${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-04-ready.json"
  local approve_file="${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-05-approve.json"
  local apply_file="${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-06-apply.json"
  local -a base_args=()
  if [[ -n "${base_revision}" ]]; then
    base_args=(--base-revision "${base_revision}")
  fi

  schemahub_note "Act: agent proposes ${change_id}"
  agent_schemahub change note retail/orders \
    --title "${title}" \
    --description "Order producers and consumers need a reviewed wire-contract rollout" \
    --reference "COMMERCE-${change_id}" \
    --id "${change_id}" \
    "${base_args[@]}" \
    --json | tee "${note_file}" >/dev/null
  CHANGE_NAME="$(jq -r '.name' "${note_file}")"
  local etag
  etag="$(jq -r '.etag' "${note_file}")"

  agent_schemahub change add-source "${CHANGE_NAME}" \
    --etag "${etag}" \
    --schema-path schemas/order.proto \
    --file "${fixture}" \
    --json | tee "${source_file}" >/dev/null
  etag="$(jq -r '.etag' "${source_file}")"

  agent_schemahub change validate "${CHANGE_NAME}" \
    --etag "${etag}" \
    --json | tee "${validate_file}" >/dev/null
  jq -e '.validation.valid == true and (.validation.issues | length == 0)' \
    "${validate_file}" >/dev/null
  etag="$(jq -r '.etag' "${validate_file}")"

  agent_schemahub change ready "${CHANGE_NAME}" \
    --etag "${etag}" \
    --json | tee "${ready_file}" >/dev/null
  etag="$(jq -r '.etag' "${ready_file}")"

  schemahub_note "Act: human approves ${change_id}"
  human_schemahub change approve "${CHANGE_NAME}" \
    --etag "${etag}" \
    --reason "Compatibility report and rollout behavior reviewed" \
    --json | tee "${approve_file}" >/dev/null
  APPROVED_ETAG="$(jq -r '.etag' "${approve_file}")"

  schemahub_note "Act: agent publishes ${change_id}"
  agent_schemahub change apply "${CHANGE_NAME}" \
    --etag "${APPROVED_ETAG}" \
    --request-id "apply-${change_id}" \
    --json | tee "${apply_file}" >/dev/null
  APPLIED_JSON="$(<"${apply_file}")"
  APPLIED_COMMIT="$(jq -r '.apply_result.commit_id' "${apply_file}")"
  jq -e '.status == "applied"' "${apply_file}" >/dev/null
  test -n "${APPLIED_COMMIT}"
}

schemahub_note "Arrange: publish the original order contract"
apply_reviewed_source \
  order-v1 \
  "Publish the original order record" \
  "${SCENARIO_DIR}/fixtures/order-v1.proto" \
  ""
V1_CHANGE_NAME="${CHANGE_NAME}"
V1_APPROVED_ETAG="${APPROVED_ETAG}"
V1_COMMIT="${APPLIED_COMMIT}"
V1_APPLY_JSON="${APPLIED_JSON}"

schemahub_note "Assert: retrying Apply replays the same immutable receipt"
agent_schemahub change apply "${V1_CHANGE_NAME}" \
  --etag "${V1_APPROVED_ETAG}" \
  --request-id apply-order-v1 \
  --json \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/order-v1-07-apply-retry.json" >/dev/null
jq -e \
  --arg commit "${V1_COMMIT}" \
  '.status == "applied" and .apply_result.commit_id == $commit' \
  "${SCHEMAHUB_EVIDENCE_DIR}/order-v1-07-apply-retry.json" >/dev/null
test "$(
  jq -r '.apply_result.operation_id' \
    "${SCHEMAHUB_EVIDENCE_DIR}/order-v1-07-apply-retry.json"
)" = "$(
  jq -r '.apply_result.operation_id' <<<"${V1_APPLY_JSON}"
)"

V1_REVISION_JSON="$(
  producer_schemahub artifact resolve retail/orders --at "@${V1_COMMIT}" --json
)"
V1_REVISION="$(jq -r '.name' <<<"${V1_REVISION_JSON}")"
schemahub_assert_revision_commit "${V1_REVISION_JSON}" "${V1_COMMIT}"
producer_schemahub artifact fetch "${V1_REVISION}" \
  --schema-path schemas/order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order-v1.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-v1-generated.json"

schemahub_note "Arrange: advance the contract with one additive field"
apply_reviewed_source \
  order-v2 \
  "Add settlement currency to the order record" \
  "${SCENARIO_DIR}/fixtures/order-v2.proto" \
  "${V1_COMMIT}"
V2_COMMIT="${APPLIED_COMMIT}"

V2_REVISION_JSON="$(producer_schemahub artifact resolve retail/orders --at main --json)"
V2_REVISION="$(jq -r '.name' <<<"${V2_REVISION_JSON}")"
schemahub_assert_revision_commit "${V2_REVISION_JSON}" "${V2_COMMIT}"
printf '%s\n' "${V2_REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-v2-revision.json"

producer_schemahub artifact fetch "${V2_REVISION}" \
  --schema-path schemas/order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order-v2.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-v2-generated.json"
producer_schemahub artifact fetch "${V2_REVISION}" \
  --schema-path schemas/order.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order-v2.desc" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-v2-descriptor.json"
V2_DESCRIPTOR_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/order-v2-descriptor.json"
)"
consumer_schemahub artifact verify "${V2_REVISION}" \
  --schema-path schemas/order.proto \
  --kind descriptors \
  --digest "${V2_DESCRIPTOR_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

schemahub_note "Assert: real generated bindings decode both rollout directions"
export SCHEMAHUB_PROTO_V1_RS="${SCHEMAHUB_EVIDENCE_DIR}/order-v1.rs"
export SCHEMAHUB_PROTO_V2_RS="${SCHEMAHUB_EVIDENCE_DIR}/order-v2.rs"
schemahub_run_consumer \
  protobuf_compat \
  "${SCHEMAHUB_EVIDENCE_DIR}/order-v2.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/consumer.txt"

jq -n \
  --arg revision "${V2_REVISION}" \
  --arg schema_path "schemas/order.proto" \
  --arg artifact_digest "${V2_DESCRIPTOR_DIGEST}" \
  '{
    schemahub_revision: $revision,
    schema_path: $schema_path,
    artifact_kind: "descriptors",
    artifact_digest: $artifact_digest
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/order-v2.bin.schema.json"

schemahub_note "Act: validate an incompatible identifier type change"
BREAKING_NOTE="$(
  agent_schemahub change note retail/orders \
    --title "Change the order identifier from string to int64" \
    --description "Negative case: persisted order keys cannot change wire type" \
    --reference COMMERCE-BREAKING \
    --base-revision "${V2_COMMIT}" \
    --id order-breaking \
    --json
)"
printf '%s\n' "${BREAKING_NOTE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-breaking-01-note.json"
BREAKING_NAME="$(jq -r '.name' <<<"${BREAKING_NOTE}")"
BREAKING_ETAG="$(jq -r '.etag' <<<"${BREAKING_NOTE}")"

BREAKING_SOURCE="$(
  agent_schemahub change add-source "${BREAKING_NAME}" \
    --etag "${BREAKING_ETAG}" \
    --schema-path schemas/order.proto \
    --file "${SCENARIO_DIR}/fixtures/order-breaking.proto" \
    --json
)"
printf '%s\n' "${BREAKING_SOURCE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-breaking-02-source.json"
BREAKING_ETAG="$(jq -r '.etag' <<<"${BREAKING_SOURCE}")"

BREAKING_VALIDATION="$(
  agent_schemahub change validate "${BREAKING_NAME}" \
    --etag "${BREAKING_ETAG}" \
    --json
)"
printf '%s\n' "${BREAKING_VALIDATION}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-breaking-03-validate.json"
BREAKING_ETAG="$(jq -r '.etag' <<<"${BREAKING_VALIDATION}")"
jq -e '
  .validation.valid == false
  and any(.validation.issues[]; .code == "compatibility_violation")
' <<<"${BREAKING_VALIDATION}" >/dev/null

if agent_schemahub change ready "${BREAKING_NAME}" \
  --etag "${BREAKING_ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/order-breaking-unexpected-ready.json" \
  2>"${SCHEMAHUB_EVIDENCE_DIR}/order-breaking-ready-error.json"; then
  printf 'breaking change unexpectedly became Ready\n' >&2
  exit 1
fi
jq -e '.error.grpc_code == "FAILED_PRECONDITION"' \
  "${SCHEMAHUB_EVIDENCE_DIR}/order-breaking-ready-error.json" >/dev/null

schemahub_note "Assert: the rejected proposal did not move main"
AFTER_BREAKING_JSON="$(
  consumer_schemahub artifact resolve retail/orders --at main --json
)"
schemahub_assert_revision_commit "${AFTER_BREAKING_JSON}" "${V2_COMMIT}"

jq -n \
  --arg scenario "RW-02" \
  --arg v1_commit "${V1_COMMIT}" \
  --arg v2_commit "${V2_COMMIT}" \
  --arg revision "${V2_REVISION}" \
  --arg digest "${V2_DESCRIPTOR_DIGEST}" \
  '{
    scenario: $scenario,
    status: "passed",
    v1_commit: $v1_commit,
    v2_commit: $v2_commit,
    pinned_revision: $revision,
    descriptor_digest: $digest,
    compatibility: {
      additive: "accepted",
      breaking_wire_type: "rejected"
    },
    consumer: "old->new and new->old generated Rust decoding passed"
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: additive rollout decoded both ways; breaking edit left main unchanged"
