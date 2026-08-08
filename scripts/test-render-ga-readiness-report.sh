#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
REPORTER="${SCRIPT_DIR}/render-ga-readiness-report.sh"
VERIFIER="${SCRIPT_DIR}/verify-ga-readiness-report.sh"
FINDINGS="${SCRIPT_DIR}/../codelabs/real-world/findings.json"

TEST_ROOT="$(mktemp -d /tmp/schemahub-ga-report-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT
SOURCE_ROOT="${TEST_ROOT}/source"
mkdir -p "${SOURCE_ROOT}"
git -C "${SOURCE_ROOT}" init --quiet
printf 'candidate source\n' >"${SOURCE_ROOT}/source.txt"
git -C "${SOURCE_ROOT}" add source.txt
git -C "${SOURCE_ROOT}" \
  -c user.name=SchemaHub \
  -c user.email=schemahub@example.invalid \
  commit --quiet -m "Create test source"
SOURCE_REVISION="$(git -C "${SOURCE_ROOT}" rev-parse HEAD)"
export SCHEMAHUB_GA_SOURCE_ROOT="${SOURCE_ROOT}"

SCENARIO_DIRS=(
  rw-01-human-agent
  rw-02-commerce
  rw-03-mobile-telemetry
  rw-04-concurrent-editors
  rw-05-data-pipeline
  rw-06-dependency-closure
  rw-07-tenant-isolation
)

make_valid_evidence() {
  local evidence_root="$1"
  local index
  for index in "${!SCENARIO_DIRS[@]}"; do
    local scenario_number
    scenario_number="$(printf '%02d' "$((index + 1))")"
    local scenario_dir="${evidence_root}/${SCENARIO_DIRS[$index]}"
    mkdir -p "${scenario_dir}"
    printf '{"matrix_version":"schemahub.capabilities.v1"}\n' \
      >"${scenario_dir}/capabilities.json"
    printf '{"event":"server_started"}\n' >"${scenario_dir}/server.log"
    printf '[RW-%s] Arrange: deterministic fixture\n' "${scenario_number}" \
      >"${scenario_dir}/transcript.log"
    printf '{"scenario":"RW-%s","status":"passed"}\n' "${scenario_number}" \
      >"${scenario_dir}/result.json"
  done
}

test_passing_portfolio_renders_normalized_report() {
  # Arrange
  local evidence_root="${TEST_ROOT}/passing-evidence"
  local output_dir="${TEST_ROOT}/passing-report"
  make_valid_evidence "${evidence_root}"

  # Act
  SCHEMAHUB_GA_REQUIRE_CLEAN=1 \
    "${REPORTER}" \
      "${evidence_root}" \
      "${output_dir}" \
      "${SOURCE_REVISION}" \
      test-pass \
      https://github.com/Shuozeli/schemahub/actions/runs/1 >/dev/null

  # Assert
  jq -e '
    .schema_version == "schemahub.ga-readiness.v1"
    and .gate.status == "passed"
    and .source.provenance_status == "clean"
    and .gate.required_scenarios == 7
    and .gate.passed_scenarios == 7
    and .gate.open_findings.release_blocker == 0
    and .gate.open_findings.high == 0
    and .gate.open_findings.low == 0
    and (.scenarios | length == 7)
    and ([.scenarios[].normalized_result_digest]
      | all(startswith("sha256:")))
  ' "${output_dir}/ga-readiness.json" >/dev/null
  test -s "${output_dir}/GA-READINESS.md"

  local archive="${TEST_ROOT}/passing-report.tar.gz"
  tar \
    --sort=name \
    --mtime='UTC 1970-01-01' \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -czf "${archive}" \
    -C "${output_dir}" \
    .
  "${VERIFIER}" "${archive}" "${SOURCE_REVISION}" >/dev/null
}

