#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
VALIDATOR="${SCRIPT_DIR}/validate-release-findings.sh"
FINDINGS="${SCRIPT_DIR}/../codelabs/real-world/findings.json"
TEST_ROOT="$(mktemp -d /tmp/schemahub-release-findings-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

make_open_deadline_findings() {
  local output="$1"
  jq '
    (.findings[] | select(.id == "RW-03-001")).state = "open"
  ' "${FINDINGS}" >"${output}"
}

test_candidate_before_deadline_passes() {
  # Arrange
  local output="${TEST_ROOT}/candidate-output.txt"
  local open_findings="${TEST_ROOT}/candidate-open-findings.json"
  make_open_deadline_findings "${open_findings}"

  # Act
  "${VALIDATOR}" 0.9.0-rc.1 "${open_findings}" >"${output}"

  # Assert
  grep -q 'No open release findings are due' "${output}"
}

test_prerelease_at_deadline_passes() {
  # Arrange
  local output="${TEST_ROOT}/prerelease-output.txt"
  local open_findings="${TEST_ROOT}/prerelease-open-findings.json"
  make_open_deadline_findings "${open_findings}"

  # Act
  "${VALIDATOR}" 1.0.0-rc.1 "${open_findings}" >"${output}"

  # Assert
  grep -q 'No open release findings are due' "${output}"
}

test_stable_release_at_deadline_fails() {
  # Arrange
  local open_findings="${TEST_ROOT}/stable-open-findings.json"
  make_open_deadline_findings "${open_findings}"
  local status=0

  # Act
  "${VALIDATOR}" 1.0.0 "${open_findings}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_later_release_with_open_finding_fails() {
  # Arrange
  local open_findings="${TEST_ROOT}/later-open-findings.json"
  make_open_deadline_findings "${open_findings}"
  local status=0

  # Act
  "${VALIDATOR}" 1.1.0-rc.1 "${open_findings}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_fixed_finding_allows_stable_release() {
  # Arrange
  local output="${TEST_ROOT}/fixed-output.txt"

  # Act
  "${VALIDATOR}" 1.0.0 "${FINDINGS}" >"${output}"

  # Assert
  grep -q 'No open release findings are due' "${output}"
}

test_malformed_deadline_fails_closed() {
  # Arrange
  local malformed_findings="${TEST_ROOT}/malformed-findings.json"
  jq '
    (.findings[] | select(.id == "RW-03-001")).must_fix_before = "v1"
  ' "${FINDINGS}" >"${malformed_findings}"
  local status=0

  # Act
  "${VALIDATOR}" 0.9.0-rc.1 "${malformed_findings}" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_candidate_before_deadline_passes
test_prerelease_at_deadline_passes
test_stable_release_at_deadline_fails
test_later_release_with_open_finding_fails
test_fixed_finding_allows_stable_release
test_malformed_deadline_fails_closed

printf 'Release finding deadline contract tests passed.\n'
