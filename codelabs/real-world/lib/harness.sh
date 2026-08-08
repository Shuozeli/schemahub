#!/usr/bin/env bash

set -Eeuo pipefail

SCHEMAHUB_CODELAB_LIB_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
SCHEMAHUB_CODELAB_ROOT="$(
  cd -- "${SCHEMAHUB_CODELAB_LIB_DIR}/.." >/dev/null 2>&1
  pwd
)"
SCHEMAHUB_REPO_ROOT="$(
  cd -- "${SCHEMAHUB_CODELAB_LIB_DIR}/../../.." >/dev/null 2>&1
  pwd
)"
SCHEMAHUB_CONSUMER_MANIFEST="${SCHEMAHUB_CODELAB_ROOT}/consumers/Cargo.toml"

SCHEMAHUB_HUMAN_TOKEN="codelab-human-token"
SCHEMAHUB_AGENT_TOKEN="codelab-agent-token"
SCHEMAHUB_PRODUCER_TOKEN="codelab-producer-token"
SCHEMAHUB_CONSUMER_TOKEN="codelab-consumer-token"

SCHEMAHUB_SERVER_PID=""
SCHEMAHUB_SCENARIO_ID=""
SCHEMAHUB_EVIDENCE_DIR=""
SCHEMAHUB_CONFIG=""
SCHEMAHUB_DB=""
SCHEMAHUB_SERVER_LOG=""
SCHEMAHUB_TRANSCRIPT=""
SCHEMAHUB_SERVER_URL=""
SCHEMAHUB_HTTP_SERVER_URL=""
SCHEMAHUB_CODELAB_HTTP_LISTEN=""
SCHEMAHUB_SERVER_BIN=""
SCHEMAHUB_CLI_BIN=""

schemahub_require_command() {
  local command_name="$1"
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "${command_name}" >&2
    return 1
  fi
}

schemahub_note() {
  local message="$*"
  printf '[%s] %s\n' "${SCHEMAHUB_SCENARIO_ID}" "${message}"
  printf '[%s] %s\n' "${SCHEMAHUB_SCENARIO_ID}" "${message}" >>"${SCHEMAHUB_TRANSCRIPT}"
}

schemahub_lab_init() {
  local scenario_id="$1"
  local default_port="$2"
  local default_http_port="${3:-}"

  SCHEMAHUB_SCENARIO_ID="${scenario_id}"
  schemahub_require_command cargo
  schemahub_require_command grep
  schemahub_require_command jq
  schemahub_require_command sha256sum

  if [[ -n "${SCHEMAHUB_CODELAB_EVIDENCE_DIR:-}" ]]; then
    SCHEMAHUB_EVIDENCE_DIR="${SCHEMAHUB_CODELAB_EVIDENCE_DIR}"
    mkdir -p "${SCHEMAHUB_EVIDENCE_DIR}"
  else
    SCHEMAHUB_EVIDENCE_DIR="$(
      mktemp -d "/tmp/schemahub-${scenario_id}.XXXXXX"
    )"
  fi

  SCHEMAHUB_CONFIG="${SCHEMAHUB_EVIDENCE_DIR}/schemahub.toml"
  SCHEMAHUB_DB="${SCHEMAHUB_EVIDENCE_DIR}/schemahub.redb"
  SCHEMAHUB_SERVER_LOG="${SCHEMAHUB_EVIDENCE_DIR}/server.log"
  SCHEMAHUB_TRANSCRIPT="${SCHEMAHUB_EVIDENCE_DIR}/transcript.log"
  : >"${SCHEMAHUB_TRANSCRIPT}"

  local bind_ip="${SCHEMAHUB_CODELAB_BIND_IP:-}"
  local client_host="${SCHEMAHUB_CODELAB_CLIENT_HOST:-}"
  if [[ -z "${bind_ip}" || -z "${client_host}" ]]; then
    if command -v tailscale >/dev/null 2>&1 \
      && tailscale ip -4 >/dev/null 2>&1; then
      bind_ip="${bind_ip:-$(tailscale ip -4)}"
      client_host="${client_host:-$(
        tailscale status --json \
          | jq -r '.Self.DNSName' \
          | sed 's/\.$//'
      )}"
    else
      bind_ip="${bind_ip:-0.0.0.0}"
      client_host="${client_host:-127.0.0.1}"
    fi
  fi

  local port="${SCHEMAHUB_CODELAB_PORT:-${default_port}}"
  SCHEMAHUB_SERVER_URL="http://${client_host}:${port}"
  SCHEMAHUB_CODELAB_LISTEN="${bind_ip}:${port}"
  if [[ -n "${default_http_port}" ]]; then
    local http_port="${SCHEMAHUB_CODELAB_HTTP_PORT:-${default_http_port}}"
    SCHEMAHUB_HTTP_SERVER_URL="http://${client_host}:${http_port}"
    SCHEMAHUB_CODELAB_HTTP_LISTEN="${bind_ip}:${http_port}"
  fi
  SCHEMAHUB_SERVER_BIN="${SCHEMAHUB_REPO_ROOT}/target/release/schemahub-server"
  SCHEMAHUB_CLI_BIN="${SCHEMAHUB_REPO_ROOT}/target/release/schemahub"

  if [[ "${SCHEMAHUB_CODELAB_SKIP_BUILD:-0}" != "1" ]]; then
    schemahub_note "Building release-mode server and CLI"
    (
      cd "${SCHEMAHUB_REPO_ROOT}"
      CARGO_INCREMENTAL=0 cargo build \
        --locked \
        --release \
        -p schemahub-server \
        -p schemahub-cli
    )
  fi

  if [[ ! -x "${SCHEMAHUB_SERVER_BIN}" || ! -x "${SCHEMAHUB_CLI_BIN}" ]]; then
    printf 'release binaries are missing; run without SCHEMAHUB_CODELAB_SKIP_BUILD=1\n' >&2
    return 1
  fi

  schemahub_note "Evidence directory: ${SCHEMAHUB_EVIDENCE_DIR}"
  schemahub_note "Client endpoint: ${SCHEMAHUB_SERVER_URL}"
  trap schemahub_cleanup EXIT
}

