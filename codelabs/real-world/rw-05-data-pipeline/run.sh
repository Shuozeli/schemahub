#!/usr/bin/env bash

set -Eeuo pipefail

SCENARIO_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
source "${SCENARIO_DIR}/../lib/harness.sh"

schemahub_lab_init "rw-05-data-pipeline" 50105

schemahub_write_config <<EOF
[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "data-platform-owner"
display = "Data Platform Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "pipeline-orchestrator"
display = "Pipeline Orchestrator"
kind = "agent"
delegated_by = "data-platform-owner"

[auth.tokens.codelab-producer-token]
id = "batch-producer"
display = "Batch Producer"
kind = "service"

[auth.tokens.codelab-consumer-token]
id = "replay-consumer"
display = "Replay Consumer"
kind = "service"

[projects.analytics]
visibility = "private"
owners = ["data-platform-owner"]
members = { pipeline-orchestrator = "Writer", batch-producer = "Reader", replay-consumer = "Reader" }

[repos."analytics/orders"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."analytics/orders".review]
required_approvals = 0
require_change_record = true

[repos."analytics/orders".serving]
source = true
descriptors = true
generated_code = true

[repos."analytics/events"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."analytics/events".review]
required_approvals = 0
require_change_record = true

[repos."analytics/events".serving]
source = true
descriptors = true
generated_code = true
EOF

schemahub_start
human_schemahub repo init analytics/orders \
  >"${SCHEMAHUB_EVIDENCE_DIR}/orders-repo-init.txt"
human_schemahub repo init analytics/events \
  >"${SCHEMAHUB_EVIDENCE_DIR}/events-repo-init.txt"
mkdir -p "${SCHEMAHUB_EVIDENCE_DIR}/data"

APPLIED_COMMIT=""

apply_source_change() {
  local repo="$1"
  local change_id="$2"
  local title="$3"
  local schema_path="$4"
  local fixture="$5"
  local base_revision="$6"
  local -a base_args=()
  if [[ -n "${base_revision}" ]]; then
    base_args=(--base-revision "${base_revision}")
  fi

  schemahub_note "Act: orchestrator proposes ${repo}/${change_id}"
  local note
  note="$(
    agent_schemahub change note "analytics/${repo}" \
      --title "${title}" \
      --description "Coordinate a producer/consumer schema handoff with immutable serving" \
      --reference "PIPELINE-${change_id}" \
      --id "${change_id}" \
      "${base_args[@]}" \
      --json
  )"
  local name
  local etag
  name="$(jq -r '.name' <<<"${note}")"
  etag="$(jq -r '.etag' <<<"${note}")"
  printf '%s\n' "${note}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-01-note.json"

  local with_source
  with_source="$(
    agent_schemahub change add-source "${name}" \
      --etag "${etag}" \
      --schema-path "${schema_path}" \
      --file "${fixture}" \
      --json
  )"
  etag="$(jq -r '.etag' <<<"${with_source}")"
  printf '%s\n' "${with_source}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-02-source.json"

  local validated
  validated="$(
    agent_schemahub change validate "${name}" \
      --etag "${etag}" \
      --json
  )"
  etag="$(jq -r '.etag' <<<"${validated}")"
  jq -e '.validation.valid == true and (.validation.issues | length == 0)' \
    <<<"${validated}" >/dev/null
  printf '%s\n' "${validated}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-03-validate.json"

  local ready
  ready="$(
    agent_schemahub change ready "${name}" \
      --etag "${etag}" \
      --json
  )"
  etag="$(jq -r '.etag' <<<"${ready}")"
  printf '%s\n' "${ready}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-04-ready.json"

  local applied
  applied="$(
    agent_schemahub change apply "${name}" \
      --etag "${etag}" \
      --request-id "apply-${change_id}" \
      --json
  )"
  APPLIED_COMMIT="$(jq -r '.apply_result.commit_id' <<<"${applied}")"
  jq -e '.status == "applied"' <<<"${applied}" >/dev/null
  printf '%s\n' "${applied}" \
    >"${SCHEMAHUB_EVIDENCE_DIR}/${change_id}-05-apply.json"
  test -n "${APPLIED_COMMIT}"
}

schemahub_note "Arrange: publish the two schemas used by the first data batch"
apply_source_change \
  orders \
  pipeline-order-v1 \
  "Publish the batch order record" \
  schemas/pipeline-order.proto \
  "${SCENARIO_DIR}/fixtures/pipeline-order-v1.proto" \
  ""
ORDER_V1_COMMIT="${APPLIED_COMMIT}"

apply_source_change \
  events \
  pipeline-event-v1 \
  "Publish the stream event record" \
  schemas/pipeline-event.fbs \
  "${SCENARIO_DIR}/fixtures/pipeline-event-v1.fbs" \
  ""
EVENT_V1_COMMIT="${APPLIED_COMMIT}"

