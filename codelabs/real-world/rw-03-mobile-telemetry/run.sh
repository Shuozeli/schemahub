#!/usr/bin/env bash

set -Eeuo pipefail

SCENARIO_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
source "${SCENARIO_DIR}/../lib/harness.sh"

schemahub_lab_init "rw-03-mobile-telemetry" 50103

schemahub_write_config <<EOF
[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "telemetry-owner"
display = "Telemetry Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "schema-agent"
display = "Schema Agent"
kind = "agent"
delegated_by = "telemetry-owner"

[auth.tokens.codelab-producer-token]
id = "mobile-sdk"
display = "Mobile SDK"
kind = "service"

[auth.tokens.codelab-consumer-token]
id = "telemetry-reader"
display = "Telemetry Reader"
kind = "service"

[projects.mobile]
visibility = "private"
owners = ["telemetry-owner"]
members = { schema-agent = "Writer", mobile-sdk = "Reader", telemetry-reader = "Reader" }

[repos."mobile/telemetry"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."mobile/telemetry".review]
required_approvals = 1
require_change_record = true

[repos."mobile/telemetry".serving]
source = true
descriptors = true
generated_code = true
EOF

schemahub_start
human_schemahub repo init mobile/telemetry \
  >"${SCHEMAHUB_EVIDENCE_DIR}/repo-init.txt"

CHANGE_NAME=""
APPLIED_COMMIT=""

apply_reviewed_source() {
  local change_id="$1"
  local title="$2"
  local fixture="$3"
  local base_revision="$4"
  local -a base_args=()
  if [[ -n "${base_revision}" ]]; then
    base_args=(--base-revision "${base_revision}")
  fi

  schemahub_note "Act: agent proposes ${change_id}"
  local note
  note="$(
    agent_schemahub change note mobile/telemetry \
      --title "${title}" \
      --description "Evolve the mobile telemetry wire layout without stranding deployed readers" \
      --reference "MOBILE-${change_id}" \
      --id "${change_id}" \
      "${base_args[@]}" \
      --json
  )"
  printf '%s\n' "${note}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-01-note.json"
  CHANGE_NAME="$(jq -r '.name' <<<"${note}")"
  local etag
  etag="$(jq -r '.etag' <<<"${note}")"

  local with_source
  with_source="$(
    agent_schemahub change add-source "${CHANGE_NAME}" \
      --etag "${etag}" \
      --schema-path schemas/mobile-event.fbs \
      --file "${fixture}" \
      --json
  )"
  printf '%s\n' "${with_source}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-02-source.json"
  etag="$(jq -r '.etag' <<<"${with_source}")"

  local validated
  validated="$(
    agent_schemahub change validate "${CHANGE_NAME}" \
      --etag "${etag}" \
      --json
  )"
  printf '%s\n' "${validated}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-03-validate.json"
  jq -e '.validation.valid == true and (.validation.issues | length == 0)' \
    <<<"${validated}" >/dev/null
  etag="$(jq -r '.etag' <<<"${validated}")"

  local ready
  ready="$(
    agent_schemahub change ready "${CHANGE_NAME}" \
      --etag "${etag}" \
      --json
  )"
  printf '%s\n' "${ready}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-04-ready.json"
  etag="$(jq -r '.etag' <<<"${ready}")"

  schemahub_note "Act: human approves ${change_id}"
  local approved
  approved="$(
    human_schemahub change approve "${CHANGE_NAME}" \
      --etag "${etag}" \
      --reason "Old/new reader behavior and compiler report reviewed" \
      --json
  )"
  printf '%s\n' "${approved}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-05-approve.json"
  etag="$(jq -r '.etag' <<<"${approved}")"

  local applied
  applied="$(
    agent_schemahub change apply "${CHANGE_NAME}" \
      --etag "${etag}" \
      --request-id "apply-${change_id}" \
      --json
  )"
  printf '%s\n' "${applied}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-06-apply.json"
  APPLIED_COMMIT="$(jq -r '.apply_result.commit_id' <<<"${applied}")"
  jq -e '.status == "applied"' <<<"${applied}" >/dev/null
  test -n "${APPLIED_COMMIT}"
}

schemahub_note "Arrange: publish the deployed mobile event layout"
apply_reviewed_source \
  mobile-event-v1 \
  "Publish the deployed mobile event layout" \
  "${SCENARIO_DIR}/fixtures/mobile-event-v1.fbs" \
  ""
V1_COMMIT="${APPLIED_COMMIT}"
V1_REVISION_JSON="$(
  producer_schemahub artifact resolve mobile/telemetry --at "@${V1_COMMIT}" --json
)"
V1_REVISION="$(jq -r '.name' <<<"${V1_REVISION_JSON}")"
schemahub_assert_revision_commit "${V1_REVISION_JSON}" "${V1_COMMIT}"
producer_schemahub artifact fetch "${V1_REVISION}" \
  --schema-path schemas/mobile-event.fbs \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v1.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v1-generated.json"