test_open_high_finding_fails_closed() {
  # Arrange
  local evidence_root="${TEST_ROOT}/high-evidence"
  local output_dir="${TEST_ROOT}/high-report"
  local findings_file="${TEST_ROOT}/high-findings.json"
  make_valid_evidence "${evidence_root}"
  jq '
    .findings += [{
      id: "RW-TEST-001",
      scenario: "RW-01",
      class: "test",
      severity: "high",
      state: "open",
      summary: "Injected high finding.",
      resolution: "Fix before candidate publication."
    }]
  ' "${FINDINGS}" >"${findings_file}"

  # Act
  local status=0
  SCHEMAHUB_GA_FINDINGS_FILE="${findings_file}" \
    "${REPORTER}" \
      "${evidence_root}" \
      "${output_dir}" \
      "${SOURCE_REVISION}" \
      test-high \
      local >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
  jq -e '
    .gate.status == "failed"
    and .gate.open_findings.high == 1
  ' "${output_dir}/ga-readiness.json" >/dev/null
}

test_missing_scenario_result_fails_closed() {
  # Arrange
  local evidence_root="${TEST_ROOT}/missing-evidence"
  local output_dir="${TEST_ROOT}/missing-report"
  make_valid_evidence "${evidence_root}"
  rm -- "${evidence_root}/rw-07-tenant-isolation/result.json"

  # Act
  local status=0
  "${REPORTER}" \
    "${evidence_root}" \
    "${output_dir}" \
    "${SOURCE_REVISION}" \
    test-missing \
    local >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_credential_material_in_result_fails_closed() {
  # Arrange
  local evidence_root="${TEST_ROOT}/secret-evidence"
  local output_dir="${TEST_ROOT}/secret-report"
  make_valid_evidence "${evidence_root}"
  jq '.agent_token = "codelab-agent-token"' \
    "${evidence_root}/rw-01-human-agent/result.json" \
    >"${TEST_ROOT}/unsafe-result.json"
  mv "${TEST_ROOT}/unsafe-result.json" \
    "${evidence_root}/rw-01-human-agent/result.json"

  # Act
  local status=0
  "${REPORTER}" \
    "${evidence_root}" \
    "${output_dir}" \
    "${SOURCE_REVISION}" \
    test-secret \
    local >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_dirty_candidate_source_fails_closed() {
  # Arrange
  local evidence_root="${TEST_ROOT}/dirty-evidence"
  local output_dir="${TEST_ROOT}/dirty-report"
  make_valid_evidence "${evidence_root}"
  printf 'uncommitted source\n' >"${SOURCE_ROOT}/untracked.txt"

  # Act
  local status=0
  SCHEMAHUB_GA_REQUIRE_CLEAN=1 \
    "${REPORTER}" \
      "${evidence_root}" \
      "${output_dir}" \
      "${SOURCE_REVISION}" \
      test-dirty \
      local >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
  jq -e '
    .gate.status == "passed"
    and .source.worktree_dirty == true
    and .source.provenance_status == "dirty"
  ' "${output_dir}/ga-readiness.json" >/dev/null
}

test_tampered_normalized_result_fails_verification() {
  # Arrange
  local evidence_root="${TEST_ROOT}/tamper-evidence"
  local output_dir="${TEST_ROOT}/tamper-report"
  local archive="${TEST_ROOT}/tamper-report.tar.gz"
  rm -- "${SOURCE_ROOT}/untracked.txt"
  make_valid_evidence "${evidence_root}"
  SCHEMAHUB_GA_REQUIRE_CLEAN=1 \
    "${REPORTER}" \
      "${evidence_root}" \
      "${output_dir}" \
      "${SOURCE_REVISION}" \
      test-tamper \
      https://github.com/Shuozeli/schemahub/actions/runs/1 >/dev/null
  printf '\n' >>"${output_dir}/scenario-results/RW-01.json"
  tar \
    --sort=name \
    --mtime='UTC 1970-01-01' \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -czf "${archive}" \
    -C "${output_dir}" \
    .

  # Act
  local status=0
  "${VERIFIER}" "${archive}" "${SOURCE_REVISION}" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_passing_portfolio_renders_normalized_report
test_open_high_finding_fails_closed
test_missing_scenario_result_fails_closed
test_credential_material_in_result_fails_closed
test_dirty_candidate_source_fails_closed
test_tampered_normalized_result_fails_verification

printf 'GA readiness report contract tests passed.\n'
