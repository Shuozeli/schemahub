#!/usr/bin/env bash

set -Eeuo pipefail

SCENARIO_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
source "${SCENARIO_DIR}/../lib/harness.sh"

schemahub_lab_init "rw-01-human-agent" 50101

schemahub_write_config <<EOF
[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "orders-owner"
display = "Orders Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "schema-agent"
display = "Delegated Schema Agent"
kind = "agent"
delegated_by = "orders-owner"

[auth.tokens.codelab-producer-token]
id = "order-writer"
display = "Order Writer"
kind = "service"

[auth.tokens.codelab-consumer-token]
id = "replay-worker"
display = "Replay Worker"
kind = "service"

[projects.codelab]
visibility = "private"
owners = ["orders-owner"]
members = { schema-agent = "Writer", order-writer = "Reader", replay-worker = "Reader" }

[repos."codelab/orders"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."codelab/orders".review]
required_approvals = 1
require_change_record = true

[repos."codelab/orders".serving]
source = true
descriptors = true
generated_code = true
EOF

schemahub_start
human_schemahub repo init codelab/orders \
  >"${SCHEMAHUB_EVIDENCE_DIR}/repo-init.txt"

schemahub_note "Arrange: agent discovers compiler capabilities before proposing"
jq -e '
  any(.formats[];
    .format_id == "protobuf"
    and .parse_and_print == true
    and .compatibility == true
    and (.generated_code_languages | index("rust") != null)
  )
' "${SCHEMAHUB_EVIDENCE_DIR}/capabilities.json" >/dev/null

schemahub_note "Act: delegated agent records intent and attaches executable source"
agent_schemahub change note codelab/orders \
  --title "Introduce the persisted order envelope" \
  --description "Order writes and historical replay need one versioned wire contract" \
  --reference CODELAB-ORDER-1 \
  --id introduce-order-record \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-01-note.json"
CHANGE_NAME="$(
  jq -r '.name' "${SCHEMAHUB_EVIDENCE_DIR}/change-01-note.json"
)"
ETAG="$(
  jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/change-01-note.json"
)"
jq -e '
  .status == "draft"
  and .created_by.identity == "schema-agent"
  and .created_by.kind == "agent"
  and .created_by.delegated_by == "orders-owner"
' "${SCHEMAHUB_EVIDENCE_DIR}/change-01-note.json" >/dev/null

agent_schemahub change add-source "${CHANGE_NAME}" \
  --etag "${ETAG}" \
  --schema-path orders/v1/order.proto \
  --file "${SCENARIO_DIR}/fixtures/order-record.proto" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-02-source.json"
ETAG="$(
  jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/change-02-source.json"
)"

agent_schemahub change validate "${CHANGE_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-03-validate.json"
ETAG="$(
  jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/change-03-validate.json"
)"
jq -e '
  .validation.valid == true
  and (.validation.issues | length == 0)
  and (.validation.edit_digest | startswith("sha256:"))
' "${SCHEMAHUB_EVIDENCE_DIR}/change-03-validate.json" >/dev/null

agent_schemahub change ready "${CHANGE_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-04-ready.json"
ETAG="$(
  jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/change-04-ready.json"
)"

schemahub_note "Assert: protected publication fails before independent review"
if agent_schemahub change apply "${CHANGE_NAME}" \
  --etag "${ETAG}" \
  --request-id apply-introduce-order-record \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/unexpected-unreviewed-apply.json" \
  2>"${SCHEMAHUB_EVIDENCE_DIR}/unreviewed-apply-error.json"; then
  printf 'unreviewed ChangeRecord unexpectedly applied\n' >&2
  exit 1
fi
jq -e '.error.grpc_code == "FAILED_PRECONDITION"' \
  "${SCHEMAHUB_EVIDENCE_DIR}/unreviewed-apply-error.json" >/dev/null

schemahub_note "Act: human reviews the stored snapshot, approves, and agent applies"
human_schemahub change get "${CHANGE_NAME}" --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-05-human-review.json"
jq -e '
  .status == "ready"
  and .validation.valid == true
  and .edits[0].kind == "replace_source"
' "${SCHEMAHUB_EVIDENCE_DIR}/change-05-human-review.json" >/dev/null

human_schemahub change approve "${CHANGE_NAME}" \
  --etag "${ETAG}" \
  --reason "Persisted-data contract and compiler validation reviewed" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-06-approve.json"
APPROVED_ETAG="$(
  jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/change-06-approve.json"
)"

agent_schemahub change apply "${CHANGE_NAME}" \
  --etag "${APPROVED_ETAG}" \
  --request-id apply-introduce-order-record \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-07-apply.json"