schemahub_note "Arrange: deprecate the old slot and add a defaulted sampling rate"
apply_reviewed_source \
  mobile-event-v2 \
  "Deprecate legacy session and add sampling rate" \
  "${SCENARIO_DIR}/fixtures/mobile-event-v2.fbs" \
  "${V1_COMMIT}"
V2_COMMIT="${APPLIED_COMMIT}"
V2_REVISION_JSON="$(
  producer_schemahub artifact resolve mobile/telemetry --at main --json
)"
V2_REVISION="$(jq -r '.name' <<<"${V2_REVISION_JSON}")"
schemahub_assert_revision_commit "${V2_REVISION_JSON}" "${V2_COMMIT}"
printf '%s\n' "${V2_REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2-revision.json"

producer_schemahub artifact fetch "${V2_REVISION}" \
  --schema-path schemas/mobile-event.fbs \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2-generated.json"
producer_schemahub artifact fetch "${V2_REVISION}" \
  --schema-path schemas/mobile-event.fbs \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2.fbs.bundle" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2-descriptor.json"
V2_DESCRIPTOR_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2-descriptor.json"
)"
consumer_schemahub artifact verify "${V2_REVISION}" \
  --schema-path schemas/mobile-event.fbs \
  --kind descriptors \
  --digest "${V2_DESCRIPTOR_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

schemahub_note "Assert: generated readers honor defaults and unknown slots"
export SCHEMAHUB_FBS_V1_RS="${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v1.rs"
export SCHEMAHUB_FBS_V2_RS="${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2.rs"
schemahub_run_consumer \
  flatbuffers_compat \
  "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/consumer.txt"
grep -q '#\[deprecated\]' "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2.rs"

schemahub_note "Act: validate physical removal of the deprecated slot"
BREAKING_NOTE="$(
  agent_schemahub change note mobile/telemetry \
    --title "Physically remove the legacy session slot" \
    --description "Negative case: deprecation must retain the FlatBuffers slot" \
    --reference MOBILE-BREAKING \
    --base-revision "${V2_COMMIT}" \
    --id mobile-event-breaking \
    --json
)"
printf '%s\n' "${BREAKING_NOTE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-breaking-01-note.json"
BREAKING_NAME="$(jq -r '.name' <<<"${BREAKING_NOTE}")"
BREAKING_ETAG="$(jq -r '.etag' <<<"${BREAKING_NOTE}")"
BREAKING_SOURCE="$(
  agent_schemahub change add-source "${BREAKING_NAME}" \
    --etag "${BREAKING_ETAG}" \
    --schema-path schemas/mobile-event.fbs \
    --file "${SCENARIO_DIR}/fixtures/mobile-event-breaking.fbs" \
    --json
)"
printf '%s\n' "${BREAKING_SOURCE}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-breaking-02-source.json"
BREAKING_ETAG="$(jq -r '.etag' <<<"${BREAKING_SOURCE}")"
BREAKING_VALIDATION="$(
  agent_schemahub change validate "${BREAKING_NAME}" \
    --etag "${BREAKING_ETAG}" \
    --json
)"
printf '%s\n' "${BREAKING_VALIDATION}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-breaking-03-validate.json"
BREAKING_ETAG="$(jq -r '.etag' <<<"${BREAKING_VALIDATION}")"
jq -e '
  .validation.valid == false
  and any(.validation.issues[]; .code == "compatibility_violation")
' <<<"${BREAKING_VALIDATION}" >/dev/null

if agent_schemahub change ready "${BREAKING_NAME}" \
  --etag "${BREAKING_ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-breaking-unexpected-ready.json" \
  2>"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-breaking-ready-error.json"; then
  printf 'removed FlatBuffers slot unexpectedly became Ready\n' >&2
  exit 1
fi
jq -e '.error.grpc_code == "FAILED_PRECONDITION"' \
  "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-breaking-ready-error.json" >/dev/null

schemahub_note "Assert: artifact bytes are stable after a process restart"
schemahub_restart
consumer_schemahub artifact fetch "${V1_REVISION}" \
  --schema-path schemas/mobile-event.fbs \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v1-after-restart.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v1-after-restart.json"
consumer_schemahub artifact fetch "${V2_REVISION}" \
  --schema-path schemas/mobile-event.fbs \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2-after-restart.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2-after-restart.json"
cmp \
  "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v1.rs" \
  "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v1-after-restart.rs"
cmp \
  "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2.rs" \
  "${SCHEMAHUB_EVIDENCE_DIR}/mobile-event-v2-after-restart.rs"
AFTER_RESTART_JSON="$(
  consumer_schemahub artifact resolve mobile/telemetry --at main --json
)"
schemahub_assert_revision_commit "${AFTER_RESTART_JSON}" "${V2_COMMIT}"

jq -n \
  --arg scenario "RW-03" \
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
      defaulted_field_and_deprecation: "accepted",
      physical_slot_removal: "rejected"
    },
    consumer: "old->new defaults and new->old generated Rust decoding passed",
    restart: "generated artifact bytes identical"
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: readers interoperated, slot removal failed, restart bytes matched"