ORDER_V1_REVISION_JSON="$(
  producer_schemahub artifact resolve analytics/orders \
    --at "@${ORDER_V1_COMMIT}" \
    --json
)"
ORDER_V1_REVISION="$(jq -r '.name' <<<"${ORDER_V1_REVISION_JSON}")"
EVENT_V1_REVISION_JSON="$(
  producer_schemahub artifact resolve analytics/events \
    --at "@${EVENT_V1_COMMIT}" \
    --json
)"
EVENT_V1_REVISION="$(jq -r '.name' <<<"${EVENT_V1_REVISION_JSON}")"
schemahub_assert_revision_commit "${ORDER_V1_REVISION_JSON}" "${ORDER_V1_COMMIT}"
schemahub_assert_revision_commit "${EVENT_V1_REVISION_JSON}" "${EVENT_V1_COMMIT}"

producer_schemahub artifact fetch "${ORDER_V1_REVISION}" \
  --schema-path schemas/pipeline-order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-generated.json"
producer_schemahub artifact fetch "${ORDER_V1_REVISION}" \
  --schema-path schemas/pipeline-order.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1.desc" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-descriptor.json"
producer_schemahub artifact fetch "${EVENT_V1_REVISION}" \
  --schema-path schemas/pipeline-event.fbs \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-generated.json"
producer_schemahub artifact fetch "${EVENT_V1_REVISION}" \
  --schema-path schemas/pipeline-event.fbs \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1.fbs.bundle" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-descriptor.json"

ORDER_GENERATED_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-generated.json"
)"
ORDER_DESCRIPTOR_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-descriptor.json"
)"
EVENT_GENERATED_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-generated.json"
)"
EVENT_DESCRIPTOR_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-descriptor.json"
)"

schemahub_note "Act: producers encode application data with served bindings"
export SCHEMAHUB_PIPELINE_PROTO_RS="${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1.rs"
schemahub_run_consumer \
  protobuf_pipeline \
  produce \
  "${SCHEMAHUB_EVIDENCE_DIR}/data/orders.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/orders-produce.txt"
export SCHEMAHUB_PIPELINE_FBS_RS="${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1.rs"
schemahub_run_consumer \
  flatbuffers_pipeline \
  produce \
  "${SCHEMAHUB_EVIDENCE_DIR}/data/events.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/events-produce.txt"

jq -n \
  --arg revision "${ORDER_V1_REVISION}" \
  --arg schema_path "schemas/pipeline-order.proto" \
  --arg descriptor_digest "${ORDER_DESCRIPTOR_DIGEST}" \
  --arg generated_digest "${ORDER_GENERATED_DIGEST}" \
  '{
    schemahub_revision: $revision,
    schema_path: $schema_path,
    descriptor_digest: $descriptor_digest,
    generated_rust_digest: $generated_digest
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/data/orders.bin.schema.json"
jq -n \
  --arg revision "${EVENT_V1_REVISION}" \
  --arg schema_path "schemas/pipeline-event.fbs" \
  --arg descriptor_digest "${EVENT_DESCRIPTOR_DIGEST}" \
  --arg generated_digest "${EVENT_GENERATED_DIGEST}" \
  '{
    schemahub_revision: $revision,
    schema_path: $schema_path,
    descriptor_digest: $descriptor_digest,
    generated_rust_digest: $generated_digest
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/data/events.bin.schema.json"
ORDER_DATA_DIGEST="$(
  sha256sum "${SCHEMAHUB_EVIDENCE_DIR}/data/orders.bin" | cut -d' ' -f1
)"
EVENT_DATA_DIGEST="$(
  sha256sum "${SCHEMAHUB_EVIDENCE_DIR}/data/events.bin" | cut -d' ' -f1
)"

schemahub_note "Act: advance each repository independently"
apply_source_change \
  orders \
  pipeline-order-v2 \
  "Add warehouse zone to the batch order" \
  schemas/pipeline-order.proto \
  "${SCENARIO_DIR}/fixtures/pipeline-order-v2.proto" \
  "${ORDER_V1_COMMIT}"
ORDER_V2_COMMIT="${APPLIED_COMMIT}"

apply_source_change \
  events \
  pipeline-event-v2 \
  "Add region to the stream event" \
  schemas/pipeline-event.fbs \
  "${SCENARIO_DIR}/fixtures/pipeline-event-v2.fbs" \
  "${EVENT_V1_COMMIT}"
EVENT_V2_COMMIT="${APPLIED_COMMIT}"

ORDER_V2_REVISION_JSON="$(
  consumer_schemahub artifact resolve analytics/orders --at main --json
)"
ORDER_V2_REVISION="$(jq -r '.name' <<<"${ORDER_V2_REVISION_JSON}")"
EVENT_V2_REVISION_JSON="$(
  consumer_schemahub artifact resolve analytics/events --at main --json
)"
EVENT_V2_REVISION="$(jq -r '.name' <<<"${EVENT_V2_REVISION_JSON}")"
schemahub_assert_revision_commit "${ORDER_V2_REVISION_JSON}" "${ORDER_V2_COMMIT}"
schemahub_assert_revision_commit "${EVENT_V2_REVISION_JSON}" "${EVENT_V2_COMMIT}"

