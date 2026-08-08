#!/usr/bin/env bash

set -Eeuo pipefail

GUI_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." >/dev/null 2>&1
  pwd
)"
REPO_ROOT="$(
  cd -- "${GUI_DIR}/../.." >/dev/null 2>&1
  pwd
)"
source "${REPO_ROOT}/codelabs/real-world/lib/harness.sh"

schemahub_lab_init "gui-live-browser" 50108 58108

for command_name in cmp curl node pnpm; do
  schemahub_require_command "${command_name}"
done

GUI_URL="${SCHEMAHUB_GUI_URL:-}"
if [[ -z "${GUI_URL}" ]]; then
  printf 'SCHEMAHUB_GUI_URL is required; use the full Tailscale MagicDNS URL outside CI\n' >&2
  exit 2
fi
GUI_URL="${GUI_URL%/}"
if [[ ! "${GUI_URL}" =~ ^https?://[^/]+$ ]]; then
  printf 'SCHEMAHUB_GUI_URL must be an origin without a path, query, or fragment\n' >&2
  exit 2
fi
GUI_HOST="$(
  node -e 'process.stdout.write(new URL(process.argv[1]).hostname)' "${GUI_URL}"
)"
GUI_PORT="$(
  node -e 'const u = new URL(process.argv[1]); process.stdout.write(u.port || (u.protocol === "https:" ? "443" : "80"))' "${GUI_URL}"
)"
GUI_BIND_IP="${SCHEMAHUB_GUI_BIND_IP:-${SCHEMAHUB_CODELAB_BIND_IP:-}}"
if [[ -z "${GUI_BIND_IP}" ]]; then
  if command -v tailscale >/dev/null 2>&1 \
    && tailscale ip -4 >/dev/null 2>&1; then
    GUI_BIND_IP="$(tailscale ip -4)"
  else
    GUI_BIND_IP="0.0.0.0"
  fi
fi

schemahub_write_config <<EOF
[http]
allowed_origins = ["${GUI_URL}"]
max_request_body_bytes = 8388608

[auth]
data_dir = "${SCHEMAHUB_EVIDENCE_DIR}/legacy-auth"

[auth.tokens.codelab-human-token]
id = "gui-owner"
display = "GUI Owner"
kind = "human"

[auth.tokens.codelab-agent-token]
id = "gui-agent"
display = "Delegated GUI Agent"
kind = "agent"
delegated_by = "gui-owner"

[auth.tokens.codelab-consumer-token]
id = "gui-consumer"
display = "GUI Artifact Consumer"
kind = "service"

[projects.gui]
visibility = "private"
owners = ["gui-owner"]
members = { gui-agent = "Writer", gui-consumer = "Reader" }

[repos."gui/contracts"]
default_bookmark = "main"
compatibility = "full"
protected_bookmarks = ["main"]

[repos."gui/contracts".review]
required_approvals = 1
require_change_record = true

[repos."gui/contracts".serving]
source = true
descriptors = true
generated_code = true
EOF

GUI_PID=""
GUI_LOG="${SCHEMAHUB_EVIDENCE_DIR}/gui.log"

gui_live_cleanup() {
  local status=$?
  trap - EXIT
  if [[ -n "${GUI_PID}" ]] && kill -0 "${GUI_PID}" 2>/dev/null; then
    kill "${GUI_PID}" >/dev/null 2>&1 || true
    wait "${GUI_PID}" >/dev/null 2>&1 || true
  fi
  schemahub_stop
  if [[ "${status}" -ne 0 ]]; then
    printf '[gui-live-browser] FAILED; GUI log follows:\n' >&2
    tail -120 "${GUI_LOG}" >&2 || true
  fi
  printf '[gui-live-browser] evidence: %s\n' "${SCHEMAHUB_EVIDENCE_DIR}"
  exit "${status}"
}
trap gui_live_cleanup EXIT

schemahub_start
human_schemahub repo init gui/contracts \
  >"${SCHEMAHUB_EVIDENCE_DIR}/repo-init.txt"

schemahub_note "Starting live GUI on ${GUI_BIND_IP}:${GUI_PORT}"
(
  cd "${GUI_DIR}"
  TAILSCALE_IP="${GUI_BIND_IP}" \
    TAILSCALE_HOST="${GUI_HOST}" \
    VITE_SCHEMAHUB_API_BASE="${SCHEMAHUB_HTTP_SERVER_URL}" \
    VITE_SCHEMAHUB_USE_MOCKS="false" \
    pnpm exec vite \
      --host "${GUI_BIND_IP}" \
      --port "${GUI_PORT}" \
      --strictPort
) >"${GUI_LOG}" 2>&1 &
GUI_PID=$!

gui_ready=0
for _ in $(seq 1 60); do
  if curl --fail --silent --show-error "${GUI_URL}" >/dev/null 2>&1; then
    gui_ready=1
    break
  fi
  if ! kill -0 "${GUI_PID}" 2>/dev/null; then
    break
  fi
  sleep 1
done
if [[ "${gui_ready}" != "1" ]]; then
  printf 'live GUI did not become ready\n' >&2
  exit 1
fi

