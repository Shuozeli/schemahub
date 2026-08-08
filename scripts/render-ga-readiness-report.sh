#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  render-ga-readiness-report.sh \
    EVIDENCE_ROOT OUTPUT_DIR SOURCE_REVISION RUN_ID RUN_URL

The report gate requires exactly RW-01 through RW-07 to have passing,
secret-free result summaries and complete process evidence. It fails when the
structured finding ledger contains an open release-blocker or high finding.

Set SCHEMAHUB_GA_REQUIRE_CLEAN=1 for a candidate gate. Tests may override the
ledger with SCHEMAHUB_GA_FINDINGS_FILE.
EOF
}

if [[ "$#" -ne 5 ]]; then
  usage
  exit 2
fi

EVIDENCE_ROOT="$1"
OUTPUT_DIR="$2"
SOURCE_REVISION="$3"
RUN_ID="$4"
RUN_URL="$5"

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
REPO_ROOT="$(
  cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1
  pwd
)"
SOURCE_ROOT="${SCHEMAHUB_GA_SOURCE_ROOT:-${REPO_ROOT}}"
FINDINGS_FILE="${SCHEMAHUB_GA_FINDINGS_FILE:-${REPO_ROOT}/codelabs/real-world/findings.json}"
REQUIRE_CLEAN="${SCHEMAHUB_GA_REQUIRE_CLEAN:-0}"

for command_name in find git jq sha256sum; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "${command_name}" >&2
    exit 2
  fi
done

if [[ ! -d "${EVIDENCE_ROOT}" ]]; then
  printf 'evidence root does not exist: %s\n' "${EVIDENCE_ROOT}" >&2
  exit 2
fi
if [[ ! -f "${FINDINGS_FILE}" ]]; then
  printf 'finding ledger does not exist: %s\n' "${FINDINGS_FILE}" >&2
  exit 2
