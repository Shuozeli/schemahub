#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
REPO_ROOT="$(
  cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1
  pwd
)"

IMAGE="${SCHEMAHUB_CONTAINER_IMAGE:-schemahub:ci}"
CURL_IMAGE="${SCHEMAHUB_CONTAINER_CURL_IMAGE:-curlimages/curl:8.14.1@sha256:9a1ed35addb45476afa911696297f8e115993df459278ed036182dd2cd22b67b}"
RUN_ID="$(
  printf '%s' \
    "${SCHEMAHUB_CONTAINER_RUN_ID:-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$}"
)"

if [[ ! "${RUN_ID}" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  printf 'container smoke run ID contains unsupported characters: %s\n' \
    "${RUN_ID}" >&2
  exit 2
fi

INITIAL_CONTAINER="schemahub-container-${RUN_ID}-initial"
REPLACEMENT_CONTAINER="schemahub-container-${RUN_ID}-replacement"
NETWORK="schemahub-container-${RUN_ID}"
VOLUME="schemahub-container-${RUN_ID}-data"
CONFIG_FIXTURE="${REPO_ROOT}/tests/integration/container-smoke.toml"
SCHEMA_FIXTURE="${REPO_ROOT}/tests/integration/user.proto"
GUI_CONTENT_SECURITY_POLICY="default-src 'self'; base-uri 'none'; connect-src 'self'; font-src 'self'; form-action 'none'; frame-ancestors 'none'; frame-src 'none'; img-src 'self' data:; media-src 'none'; object-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'"

for command_name in docker grep head jq sed seq tr; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "${command_name}" >&2
    exit 2
  fi
done
for fixture in "${CONFIG_FIXTURE}" "${SCHEMA_FIXTURE}"; do
  if [[ ! -s "${fixture}" ]]; then
    printf 'container smoke fixture is missing or empty: %s\n' "${fixture}" >&2
    exit 2
  fi
done
docker image inspect "${IMAGE}" >/dev/null

if docker container inspect "${INITIAL_CONTAINER}" >/dev/null 2>&1 \
  || docker container inspect "${REPLACEMENT_CONTAINER}" >/dev/null 2>&1 \
  || docker network inspect "${NETWORK}" >/dev/null 2>&1 \
  || docker volume inspect "${VOLUME}" >/dev/null 2>&1
then
  printf 'container smoke resource already exists for run ID: %s\n' \
    "${RUN_ID}" >&2
  exit 2
fi

cleanup() {
  docker container rm --force --volumes \
    "${INITIAL_CONTAINER}" >/dev/null 2>&1 || true
  docker container rm --force --volumes \
    "${REPLACEMENT_CONTAINER}" >/dev/null 2>&1 || true
  docker network rm "${NETWORK}" >/dev/null 2>&1 || true
  docker volume rm "${VOLUME}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_until_healthy() {
  local target="$1"
  local health_state="starting"

  for _ in $(seq 1 30); do
    health_state="$(
      docker container inspect \
        --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' \
        "${target}"
    )"
    if [[ "${health_state}" == "healthy" ]]; then
      return 0
    fi
    if [[ "${health_state}" == "unhealthy" ]] \
      || [[ "$(
        docker container inspect \
          --format '{{.State.Running}}' \
          "${target}"
      )" != "true" ]]
    then
      docker container logs "${target}" >&2
      return 1
    fi
    sleep 2
  done

  docker container logs "${target}" >&2
  printf 'container did not become healthy: %s (%s)\n' \
    "${target}" \
    "${health_state}" >&2
  return 1
}

start_container() {
  local target="$1"

  docker run --detach \
    --name "${target}" \
    --network "${NETWORK}" \
    --volume "${VOLUME}:/var/lib/schemahub" \
    --volume "${SCHEMA_FIXTURE}:/fixtures/user.proto:ro" \
    --volume "${CONFIG_FIXTURE}:/etc/schemahub/schemahub.toml:ro" \
    --tmpfs /tmp:rw,noexec,nosuid,nodev,uid=65532,gid=65532,mode=1777 \
    "${IMAGE}" \
    --listen 0.0.0.0:50051 \
    --http-listen 0.0.0.0:8080 \
    --gui-dir /usr/share/schemahub/gui \
    --db /var/lib/schemahub/schemahub.redb \
    --config /etc/schemahub/schemahub.toml >/dev/null
  wait_until_healthy "${target}"
}

schemahub_cli() {
  local target="$1"
  shift

  docker exec "${target}" schemahub \
    --server http://127.0.0.1:50051 \
    --token container-smoke-owner-token \
    "$@"
}

http_from_network() {
  docker run --rm \
    --network "${NETWORK}" \
    "${CURL_IMAGE}" \
    "$@"
}

# Arrange: create the isolated network and named durable volume.
docker network create "${NETWORK}" >/dev/null
docker volume create "${VOLUME}" >/dev/null

# Act: boot the exact image as its configured non-root user.
start_container "${INITIAL_CONTAINER}"

# Assert: probes, the bundled GUI, route boundaries, and capabilities work.
test "$(
  docker container inspect \
    --format '{{.Config.User}}' \
    "${INITIAL_CONTAINER}"
)" = "65532:65532"
http_from_network \
  --fail --silent \
  "http://${INITIAL_CONTAINER}:8080/readyz" >/dev/null
GUI_INDEX="$(
  http_from_network \
    --fail --silent \
    "http://${INITIAL_CONTAINER}:8080/"
)"
grep -q '<title>SchemaHub Console</title>' <<<"${GUI_INDEX}"
GUI_INDEX_HEADERS="$(
  http_from_network \
    --fail --silent --dump-header - --output /dev/null \
    "http://${INITIAL_CONTAINER}:8080/" \
    | tr -d '\r'
)"
grep -Fxiq 'cache-control: no-cache' <<<"${GUI_INDEX_HEADERS}"
grep -Fxiq 'x-content-type-options: nosniff' <<<"${GUI_INDEX_HEADERS}"
grep -Fxiq 'referrer-policy: same-origin' <<<"${GUI_INDEX_HEADERS}"
grep -Fxiq 'permissions-policy: camera=(), geolocation=(), microphone=()' \
  <<<"${GUI_INDEX_HEADERS}"