schemahub_write_config() {
  tee "${SCHEMAHUB_CONFIG}" >/dev/null
}

schemahub_cli_with_token() {
  local token="$1"
  shift
  "${SCHEMAHUB_CLI_BIN}" \
    --server "${SCHEMAHUB_SERVER_URL}" \
    --token "${token}" \
    --json-errors \
    "$@"
}

human_schemahub() {
  schemahub_cli_with_token "${SCHEMAHUB_HUMAN_TOKEN}" "$@"
}

agent_schemahub() {
  schemahub_cli_with_token "${SCHEMAHUB_AGENT_TOKEN}" "$@"
}

producer_schemahub() {
  schemahub_cli_with_token "${SCHEMAHUB_PRODUCER_TOKEN}" "$@"
}

consumer_schemahub() {
  schemahub_cli_with_token "${SCHEMAHUB_CONSUMER_TOKEN}" "$@"
}

schemahub_start() {
  schemahub_note "Starting release server on ${SCHEMAHUB_CODELAB_LISTEN}"
  local -a http_args=()
  if [[ -n "${SCHEMAHUB_CODELAB_HTTP_LISTEN}" ]]; then
    schemahub_note "Starting HTTP BFF on ${SCHEMAHUB_CODELAB_HTTP_LISTEN}"
    http_args=(--http-listen "${SCHEMAHUB_CODELAB_HTTP_LISTEN}")
  fi
  "${SCHEMAHUB_SERVER_BIN}" \
    --listen "${SCHEMAHUB_CODELAB_LISTEN}" \
    "${http_args[@]}" \
    --db "${SCHEMAHUB_DB}" \
    --config "${SCHEMAHUB_CONFIG}" \
    --log-format json \
    >>"${SCHEMAHUB_SERVER_LOG}" 2>&1 &
  SCHEMAHUB_SERVER_PID=$!

  local ready=0
  for _ in $(seq 1 60); do
    if agent_schemahub capabilities --json \
      >"${SCHEMAHUB_EVIDENCE_DIR}/capabilities.json" 2>/dev/null; then
      ready=1
      break
    fi
    if ! kill -0 "${SCHEMAHUB_SERVER_PID}" 2>/dev/null; then
      break
    fi
    sleep 1
  done
  if [[ "${ready}" != "1" ]]; then
    printf 'SchemaHub did not become ready; server log follows\n' >&2
    tail -120 "${SCHEMAHUB_SERVER_LOG}" >&2 || true
    return 1
  fi
}

schemahub_stop() {
  if [[ -n "${SCHEMAHUB_SERVER_PID}" ]] \
    && kill -0 "${SCHEMAHUB_SERVER_PID}" 2>/dev/null; then
    schemahub_note "Stopping server process ${SCHEMAHUB_SERVER_PID}"
    kill "${SCHEMAHUB_SERVER_PID}"
    wait "${SCHEMAHUB_SERVER_PID}" || true
  fi
  SCHEMAHUB_SERVER_PID=""
}

schemahub_restart() {
  schemahub_note "Restarting against the same redb database"
  schemahub_stop
  schemahub_start
}

schemahub_run_consumer() {
  local binary="$1"
  shift
  CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="${SCHEMAHUB_REPO_ROOT}/target/codelab-consumers" \
    cargo run \
      --locked \
      --release \
      --manifest-path "${SCHEMAHUB_CONSUMER_MANIFEST}" \
      --bin "${binary}" \
      -- "$@"
}

schemahub_assert_revision_commit() {
  local revision_json="$1"
  local expected_commit="$2"
  jq -e --arg commit "${expected_commit}" \
    '.commit_id == $commit' <<<"${revision_json}" >/dev/null
}

schemahub_cleanup() {
  local status=$?
  trap - EXIT
  schemahub_stop
  if [[ "${status}" -ne 0 ]]; then
    printf '[%s] FAILED; last server events:\n' "${SCHEMAHUB_SCENARIO_ID}" >&2
    tail -120 "${SCHEMAHUB_SERVER_LOG}" >&2 || true
  fi
  printf '[%s] evidence: %s\n' \
    "${SCHEMAHUB_SCENARIO_ID}" \
    "${SCHEMAHUB_EVIDENCE_DIR}"
  exit "${status}"
}
