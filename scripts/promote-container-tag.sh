#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
  printf '%s\n' \
    "Usage: promote-container-tag.sh CONTAINER_IMAGE VERSION CONTAINER_DIGEST" \
    "" \
    "Creates CONTAINER_IMAGE:VERSION from the already-pushed immutable digest." \
    "An existing version tag is accepted only when it names that exact digest." \
    >&2
}

if [[ "$#" -ne 3 ]]; then
  usage
  exit 2
fi

CONTAINER_IMAGE="$1"
VERSION="$2"
CONTAINER_DIGEST="$3"
DOCKER_COMMAND="${SCHEMAHUB_DOCKER_COMMAND:-docker}"
SEMVER_PATTERN='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
IMAGE_PATTERN='^ghcr[.]io/[a-z0-9][a-z0-9._/-]*$'
DIGEST_PATTERN='^sha256:[0-9a-f]{64}$'

if [[ ! "${CONTAINER_IMAGE}" =~ ${IMAGE_PATTERN} ]]; then
  printf 'container image must be a canonical lowercase GHCR repository\n' >&2
  exit 2
fi
if [[ ! "${VERSION}" =~ ${SEMVER_PATTERN} ]]; then
  printf 'container version must be MAJOR.MINOR.PATCH[-PRERELEASE]\n' >&2
  exit 2
fi
if [[ ! "${CONTAINER_DIGEST}" =~ ${DIGEST_PATTERN} ]]; then
  printf 'container digest must be an immutable SHA-256 digest\n' >&2
  exit 2
fi
for command_name in "${DOCKER_COMMAND}" awk; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "${command_name}" >&2
    exit 2
  fi
done

SOURCE="${CONTAINER_IMAGE}@${CONTAINER_DIGEST}"
TARGET="${CONTAINER_IMAGE}:${VERSION}"

inspect_digest() {
  local coordinate="$1"
  local inspect_output
  local resolved_digest

  if ! inspect_output="$(
    "${DOCKER_COMMAND}" buildx imagetools inspect "${coordinate}" 2>/dev/null
  )"; then
    return 1
  fi
  resolved_digest="$(
    awk '$1 == "Digest:" { print $2; exit }' <<<"${inspect_output}"
  )"
  if [[ ! "${resolved_digest}" =~ ${DIGEST_PATTERN} ]]; then
    printf 'registry inspection returned no immutable digest: %s\n' \
      "${coordinate}" >&2
    return 2
  fi
  printf '%s\n' "${resolved_digest}"
}

SOURCE_DIGEST="$(
  inspect_digest "${SOURCE}"
)" || {
  printf 'immutable candidate image is unavailable: %s\n' "${SOURCE}" >&2
  exit 1
}
if [[ "${SOURCE_DIGEST}" != "${CONTAINER_DIGEST}" ]]; then
  printf 'candidate image resolved to an unexpected digest: %s\n' \
    "${SOURCE_DIGEST}" >&2
  exit 1
fi

set +e
EXISTING_DIGEST="$(
  inspect_digest "${TARGET}"
)"
INSPECT_STATUS="$?"
set -e

if [[ "${INSPECT_STATUS}" -eq 0 ]]; then
  if [[ "${EXISTING_DIGEST}" != "${CONTAINER_DIGEST}" ]]; then
    printf 'refusing to overwrite version tag %s at %s\n' \
      "${TARGET}" \
      "${EXISTING_DIGEST}" >&2
    exit 1
  fi
elif [[ "${INSPECT_STATUS}" -eq 1 ]]; then
  "${DOCKER_COMMAND}" buildx imagetools create \
    --tag "${TARGET}" \
    "${SOURCE}"
else
  printf 'could not safely inspect version tag: %s\n' "${TARGET}" >&2
  exit 1
fi

PUBLISHED_DIGEST="$(
  inspect_digest "${TARGET}"
)" || {
  printf 'published version tag cannot be resolved: %s\n' "${TARGET}" >&2
  exit 1
}
if [[ "${PUBLISHED_DIGEST}" != "${CONTAINER_DIGEST}" ]]; then
  printf 'published version tag digest mismatch: expected %s, got %s\n' \
    "${CONTAINER_DIGEST}" \
    "${PUBLISHED_DIGEST}" >&2
  exit 1
fi

printf 'Container tag verified: %s@%s\n' \
  "${TARGET}" \
  "${PUBLISHED_DIGEST}"
