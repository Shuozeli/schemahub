#!/usr/bin/env bash

set -Eeuo pipefail

SCENARIO_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
source "${SCENARIO_DIR}/../lib/harness.sh"

schemahub_lab_init "rw-06-dependency-closure" 50106

schemahub_write_config <<EOF
[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "payments-owner"
display = "Payments Contract Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "payments-schema-agent"
display = "Payments Schema Agent"
kind = "agent"
delegated_by = "payments-owner"

[auth.tokens.codelab-producer-token]
id = "capture-writer"
display = "Payment Capture Writer"
kind = "service"

[auth.tokens.codelab-consumer-token]
id = "settlement-reader"
display = "Settlement Reader"
kind = "service"

[projects.payments]
visibility = "private"
owners = ["payments-owner"]
members = { payments-schema-agent = "Writer", capture-writer = "Reader", settlement-reader = "Reader" }

[repos."payments/contracts"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."payments/contracts".review]
required_approvals = 1
require_change_record = true

[repos."payments/contracts".serving]
source = true
descriptors = true
generated_code = true
EOF

schemahub_start
human_schemahub repo init payments/contracts \
  >"${SCHEMAHUB_EVIDENCE_DIR}/repo-init.txt"

schemahub_note "Arrange: publish a payment event and its shared Money dependency"
agent_schemahub change note payments/contracts \
  --title "Publish the payment capture contract" \
  --description "Capture and settlement services share a versioned Money type" \
  --reference PAYMENTS-1001 \
  --id payment-contract-v1 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-01-note.json"
V1_NAME="$(jq -r '.name' "${SCHEMAHUB_EVIDENCE_DIR}/v1-01-note.json")"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v1-01-note.json")"

agent_schemahub change add-source "${V1_NAME}" \
  --etag "${ETAG}" \
  --schema-path payments/money.proto \
  --file "${SCENARIO_DIR}/fixtures/money-v1.proto" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-02-money.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v1-02-money.json")"
agent_schemahub change add-source "${V1_NAME}" \
  --etag "${ETAG}" \
  --schema-path payments/payment.proto \
  --file "${SCENARIO_DIR}/fixtures/payment.proto" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-03-payment.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v1-03-payment.json")"
agent_schemahub change validate "${V1_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-04-validate.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v1-04-validate.json")"
jq -e '.validation.valid == true and (.validation.issues | length == 0)' \
  "${SCHEMAHUB_EVIDENCE_DIR}/v1-04-validate.json" >/dev/null
agent_schemahub change ready "${V1_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-05-ready.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v1-05-ready.json")"
human_schemahub change approve "${V1_NAME}" \
  --etag "${ETAG}" \
  --reason "Import graph and payment wire contract reviewed" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-06-approve.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v1-06-approve.json")"
agent_schemahub change apply "${V1_NAME}" \
  --etag "${ETAG}" \
  --request-id apply-payment-contract-v1 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-07-apply.json"
V1_COMMIT="$(
  jq -r '.apply_result.commit_id' "${SCHEMAHUB_EVIDENCE_DIR}/v1-07-apply.json"
)"

V1_REVISION_JSON="$(
  producer_schemahub artifact resolve payments/contracts --at main --json
)"
V1_REVISION="$(jq -r '.name' <<<"${V1_REVISION_JSON}")"
schemahub_assert_revision_commit "${V1_REVISION_JSON}" "${V1_COMMIT}"
printf '%s\n' "${V1_REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v1-revision.json"
producer_schemahub artifact fetch "${V1_REVISION}" \
  --schema-path payments/payment.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/payment-v1.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/payment-v1-generated.json"
V1_ARTIFACT_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/payment-v1-generated.json"
)"
V1_CLOSURE_DIGEST="$(
  jq -r '.closure_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/payment-v1-generated.json"
)"
jq -e '
  .dependency_schemas
  | index("payments/contracts/payments/money.proto") != null
