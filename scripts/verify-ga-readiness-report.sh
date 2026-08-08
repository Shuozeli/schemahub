#!/usr/bin/env bash

set -Eeuo pipefail

if [[ "$#" -ne 2 ]]; then
  printf 'Usage: verify-ga-readiness-report.sh ARCHIVE SOURCE_REVISION\n' >&2
  exit 2
fi

ARCHIVE="$1"
SOURCE_REVISION="$2"

for command_name in find grep jq sha256sum tar; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "${command_name}" >&2
    exit 2
  fi
done

if [[ ! -f "${ARCHIVE}" ]]; then
  printf 'GA readiness archive does not exist: %s\n' "${ARCHIVE}" >&2
  exit 2
fi
if [[ ! "${SOURCE_REVISION}" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; then
  printf 'source revision must be an immutable 40- or 64-character lowercase hex digest\n' >&2
  exit 2
fi
if tar -tzf "${ARCHIVE}" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
  printf 'GA readiness archive contains an unsafe path\n' >&2
  exit 1
fi

REPORT_DIR="$(mktemp -d /tmp/schemahub-ga-verify.XXXXXX)"
trap 'rm -rf -- "${REPORT_DIR}"' EXIT
tar \
  --extract \
  --gzip \
  --file "${ARCHIVE}" \
  --directory "${REPORT_DIR}" \
  --no-same-owner \
  --no-same-permissions

if [[ -n "$(
  find "${REPORT_DIR}" \
    ! -type d \
    ! -type f \
    -print \
    -quit
)" ]]; then
  printf 'GA readiness archive must contain only directories and regular files\n' >&2
  exit 1
fi
file_count="$(find "${REPORT_DIR}" -type f -print | wc -l | tr -d ' ')"
if [[ "${file_count}" != "9" ]]; then
  printf 'GA readiness archive must contain exactly 9 files, found %s\n' \
    "${file_count}" >&2
  exit 1
fi

REPORT_JSON="${REPORT_DIR}/ga-readiness.json"
REPORT_MARKDOWN="${REPORT_DIR}/GA-READINESS.md"
if [[ ! -s "${REPORT_JSON}" || ! -s "${REPORT_MARKDOWN}" ]]; then
  printf 'GA readiness archive is missing its human or machine report\n' >&2
  exit 1
fi

jq -e --arg revision "${SOURCE_REVISION}" '
  .schema_version == "schemahub.ga-readiness.v1"
  and .source.revision == $revision
  and .source.worktree_dirty == false
  and .source.provenance_status == "clean"
  and (.run.id | type == "string" and length > 0)
  and (.run.url | type == "string" and startswith("https://github.com/"))
  and .gate.status == "passed"
  and .gate.required_scenarios == 7
  and .gate.passed_scenarios == 7
  and .gate.open_findings.release_blocker == 0
  and .gate.open_findings.high == 0
  and .gate.release_authorized == false
  and (.scenarios | length == 7)
  and ([.scenarios[].id] | unique | length) == 7
  and ([.scenarios[].id] | sort) == [
    "RW-01", "RW-02", "RW-03", "RW-04",
    "RW-05", "RW-06", "RW-07"
  ]
  and (.scenarios | all(
    .status == "passed"
    and (.normalized_result_digest
      | test("^sha256:[0-9a-f]{64}$"))
  ))
' "${REPORT_JSON}" >/dev/null || {
  printf 'GA readiness machine report does not satisfy the release contract\n' >&2
  exit 1
}

verified_results=0
while IFS=$'\t' read -r scenario_id expected_digest; do
  result="${REPORT_DIR}/scenario-results/${scenario_id}.json"
  if [[ ! -f "${result}" ]]; then
    printf '%s normalized result is missing\n' "${scenario_id}" >&2
    exit 1
  fi
  jq -e \
    --arg scenario_id "${scenario_id}" \
    '.scenario == $scenario_id and .status == "passed"' \
    "${result}" >/dev/null || {
    printf '%s normalized result does not match its passing scenario\n' \
      "${scenario_id}" >&2
    exit 1
  }
  actual_digest="sha256:$(sha256sum "${result}" | cut -d ' ' -f 1)"
  if [[ "${actual_digest}" != "${expected_digest}" ]]; then
    printf '%s normalized result digest mismatch\n' "${scenario_id}" >&2
    exit 1
  fi
  verified_results="$((verified_results + 1))"
done < <(
  jq -r '
    .scenarios[]
    | [.id, .normalized_result_digest]
    | @tsv
  ' "${REPORT_JSON}"
)

if [[ "${verified_results}" != "7" ]]; then
  printf 'expected to verify 7 normalized results, verified %s\n' \
    "${verified_results}" >&2
  exit 1
fi

printf 'GA readiness archive verified for source %s\n' "${SOURCE_REVISION}"