consumer_schemahub artifact fetch "${ORDER_V2_REVISION}" \
  --schema-path schemas/pipeline-order.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v2.desc" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v2-descriptor.json"
consumer_schemahub artifact fetch "${EVENT_V2_REVISION}" \
  --schema-path schemas/pipeline-event.fbs \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v2.fbs.bundle" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v2-descriptor.json"
ORDER_V2_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v2-descriptor.json"
)"
EVENT_V2_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v2-descriptor.json"
)"

schemahub_note "Assert: sidecar revisions still return the exact historical bindings"
consumer_schemahub artifact fetch "${ORDER_V1_REVISION}" \
  --schema-path schemas/pipeline-order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-replay.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-replay.json"
consumer_schemahub artifact fetch "${EVENT_V1_REVISION}" \
  --schema-path schemas/pipeline-event.fbs \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-replay.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-replay.json"
cmp \
  "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1.rs" \
  "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-replay.rs"
cmp \
  "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1.rs" \
  "${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-replay.rs"
consumer_schemahub artifact verify "${ORDER_V1_REVISION}" \
  --schema-path schemas/pipeline-order.proto \
  --kind generated-code \
  --language rust \
  --digest "${ORDER_GENERATED_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null
consumer_schemahub artifact verify "${EVENT_V1_REVISION}" \
  --schema-path schemas/pipeline-event.fbs \
  --kind generated-code \
  --language rust \
  --digest "${EVENT_GENERATED_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

export SCHEMAHUB_PIPELINE_PROTO_RS="${SCHEMAHUB_EVIDENCE_DIR}/pipeline-order-v1-replay.rs"
schemahub_run_consumer \
  protobuf_pipeline \
  consume \
  "${SCHEMAHUB_EVIDENCE_DIR}/data/orders.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/orders-replay.txt"
export SCHEMAHUB_PIPELINE_FBS_RS="${SCHEMAHUB_EVIDENCE_DIR}/pipeline-event-v1-replay.rs"
schemahub_run_consumer \
  flatbuffers_pipeline \
  consume \
  "${SCHEMAHUB_EVIDENCE_DIR}/data/events.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/events-replay.txt"
test "$(
  sha256sum "${SCHEMAHUB_EVIDENCE_DIR}/data/orders.bin" | cut -d' ' -f1
)" = "${ORDER_DATA_DIGEST}"
test "$(
  sha256sum "${SCHEMAHUB_EVIDENCE_DIR}/data/events.bin" | cut -d' ' -f1
)" = "${EVENT_DATA_DIGEST}"

schemahub_note "Act: roll back both repositories explicitly (no global transaction)"
human_schemahub undo analytics/orders --author data-platform-owner \
  >"${SCHEMAHUB_EVIDENCE_DIR}/orders-undo.txt"
human_schemahub undo analytics/events --author data-platform-owner \
  >"${SCHEMAHUB_EVIDENCE_DIR}/events-undo.txt"

ORDER_ROLLED_BACK="$(
  consumer_schemahub artifact resolve analytics/orders --at main --json
)"
EVENT_ROLLED_BACK="$(
  consumer_schemahub artifact resolve analytics/events --at main --json
)"
schemahub_assert_revision_commit "${ORDER_ROLLED_BACK}" "${ORDER_V1_COMMIT}"
schemahub_assert_revision_commit "${EVENT_ROLLED_BACK}" "${EVENT_V1_COMMIT}"

schemahub_note "Assert: rollback does not erase either immutable v2 revision"
consumer_schemahub artifact verify "${ORDER_V2_REVISION}" \
  --schema-path schemas/pipeline-order.proto \
  --kind descriptors \
  --digest "${ORDER_V2_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null
consumer_schemahub artifact verify "${EVENT_V2_REVISION}" \
  --schema-path schemas/pipeline-event.fbs \
  --kind descriptors \
  --digest "${EVENT_V2_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

jq -n \
  --arg scenario "RW-05" \
  --arg order_v1 "${ORDER_V1_COMMIT}" \
  --arg order_v2 "${ORDER_V2_COMMIT}" \
  --arg event_v1 "${EVENT_V1_COMMIT}" \
  --arg event_v2 "${EVENT_V2_COMMIT}" \
  --arg order_revision "${ORDER_V1_REVISION}" \
  --arg event_revision "${EVENT_V1_REVISION}" \
  '{
    scenario: $scenario,
    status: "passed",
    repositories: {
      orders: {
        v1_commit: $order_v1,
        v2_commit: $order_v2,
        historical_revision: $order_revision,
        main_after_rollback: $order_v1
      },
      events: {
        v1_commit: $event_v1,
        v2_commit: $event_v2,
        historical_revision: $event_revision,
        main_after_rollback: $event_v1
      }
    },
    data: "application bytes unchanged and decoded from sidecar-pinned bindings",
    rollback: "two explicit repository undo operations; v2 revisions retained",
    global_transaction: false
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: historical data replayed after advance and explicit two-repo rollback"
