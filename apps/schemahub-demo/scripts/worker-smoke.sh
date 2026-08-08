#!/usr/bin/env bash
set -euo pipefail

worker_ip="${TAILSCALE_IP:-0.0.0.0}"
probe_host="${TAILSCALE_HOST:-${TAILSCALE_IP:-127.0.0.1}}"
worker_port="${SCHEMAHUB_DEMO_WORKER_PORT:-4179}"
worker_log="$(mktemp)"
response_body="$(mktemp)"
worker_pid=""

cleanup() {
  if [[ -n "$worker_pid" ]]; then
    kill "$worker_pid" >/dev/null 2>&1 || true
    wait "$worker_pid" >/dev/null 2>&1 || true
  fi
  rm -f "$worker_log" "$response_body"
}
trap cleanup EXIT

# Arrange
pnpm exec wrangler dev \
  --local \
  --ip "$worker_ip" \
  --port "$worker_port" >"$worker_log" 2>&1 &
worker_pid="$!"
worker_url="http://${probe_host}:${worker_port}/"

# Act
ready=false
for _ in $(seq 1 30); do
  if curl --fail --silent --output "$response_body" "$worker_url"; then
    ready=true
    break
  fi
  if ! kill -0 "$worker_pid" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

# Assert
if [[ "$ready" != true ]]; then
  cat "$worker_log" >&2
  exit 1
fi
grep -Fq "<title>SchemaHub Workflow Lab</title>" "$response_body"
grep -Fq "Use reality to find the bugs." "$response_body"
grep -Fq "Human + agent approval" "$response_body"

echo "OpenNext workerd smoke passed at $worker_url"