schemahub_note "Act: agent authors, human approves, and agent applies through the live browser"
(
  cd "${GUI_DIR}"
  PLAYWRIGHT_CHROMIUM_EXECUTABLE="${PLAYWRIGHT_CHROMIUM_EXECUTABLE:-}" \
    SCHEMAHUB_GUI_URL="${GUI_URL}" \
    SCHEMAHUB_GUI_AGENT_TOKEN="${SCHEMAHUB_AGENT_TOKEN}" \
    SCHEMAHUB_GUI_HUMAN_TOKEN="${SCHEMAHUB_HUMAN_TOKEN}" \
    SCHEMAHUB_GUI_SCREENSHOT="${SCHEMAHUB_EVIDENCE_DIR}/browser.png" \
    pnpm run test:browser:live
) | tee "${SCHEMAHUB_EVIDENCE_DIR}/browser.txt"

agent_schemahub change list gui/contracts --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/changes.json"
CHANGE_COUNT="$(
  jq '[.changes[] | select(.title == "Live browser governed contract")] | length' \
    "${SCHEMAHUB_EVIDENCE_DIR}/changes.json"
)"
if [[ "${CHANGE_COUNT}" != "1" ]]; then
  printf 'expected one live-browser ChangeRecord, found %s\n' "${CHANGE_COUNT}" >&2
  exit 1
fi
CHANGE_NAME="$(
  jq -r '.changes[] | select(.title == "Live browser governed contract") | .name' \
    "${SCHEMAHUB_EVIDENCE_DIR}/changes.json"
)"
COMMIT_ID="$(
  jq -r '.changes[] | select(.title == "Live browser governed contract") | .apply_result.commit_id' \
    "${SCHEMAHUB_EVIDENCE_DIR}/changes.json"
)"
jq -e '
  .changes[]
  | select(.title == "Live browser governed contract")
  | .status == "applied"
    and .created_by.identity == "gui-agent"
    and .created_by.kind == "agent"
    and .created_by.delegated_by == "gui-owner"
    and .reviews[0].reviewer.identity == "gui-owner"
    and .reviews[0].decision == "approved"
    and (.apply_result.commit_id | length > 0)
' "${SCHEMAHUB_EVIDENCE_DIR}/changes.json" >/dev/null

REVISION_JSON="$(
  consumer_schemahub artifact resolve gui/contracts --at main --json
)"
REVISION="$(jq -r '.name' <<<"${REVISION_JSON}")"
schemahub_assert_revision_commit "${REVISION_JSON}" "${COMMIT_ID}"
printf '%s\n' "${REVISION_JSON}" \
  >"${SCHEMAHUB_EVIDENCE_DIR}/revision.json"
consumer_schemahub artifact fetch "${REVISION}" \
  --schema-path schemas/live-browser.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/live-browser.desc" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/descriptor.json"
DESCRIPTOR_DIGEST="$(
  jq -r '.artifact_digest' "${SCHEMAHUB_EVIDENCE_DIR}/descriptor.json"
)"

schemahub_note "Assert: browser-created audit and descriptor bytes survive restart"
schemahub_restart
human_schemahub change get "${CHANGE_NAME}" --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/change-after-restart.json"
jq -e \
  --arg commit "${COMMIT_ID}" \
  '.status == "applied"
    and .created_by.identity == "gui-agent"
    and .created_by.delegated_by == "gui-owner"
    and .reviews[0].reviewer.identity == "gui-owner"
    and .apply_result.commit_id == $commit' \
  "${SCHEMAHUB_EVIDENCE_DIR}/change-after-restart.json" >/dev/null
consumer_schemahub artifact fetch "${REVISION}" \
  --schema-path schemas/live-browser.proto \
  --kind descriptors \
  --output "${SCHEMAHUB_EVIDENCE_DIR}/live-browser-after-restart.desc" \
  --json \
  >"${SCHEMAHUB_EVIDENCE_DIR}/descriptor-after-restart.json"
cmp \
  "${SCHEMAHUB_EVIDENCE_DIR}/live-browser.desc" \
  "${SCHEMAHUB_EVIDENCE_DIR}/live-browser-after-restart.desc"
jq -e \
  --arg digest "${DESCRIPTOR_DIGEST}" \
  '.artifact_digest == $digest' \
  "${SCHEMAHUB_EVIDENCE_DIR}/descriptor-after-restart.json" >/dev/null

jq -n \
  --arg change "${CHANGE_NAME}" \
  --arg commit "${COMMIT_ID}" \
  --arg revision "${REVISION}" \
  --arg descriptor_digest "${DESCRIPTOR_DIGEST}" \
  '{
    schema_version: "schemahub.gui-live-acceptance.v1",
    status: "passed",
    browser: {
      agent_authored_source: true,
      pre_review_apply: "rejected",
      independent_human_review: "approved",
      agent_apply: "succeeded",
      live_schema_detail: "rendered"
    },
    audit: {
      change: $change,
      author: "gui-agent",
      delegated_by: "gui-owner",
      reviewer: "gui-owner",
      commit: $commit
    },
    serving: {
      revision: $revision,
      descriptor_digest: $descriptor_digest,
      restart_bytes: "identical"
    }
  }' >"${SCHEMAHUB_EVIDENCE_DIR}/result.json"

schemahub_note "PASS: live browser governance, Apply, audit, and restart serving verified"