grep -Fxiq 'x-frame-options: DENY' <<<"${GUI_INDEX_HEADERS}"
grep -Fxiq \
  "content-security-policy: ${GUI_CONTENT_SECURITY_POLICY}" \
  <<<"${GUI_INDEX_HEADERS}"
http_from_network \
  --fail --silent \
  "http://${INITIAL_CONTAINER}:8080/projects/acme/repos/commerce/changes" \
  | grep -q '<title>SchemaHub Console</title>'
GUI_ASSET="$(
  grep -o '/assets/[^"]*[.]js' <<<"${GUI_INDEX}" \
    | head -n 1
)"
test -n "${GUI_ASSET}"
GUI_ASSET_HEADERS="$(
  http_from_network \
    --fail --silent --dump-header - --output /dev/null \
    "http://${INITIAL_CONTAINER}:8080${GUI_ASSET}" \
    | tr -d '\r'
)"
grep -Fxiq \
  'cache-control: public, max-age=31536000, immutable' \
  <<<"${GUI_ASSET_HEADERS}"
grep -Fxiq 'x-content-type-options: nosniff' <<<"${GUI_ASSET_HEADERS}"
grep -Fxiq \
  "content-security-policy: ${GUI_CONTENT_SECURITY_POLICY}" \
  <<<"${GUI_ASSET_HEADERS}"
http_from_network \
  --fail --silent \
  "http://${INITIAL_CONTAINER}:8080/metrics" \
  | grep -q 'schemahub_build_info'
docker exec "${INITIAL_CONTAINER}" schemahub \
  --server http://127.0.0.1:50051 \
  capabilities \
  --json \
  | jq -e '
      .matrix_version == "1.0"
      and ([.formats[].format_id] | sort)
        == ["flatbuffers", "openapi", "protobuf"]
    ' >/dev/null
UNKNOWN_HEADERS="$(
  http_from_network \
    --silent --dump-header - --output /dev/null \
    "http://${INITIAL_CONTAINER}:8080/api/not-a-route" \
    | tr -d '\r'
)"
grep -Eq '^HTTP/[0-9.]+ 404 ' <<<"${UNKNOWN_HEADERS}"
grep -Fxiq \
  'x-schemahub-api-surface: gui-bff' \
  <<<"${UNKNOWN_HEADERS}"

# Arrange: create durable schema state and materialize immutable artifacts.
schemahub_cli \
  "${INITIAL_CONTAINER}" \
  repo init acceptance/registry --public