' "${SCHEMAHUB_EVIDENCE_DIR}/payment-v1-generated.json" >/dev/null
grep -Fq 'pub mod payments {' \
  "${SCHEMAHUB_EVIDENCE_DIR}/payment-v1.rs"
grep -Fq 'use super::super::super::payments;' \
  "${SCHEMAHUB_EVIDENCE_DIR}/payment-v1.rs"
grep -Fq 'pub use payments::capture::v1::*;' \
  "${SCHEMAHUB_EVIDENCE_DIR}/payment-v1.rs"

schemahub_note "Assert: reverse discovery identifies the importing payment schema"
human_schemahub schema dependents \
  payments/contracts/payments/money.proto \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/money-dependents.json"
jq -e '
  any(.dependents[];
    .importingProject == "payments"
    and .importingRepo == "contracts"
    and .importingSchema == "payments/payment.proto"
    and .importPath == "payments/contracts/payments/money.proto"
  )
' "${SCHEMAHUB_EVIDENCE_DIR}/money-dependents.json" >/dev/null

schemahub_note "Act: evolve the shared type while leaving the importing source unchanged"
agent_schemahub change note payments/contracts \
  --title "Add ISO currency to shared Money" \
  --description "Settlement needs currency while deployed readers continue decoding" \
  --reference PAYMENTS-1002 \
  --base-revision "${V1_COMMIT}" \
  --id payment-money-v2 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v2-01-note.json"
V2_NAME="$(jq -r '.name' "${SCHEMAHUB_EVIDENCE_DIR}/v2-01-note.json")"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v2-01-note.json")"
agent_schemahub change add-source "${V2_NAME}" \
  --etag "${ETAG}" \
  --schema-path payments/money.proto \
  --file "${SCENARIO_DIR}/fixtures/money-v2.proto" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v2-02-money.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v2-02-money.json")"
agent_schemahub change validate "${V2_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v2-03-validate.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v2-03-validate.json")"
jq -e '.validation.valid == true and (.validation.issues | length == 0)' \
  "${SCHEMAHUB_EVIDENCE_DIR}/v2-03-validate.json" >/dev/null
agent_schemahub change ready "${V2_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v2-04-ready.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v2-04-ready.json")"
human_schemahub change approve "${V2_NAME}" \
  --etag "${ETAG}" \
  --reason "Additive shared-type change and dependent impact reviewed" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v2-05-approve.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/v2-05-approve.json")"
agent_schemahub change apply "${V2_NAME}" \
  --etag "${ETAG}" \
  --request-id apply-payment-money-v2 \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v2-06-apply.json"
V2_COMMIT="$(
  jq -r '.apply_result.commit_id' "${SCHEMAHUB_EVIDENCE_DIR}/v2-06-apply.json"
)"

V2_REVISION_JSON="$(
  producer_schemahub artifact resolve payments/contracts --at main --json
)"
V2_REVISION="$(jq -r '.name' <<<"${V2_REVISION_JSON}")"
schemahub_assert_revision_commit "${V2_REVISION_JSON}" "${V2_COMMIT}"
printf '%s\n' "${V2_REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/v2-revision.json"
producer_schemahub artifact fetch "${V2_REVISION}" \
  --schema-path payments/payment.proto \
  --kind generated-code \
  --language rust \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/payment-v2.rs" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/payment-v2-generated.json"
V2_ARTIFACT_DIGEST="$(
  jq -r '.artifact_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/payment-v2-generated.json"
)"
V2_CLOSURE_DIGEST="$(
  jq -r '.closure_digest' \
    "${SCHEMAHUB_EVIDENCE_DIR}/payment-v2-generated.json"
)"
test "${V1_ARTIFACT_DIGEST}" != "${V2_ARTIFACT_DIGEST}"
test "${V1_CLOSURE_DIGEST}" != "${V2_CLOSURE_DIGEST}"
jq -e '
  .dependency_schemas
  | index("payments/contracts/payments/money.proto") != null
' "${SCHEMAHUB_EVIDENCE_DIR}/payment-v2-generated.json" >/dev/null