fi
if [[ ! "${SOURCE_REVISION}" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; then
  printf 'source revision must be an immutable 40- or 64-character lowercase hex digest\n' >&2
  exit 2
fi
if [[ -z "${RUN_ID}" ]]; then
  printf 'run ID must not be empty\n' >&2
  exit 2
fi
if [[ "${RUN_URL}" != "local" && ! "${RUN_URL}" =~ ^https:// ]]; then
  printf 'run URL must be local or an https URL\n' >&2
  exit 2
fi
if [[ "${REQUIRE_CLEAN}" != "0" && "${REQUIRE_CLEAN}" != "1" ]]; then
  printf 'SCHEMAHUB_GA_REQUIRE_CLEAN must be 0 or 1\n' >&2
  exit 2
fi
if [[ "$(git -C "${SOURCE_ROOT}" rev-parse --is-inside-work-tree 2>/dev/null)" != "true" ]]; then
  printf 'GA source root is not a Git worktree: %s\n' "${SOURCE_ROOT}" >&2
  exit 2
fi
ACTUAL_SOURCE_REVISION="$(git -C "${SOURCE_ROOT}" rev-parse HEAD)"
if [[ "${SOURCE_REVISION}" != "${ACTUAL_SOURCE_REVISION}" ]]; then
  printf 'source revision does not match the checked-out GA source root\n' >&2
  exit 2
fi

jq -e '
  .schema_version == "schemahub.ga-findings.v1"
  and (.findings | type == "array")
  and (.findings | all(
    (.id | type == "string" and length > 0)
    and (.scenario | type == "string" and length > 0)
    and (.class | type == "string" and length > 0)
    and (.severity | IN("release-blocker", "high", "medium", "low"))
    and (.state | IN("open", "fixed"))
    and (
      .must_fix_before == null
      or (
        .must_fix_before
        | type == "string"
          and test("^[0-9]+[.][0-9]+[.][0-9]+$")
      )
    )
    and (.summary | type == "string" and length > 0)
    and (.resolution | type == "string" and length > 0)
  ))
  and (([.findings[].id] | length) == ([.findings[].id] | unique | length))
' "${FINDINGS_FILE}" >/dev/null || {
  printf 'finding ledger is malformed or contains duplicate IDs\n' >&2
  exit 2
}

SCENARIO_DIRS=(
  rw-01-human-agent
  rw-02-commerce
  rw-03-mobile-telemetry
  rw-04-concurrent-editors
  rw-05-data-pipeline
  rw-06-dependency-closure
  rw-07-tenant-isolation
)
SCENARIO_IDS=(
  RW-01
  RW-02
  RW-03
  RW-04
  RW-05
  RW-06
  RW-07
)

result_count="$(
  find "${EVIDENCE_ROOT}" \
    -mindepth 2 \
    -maxdepth 2 \
    -type f \
    -name result.json \
    -print \
    | wc -l \
    | tr -d ' '
)"
if [[ "${result_count}" != "${#SCENARIO_IDS[@]}" ]]; then
  printf 'expected exactly %s result summaries, found %s\n' \
    "${#SCENARIO_IDS[@]}" \
    "${result_count}" >&2
  exit 1
fi

mkdir -p "${OUTPUT_DIR}/scenario-results"
TEMP_DIR="$(mktemp -d /tmp/schemahub-ga-report.XXXXXX)"
trap 'rm -rf -- "${TEMP_DIR}"' EXIT
SCENARIOS_NDJSON="${TEMP_DIR}/scenarios.ndjson"
: >"${SCENARIOS_NDJSON}"

for index in "${!SCENARIO_IDS[@]}"; do
  scenario_dir="${EVIDENCE_ROOT}/${SCENARIO_DIRS[$index]}"
  scenario_id="${SCENARIO_IDS[$index]}"
  result_file="${scenario_dir}/result.json"

  for required_file in \
    "${result_file}" \
    "${scenario_dir}/capabilities.json" \
    "${scenario_dir}/server.log" \
    "${scenario_dir}/transcript.log"
  do
    if [[ ! -s "${required_file}" ]]; then
      printf '%s is missing required non-empty evidence: %s\n' \
        "${scenario_id}" \
        "${required_file}" >&2
      exit 1
    fi
  done

  jq -e . "${scenario_dir}/capabilities.json" >/dev/null || {
    printf '%s capabilities evidence is not valid JSON\n' "${scenario_id}" >&2
    exit 1
  }

  normalized_result="${OUTPUT_DIR}/scenario-results/${scenario_id}.json"
  jq -S -e --arg scenario_id "${scenario_id}" '
    select(type == "object")
    | select(.scenario == $scenario_id)
    | select(.status == "passed")
    | select(
        ([paths
          | select(.[-1] | type == "string")
          | .[-1]
          | ascii_downcase]
        | all(test("(^|_)(bearer|password|secret|token)($|_)") | not))
      )
    | select(
        ([.. | strings]
        | all(
            test(
              "codelab-(human|agent|producer|consumer)-token|bearer[[:space:]]+[A-Za-z0-9._~-]+";
              "i"
            )
            | not
          ))
      )
  ' "${result_file}" >"${normalized_result}" || {
    printf '%s result is malformed, non-passing, mismatched, or contains credential material\n' \
      "${scenario_id}" >&2
    exit 1
  }

  result_digest="sha256:$(sha256sum "${normalized_result}" | cut -d ' ' -f 1)"
  jq -n -c \
    --arg id "${scenario_id}" \
    --arg status "passed" \
    --arg result_digest "${result_digest}" \
    '{
      id: $id,
      status: $status,
      normalized_result_digest: $result_digest
    }' >>"${SCENARIOS_NDJSON}"
done

SCENARIOS_JSON="$(jq -s . "${SCENARIOS_NDJSON}")"
OPEN_FINDINGS_JSON="$(jq '[.findings[] | select(.state == "open")]' "${FINDINGS_FILE}")"
OPEN_RELEASE_BLOCKERS="$(jq '[.[] | select(.severity == "release-blocker")] | length' <<<"${OPEN_FINDINGS_JSON}")"
OPEN_HIGH="$(jq '[.[] | select(.severity == "high")] | length' <<<"${OPEN_FINDINGS_JSON}")"
OPEN_MEDIUM="$(jq '[.[] | select(.severity == "medium")] | length' <<<"${OPEN_FINDINGS_JSON}")"
OPEN_LOW="$(jq '[.[] | select(.severity == "low")] | length' <<<"${OPEN_FINDINGS_JSON}")"

SOURCE_DIRTY=false
if [[ -n "$(git -C "${SOURCE_ROOT}" status --porcelain --untracked-files=normal)" ]]; then
  SOURCE_DIRTY=true
fi

GATE_STATUS=passed
if (( OPEN_RELEASE_BLOCKERS > 0 || OPEN_HIGH > 0 )); then
  GATE_STATUS=failed
fi

PROVENANCE_STATUS=clean
if [[ "${SOURCE_DIRTY}" == "true" ]]; then
  PROVENANCE_STATUS=dirty
fi

