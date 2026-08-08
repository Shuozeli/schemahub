#!/usr/bin/env bash

set -Eeuo pipefail

: "${FAKE_DOCKER_MODE:?}"
: "${FAKE_DOCKER_STATE:?}"
: "${FAKE_DOCKER_LOG:?}"
: "${FAKE_DOCKER_EXPECTED_DIGEST:?}"
: "${FAKE_DOCKER_OTHER_DIGEST:?}"

if [[ "$#" -lt 4 || "$1" != "buildx" || "$2" != "imagetools" ]]; then
  printf 'unexpected fake docker invocation: %s\n' "$*" >&2
  exit 2
fi

print_inspect() {
  local coordinate="$1"
  local digest="$2"
  printf 'Name: %s\nMediaType: application/vnd.oci.image.index.v1+json\nDigest: %s\n' \
    "${coordinate}" \
    "${digest}"
}

case "$3" in
  inspect)
    coordinate="$4"
    if [[ "${coordinate}" == *@* ]]; then
      if [[ "${FAKE_DOCKER_MODE}" == "missing-source" ]]; then
        exit 1
      elif [[ "${FAKE_DOCKER_MODE}" == "bad-source" ]]; then
        print_inspect "${coordinate}" "${FAKE_DOCKER_OTHER_DIGEST}"
      else
        print_inspect "${coordinate}" "${FAKE_DOCKER_EXPECTED_DIGEST}"
      fi
      exit 0
    fi

    case "${FAKE_DOCKER_MODE}" in
      matching)
        print_inspect "${coordinate}" "${FAKE_DOCKER_EXPECTED_DIGEST}"
        ;;
      mismatch)
        print_inspect "${coordinate}" "${FAKE_DOCKER_OTHER_DIGEST}"
        ;;
      missing | bad-source | missing-source)
        if [[ ! -f "${FAKE_DOCKER_STATE}" ]]; then
          exit 1
        fi
        print_inspect "${coordinate}" "${FAKE_DOCKER_EXPECTED_DIGEST}"
        ;;
      wrong-after-create)
        if [[ ! -f "${FAKE_DOCKER_STATE}" ]]; then
          exit 1
        fi
        print_inspect "${coordinate}" "${FAKE_DOCKER_OTHER_DIGEST}"
        ;;
      *)
        printf 'unknown fake docker mode: %s\n' "${FAKE_DOCKER_MODE}" >&2
        exit 2
        ;;
    esac
    ;;
  create)
    if [[ "$#" -ne 6 || "$4" != "--tag" ]]; then
      printf 'unexpected imagetools create invocation: %s\n' "$*" >&2
      exit 2
    fi
    printf 'create %s %s\n' "$5" "$6" >>"${FAKE_DOCKER_LOG}"
    touch "${FAKE_DOCKER_STATE}"
    ;;
  *)
    printf 'unexpected imagetools command: %s\n' "$3" >&2
    exit 2
    ;;
esac
