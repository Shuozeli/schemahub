#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
  printf 'Usage: validate-release-findings.sh VERSION FINDINGS_FILE\n' >&2
}

if [[ "$#" -ne 2 ]]; then
  usage
  exit 2
fi

VERSION="$1"
FINDINGS_FILE="$2"
SEMVER_PATTERN='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'

if [[ ! "${VERSION}" =~ ${SEMVER_PATTERN} ]]; then
  printf 'release version must be MAJOR.MINOR.PATCH[-PRERELEASE]: %s\n' \
    "${VERSION}" >&2
  exit 2
fi
if [[ ! -s "${FINDINGS_FILE}" ]]; then
  printf 'release finding ledger is missing or empty: %s\n' \
    "${FINDINGS_FILE}" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  printf 'required command is missing: jq\n' >&2
  exit 2
fi

jq -e '
  .schema_version == "schemahub.ga-findings.v1"
  and (.findings | type == "array")
  and (.findings | all(
    (.id | type == "string" and length > 0)
    and (.state | IN("open", "fixed"))
    and (
      .must_fix_before == null
      or (
        .must_fix_before
        | type == "string"
          and test("^[0-9]+[.][0-9]+[.][0-9]+$")
      )
    )
  ))
  and (([.findings[].id] | length) == ([.findings[].id] | unique | length))
' "${FINDINGS_FILE}" >/dev/null || {
  printf 'release finding ledger has malformed IDs, states, or deadlines\n' >&2
  exit 2
}

version_reaches_deadline() {
  local candidate="$1"
  local deadline="$2"
  local candidate_core="${candidate%%-*}"
  local candidate_prerelease=false
  local candidate_major_text
  local candidate_minor_text
  local candidate_patch_text
  local deadline_major_text
  local deadline_minor_text
  local deadline_patch_text

  if [[ "${candidate}" == *-* ]]; then
    candidate_prerelease=true
  fi

  IFS=. read -r \
    candidate_major_text \
    candidate_minor_text \
    candidate_patch_text <<<"${candidate_core}"
  IFS=. read -r \
    deadline_major_text \
    deadline_minor_text \
    deadline_patch_text <<<"${deadline}"

  local candidate_major=$((10#${candidate_major_text}))
  local candidate_minor=$((10#${candidate_minor_text}))
  local candidate_patch=$((10#${candidate_patch_text}))
  local deadline_major=$((10#${deadline_major_text}))
  local deadline_minor=$((10#${deadline_minor_text}))
  local deadline_patch=$((10#${deadline_patch_text}))

  if (( candidate_major != deadline_major )); then
    (( candidate_major > deadline_major ))
    return
  fi
  if (( candidate_minor != deadline_minor )); then
    (( candidate_minor > deadline_minor ))
    return
  fi
  if (( candidate_patch != deadline_patch )); then
    (( candidate_patch > deadline_patch ))
    return
  fi

  [[ "${candidate_prerelease}" == "false" ]]
}

blocked=false
while IFS=$'\t' read -r finding_id deadline; do
  if version_reaches_deadline "${VERSION}" "${deadline}"; then
    printf 'open finding %s must be fixed before release %s\n' \
      "${finding_id}" \
      "${deadline}" >&2
    blocked=true
  fi
done < <(
  jq -r '
    .findings[]
    | select(.state == "open" and .must_fix_before != null)
    | [.id, .must_fix_before]
    | @tsv
  ' "${FINDINGS_FILE}"
)

if [[ "${blocked}" == "true" ]]; then
  exit 1
fi

printf 'No open release findings are due for %s.\n' "${VERSION}"