REPORT_JSON="${OUTPUT_DIR}/ga-readiness.json"
jq -n -S \
  --arg schema_version "schemahub.ga-readiness.v1" \
  --arg source_revision "${SOURCE_REVISION}" \
  --arg run_id "${RUN_ID}" \
  --arg run_url "${RUN_URL}" \
  --arg gate_status "${GATE_STATUS}" \
  --arg provenance_status "${PROVENANCE_STATUS}" \
  --argjson source_dirty "${SOURCE_DIRTY}" \
  --argjson required_scenarios "${#SCENARIO_IDS[@]}" \
  --argjson scenarios "${SCENARIOS_JSON}" \
  --argjson open_findings "${OPEN_FINDINGS_JSON}" \
  --argjson open_release_blockers "${OPEN_RELEASE_BLOCKERS}" \
  --argjson open_high "${OPEN_HIGH}" \
  --argjson open_medium "${OPEN_MEDIUM}" \
  --argjson open_low "${OPEN_LOW}" \
  '{
    schema_version: $schema_version,
    source: {
      revision: $source_revision,
      worktree_dirty: $source_dirty,
      provenance_status: $provenance_status
    },
    run: {
      id: $run_id,
      url: $run_url
    },
    gate: {
      status: $gate_status,
      required_scenarios: $required_scenarios,
      passed_scenarios: ($scenarios | length),
      open_findings: {
        release_blocker: $open_release_blockers,
        high: $open_high,
        medium: $open_medium,
        low: $open_low
      },
      release_authorized: false
    },
    scenarios: $scenarios,
    open_findings: $open_findings,
    remaining_external_gates: [
      "exact-digest staging deployment",
      "real-provider JWT rotation and staleness drill",
      "explicit tag and publication authorization"
    ]
  }' >"${REPORT_JSON}"

REPORT_MARKDOWN="${OUTPUT_DIR}/GA-READINESS.md"
{
  printf '# SchemaHub GA Readiness Evidence\n\n'
  printf -- "- Source revision: \`%s\`\n" "${SOURCE_REVISION}"
  printf -- '- Source provenance: **%s**\n' "${PROVENANCE_STATUS}"
  if [[ "${RUN_URL}" == "local" ]]; then
    printf -- "- Evidence run: \`%s\` (local)\n" "${RUN_ID}"
  else
    printf -- '- Evidence run: [%s](%s)\n' "${RUN_ID}" "${RUN_URL}"
  fi
  printf -- '- Scenario gate: **%s**\n' "${GATE_STATUS}"
  printf -- '- Release authorization: **not granted by this report**\n\n'
  printf '## Scenario Results\n\n'
  printf '| Scenario | Status | Normalized result digest |\n'
  printf '|---|---|---|\n'
  jq -r '.scenarios[] | "| \(.id) | \(.status) | `\(.normalized_result_digest)` |"' \
    "${REPORT_JSON}"
  printf '\n## Open Findings\n\n'
  if [[ "$(jq '.open_findings | length' "${REPORT_JSON}")" == "0" ]]; then
    printf 'No open findings.\n'
  else
    printf '| Finding | Scenario | Severity | Summary |\n'
    printf '|---|---|---|---|\n'
    jq -r '
      .open_findings[]
      | "| \(.id) | \(.scenario) | \(.severity) | \(.summary | gsub("\\|"; "\\\\|")) |"
    ' "${REPORT_JSON}"
  fi
  printf '\n## Release Boundary\n\n'
  printf 'This report proves only the repository scenario gate. It does not replace '
  printf 'the exact-digest staging deployment, real-provider JWT rotation/staleness '
  printf 'drill, or explicit authorization to push a release tag and publish assets.\n'
} >"${REPORT_MARKDOWN}"

REPORT_DIGEST="sha256:$(sha256sum "${REPORT_JSON}" | cut -d ' ' -f 1)"
output_file_count="$(
  find "${OUTPUT_DIR}" -type f -print | wc -l | tr -d ' '
)"
if [[ "${output_file_count}" != "9" ]]; then
  printf 'GA report output must contain exactly 9 files, found %s\n' \
    "${output_file_count}" >&2
  exit 1
fi
printf 'GA readiness report: %s\n' "${REPORT_MARKDOWN}"
printf 'Machine report digest: %s\n' "${REPORT_DIGEST}"

if [[ "${GATE_STATUS}" != "passed" ]]; then
  printf 'GA scenario gate failed: open release-blocker or high finding\n' >&2
  exit 1
fi
if [[ "${REQUIRE_CLEAN}" == "1" && "${SOURCE_DIRTY}" == "true" ]]; then
  printf 'GA candidate gate requires a clean source worktree\n' >&2
  exit 1
fi