COMMIT_ID="$(
  jq -r '.apply_result.commit_id' \
    "${SCHEMAHUB_EVIDENCE_DIR}/change-07-apply.json"
)"
OPERATION_ID="$(
  jq -r '.apply_result.operation_id' \
    "${SCHEMAHUB_EVIDENCE_DIR}/change-07-apply.json"
)"
jq -e '
  .status == "applied"
  and .reviews[0].reviewer.kind == "human"
  and (.apply_result.commit_id | length > 0)
  and (.apply_result.operation_id | length > 0)
' "${SCHEMAHUB_EVIDENCE_DIR}/change-07-apply.json" >/dev/null

schemahub_note "Assert: retry uses the same Apply receipt"
agent_schemahub change apply "${CHANGE_NAME}" \
  --etag "${APPROVED_ETAG}" \
  --request-id apply-introduce-order-record \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-08-apply-retry.json"
jq -e \
  --arg commit "${COMMIT_ID}" \
  --arg operation "${OPERATION_ID}" \
  '.apply_result.commit_id == $commit
    and .apply_result.operation_id == $operation' \
  "${SCHEMAHUB_EVIDENCE_DIR}/change-08-apply-retry.json" >/dev/null

schemahub_note "Act: producer pins a revision, verifies artifacts, and writes data"
REVISION_JSON="$(
  producer_schemahub artifact resolve codelab/orders --at main --json
)"
REVISION="$(jq -r '.name' <<<"${REVISION_JSON}")"
schemahub_assert_revision_commit "${REVISION_JSON}" "${COMMIT_ID}"
printf '%s\n' "${REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/revision.json"

producer_schemahub artifact fetch "${REVISION}" \
  --schema-path orders/v1/order.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order.desc" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/descriptor.json"
DESCRIPTOR_DIGEST="$(
  jq -r '.artifact_digest' "${SCHEMAHUB_EVIDENCE_DIR}/descriptor.json"
)"
producer_schemahub artifact verify "${REVISION}" \
  --schema-path orders/v1/order.proto \
  --kind descriptors \
  --digest "${DESCRIPTOR_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

producer_schemahub artifact fetch "${REVISION}" \
  --schema-path orders/v1/order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/generated.json"
GENERATED_DIGEST="$(
  jq -r '.artifact_digest' "${SCHEMAHUB_EVIDENCE_DIR}/generated.json"
)"

export SCHEMAHUB_PINNED_PROTO_RS="${SCHEMAHUB_EVIDENCE_DIR}/order.rs"
schemahub_run_consumer \
  protobuf_pinned \
  "${SCHEMAHUB_EVIDENCE_DIR}/order-data.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/consumer.txt"

jq -n \
  --arg revision "${REVISION}" \
  --arg descriptor_digest "${DESCRIPTOR_DIGEST}" \
  --arg generated_digest "${GENERATED_DIGEST}" \
  '{
    schemahub_revision: $revision,
    schema_path: "orders/v1/order.proto",
    descriptor_digest: $descriptor_digest,
    generated_code_digest: $generated_digest,
    data_file: "order-data.bin"
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/order-data.schema.json"

schemahub_note "Assert: audit and immutable artifacts survive process restart"
schemahub_restart
human_schemahub change get "${CHANGE_NAME}" --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-after-restart.json"
jq -e \
  --arg commit "${COMMIT_ID}" \
  '.status == "applied"
    and .created_by.kind == "agent"
    and .reviews[0].reviewer.kind == "human"
    and .apply_result.commit_id == $commit' \
  "${SCHEMAHUB_EVIDENCE_DIR}/change-after-restart.json" >/dev/null

consumer_schemahub artifact verify "${REVISION}" \
  --schema-path orders/v1/order.proto \
  --kind descriptors \
  --digest "${DESCRIPTOR_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null
consumer_schemahub artifact fetch "${REVISION}" \
  --schema-path orders/v1/order.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/order-after-restart.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/generated-after-restart.json"
test "$(
  sha256sum "${SCHEMAHUB_EVIDENCE_DIR}/order.rs" | cut -d' ' -f1
)" = "$(
  sha256sum "${SCHEMAHUB_EVIDENCE_DIR}/order-after-restart.rs" | cut -d' ' -f1
)"

jq -n \
  --arg change "${CHANGE_NAME}" \
  --arg commit "${COMMIT_ID}" \
  --arg operation "${OPERATION_ID}" \
  --arg revision "${REVISION}" \
  --arg descriptor_digest "${DESCRIPTOR_DIGEST}" \
  --arg generated_digest "${GENERATED_DIGEST}" \
  '{
    scenario: "RW-01",
    status: "passed",
    change: $change,
    commit: $commit,
    operation: $operation,
    pinned_revision: $revision,
    descriptor_digest: $descriptor_digest,
    generated_code_digest: $generated_digest,
    policy: {
      unreviewed_apply: "rejected",
      independent_human_review: "recorded",
      apply_retry: "same commit and operation"
    },
    data: "real generated binding encoded and decoded persisted bytes",
    restart: "audit record and artifact bytes retained"
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: agent intent, human approval, pinned data, and restart all verified"