schemahub_note "Assert: served import closures compile and interoperate"
export SCHEMAHUB_DEPENDENCY_PROTO_V1_RS="${SCHEMAHUB_EVIDENCE_DIR}/payment-v1.rs"
export SCHEMAHUB_DEPENDENCY_PROTO_V2_RS="${SCHEMAHUB_EVIDENCE_DIR}/payment-v2.rs"
schemahub_run_consumer \
  protobuf_dependency \
  "${SCHEMAHUB_EVIDENCE_DIR}/payment-v2.bin" \
  | tee "${SCHEMAHUB_EVIDENCE_DIR}/consumer.txt"

schemahub_note "Act: propose deleting a still-imported shared schema"
agent_schemahub change note payments/contracts \
  --title "Remove the shared Money schema" \
  --description "Negative case: payment.proto still imports this file" \
  --reference PAYMENTS-NEGATIVE-1 \
  --base-revision "${V2_COMMIT}" \
  --id delete-imported-money \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/delete-01-note.json"
DELETE_NAME="$(
  jq -r '.name' "${SCHEMAHUB_EVIDENCE_DIR}/delete-01-note.json"
)"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/delete-01-note.json")"
agent_schemahub change delete-schema "${DELETE_NAME}" \
  --etag "${ETAG}" \
  --schema-path payments/money.proto \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/delete-02-edit.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/delete-02-edit.json")"
agent_schemahub change validate "${DELETE_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/delete-03-validate.json"
ETAG="$(jq -r '.etag' "${SCHEMAHUB_EVIDENCE_DIR}/delete-03-validate.json")"
jq -e '
  .validation.valid == false
  and (.validation.issues | length > 0)
' "${SCHEMAHUB_EVIDENCE_DIR}/delete-03-validate.json" >/dev/null
if agent_schemahub change ready "${DELETE_NAME}" \
  --etag "${ETAG}" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/delete-unexpected-ready.json" \
  2>"${SCHEMAHUB_EVIDENCE_DIR}/delete-ready-error.json"; then
  printf 'deletion of an imported schema unexpectedly became Ready\n' >&2
  exit 1
fi
jq -e '.error.grpc_code == "FAILED_PRECONDITION"' \
  "${SCHEMAHUB_EVIDENCE_DIR}/delete-ready-error.json" >/dev/null

schemahub_note "Assert: failed deletion leaves main and both immutable closures intact"
AFTER_DELETE="$(
  consumer_schemahub artifact resolve payments/contracts --at main --json
)"
schemahub_assert_revision_commit "${AFTER_DELETE}" "${V2_COMMIT}"
consumer_schemahub artifact verify "${V1_REVISION}" \
  --schema-path payments/payment.proto \
  --kind generated-code \
  --language rust \
  --digest "${V1_ARTIFACT_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null
consumer_schemahub artifact verify "${V2_REVISION}" \
  --schema-path payments/payment.proto \
  --kind generated-code \
  --language rust \
  --digest "${V2_ARTIFACT_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

jq -n \
  --arg v1_revision "${V1_REVISION}" \
  --arg v2_revision "${V2_REVISION}" \
  --arg v1_digest "${V1_ARTIFACT_DIGEST}" \
  --arg v2_digest "${V2_ARTIFACT_DIGEST}" \
  --arg v1_closure "${V1_CLOSURE_DIGEST}" \
  --arg v2_closure "${V2_CLOSURE_DIGEST}" \
  '{
    scenario: "RW-06",
    status: "passed",
    root_schema: "payments/payment.proto",
    dependency: "payments/contracts/payments/money.proto",
    revisions: {
      v1: {name: $v1_revision, artifact_digest: $v1_digest, closure_digest: $v1_closure},
      v2: {name: $v2_revision, artifact_digest: $v2_digest, closure_digest: $v2_closure}
    },
    discovery: "direct dependent found at an immutable snapshot",
    interoperability: "old and new cross-package generated closure bindings compiled and decoded both directions",
    imported_schema_deletion: "rejected before Ready"
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: cross-package closure codegen, discovery, evolution, and deletion guard verified"