schemahub_cli \
  "${INITIAL_CONTAINER}" \
  schema create \
  --project acceptance \
  --repo registry \
  --name user.proto \
  /fixtures/user.proto
SCHEMA_BEFORE="$(
  schemahub_cli \
    "${INITIAL_CONTAINER}" \
    schema pull acceptance/registry/user.proto
)"
REVISION_JSON="$(
  schemahub_cli \
    "${INITIAL_CONTAINER}" \
    artifact resolve acceptance/registry --at main --json
)"
REVISION="$(jq -r .name <<<"${REVISION_JSON}")"
jq -e '
  .name
    | test(
        "^projects/acceptance/repos/registry/revisions/[0-9a-f]{128}$"
      )
' <<<"${REVISION_JSON}" >/dev/null

# Act: fetch and verify descriptor plus generated Rust bytes.
DESCRIPTOR_JSON="$(
  schemahub_cli \
    "${INITIAL_CONTAINER}" \
    artifact fetch "${REVISION}" \
    --schema-path user.proto \
    --kind descriptors \
    --output /tmp/user.desc \
    --json
)"
DESCRIPTOR_DIGEST="$(
  jq -r .artifact_digest <<<"${DESCRIPTOR_JSON}"
)"
[[ "${DESCRIPTOR_DIGEST}" == sha256:* ]]
schemahub_cli \
  "${INITIAL_CONTAINER}" \
  artifact verify "${REVISION}" \
  --schema-path user.proto \
  --kind descriptors \
  --digest "${DESCRIPTOR_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

GENERATED_JSON="$(
  schemahub_cli \
    "${INITIAL_CONTAINER}" \
    artifact fetch "${REVISION}" \
    --schema-path user.proto \
    --kind generated-code \
    --language rust \
    --output /tmp/user.rs \
    --json
)"
GENERATED_DIGEST="$(
  jq -r .artifact_digest <<<"${GENERATED_JSON}"
)"
[[ "${GENERATED_DIGEST}" == sha256:* ]]
schemahub_cli \
  "${INITIAL_CONTAINER}" \
  artifact verify "${REVISION}" \
  --schema-path user.proto \
  --kind generated-code \
  --language rust \
  --digest "${GENERATED_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null

# Assert: the first process drains cleanly before replacement.
docker stop --timeout 35 "${INITIAL_CONTAINER}" >/dev/null
test "$(
  docker container inspect \
    --format '{{.State.ExitCode}}' \
    "${INITIAL_CONTAINER}"
)" = "0"
docker container rm "${INITIAL_CONTAINER}" >/dev/null

# Act: replace the container while retaining only its named data volume.
start_container "${REPLACEMENT_CONTAINER}"
SCHEMA_AFTER="$(
  schemahub_cli \
    "${REPLACEMENT_CONTAINER}" \
    schema pull acceptance/registry/user.proto
)"
REVISION_AFTER="$(
  schemahub_cli \
    "${REPLACEMENT_CONTAINER}" \
    artifact resolve acceptance/registry --at main --json \
    | jq -r .name
)"

# Assert: schema coordinates and materialized bytes remain identical.
test "${SCHEMA_AFTER}" = "${SCHEMA_BEFORE}"
test "${REVISION_AFTER}" = "${REVISION}"
schemahub_cli \
  "${REPLACEMENT_CONTAINER}" \
  artifact verify "${REVISION}" \
  --schema-path user.proto \
  --kind descriptors \
  --digest "${DESCRIPTOR_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null
schemahub_cli \
  "${REPLACEMENT_CONTAINER}" \
  artifact verify "${REVISION}" \
  --schema-path user.proto \
  --kind generated-code \
  --language rust \
  --digest "${GENERATED_DIGEST}" \
  --json \
  | jq -e '.valid == true' >/dev/null
docker stop --timeout 35 "${REPLACEMENT_CONTAINER}" >/dev/null
test "$(
  docker container inspect \
    --format '{{.State.ExitCode}}' \
    "${REPLACEMENT_CONTAINER}"
)" = "0"

printf '%s\n' \
  "Runtime container persistence acceptance passed." \
  "revision=${REVISION}" \
  "descriptor_digest=${DESCRIPTOR_DIGEST}" \
  "generated_digest=${GENERATED_DIGEST}"
