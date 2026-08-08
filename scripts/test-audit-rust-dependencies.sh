#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
AUDIT_GATE="${SCRIPT_DIR}/audit-rust-dependencies.sh"
FAKE_AUDITOR="${SCRIPT_DIR}/../tests/integration/fake-cargo-audit.sh"
TEST_ROOT="$(mktemp -d /tmp/schemahub-rust-audit-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

run_gate() {
  local scenario="$1"

  SCHEMAHUB_FAKE_CARGO_AUDIT_SCENARIO="${scenario}" \
  SCHEMAHUB_CARGO_AUDIT_BIN="${FAKE_AUDITOR}" \
    "${AUDIT_GATE}"
}

test_exact_reviewed_warning_set_passes() {
  # Arrange
  local output="${TEST_ROOT}/accepted.txt"

  # Act
  run_gate accepted >"${output}"

  # Assert
  grep -Fq 'Rust dependency audit passed' "${output}"
}

test_vulnerability_fails_closed() {
  # Arrange
  local gate_exit_code=0

  # Act
  run_gate vulnerability >/dev/null 2>&1 || gate_exit_code=$?

  # Assert
  test "${gate_exit_code}" -eq 1
}

test_new_warning_fails_closed() {
  # Arrange
  local gate_exit_code=0

  # Act
  run_gate new-warning >/dev/null 2>&1 || gate_exit_code=$?

  # Assert
  test "${gate_exit_code}" -eq 1
}

test_disappeared_reviewed_warning_requires_policy_cleanup() {
  # Arrange
  local gate_exit_code=0

  # Act
  run_gate missing-warning >/dev/null 2>&1 || gate_exit_code=$?

  # Assert
  test "${gate_exit_code}" -eq 1
}

test_wrong_auditor_version_fails_before_scan() {
  # Arrange
  local gate_exit_code=0

  # Act
  run_gate wrong-version >/dev/null 2>&1 || gate_exit_code=$?

  # Assert
  test "${gate_exit_code}" -eq 2
}

test_malformed_report_fails_closed() {
  # Arrange
  local gate_exit_code=0

  # Act
  run_gate malformed >/dev/null 2>&1 || gate_exit_code=$?

  # Assert
  test "${gate_exit_code}" -eq 1
}

test_auditor_failure_fails_closed() {
  # Arrange
  local gate_exit_code=0

  # Act
  run_gate command-failure >/dev/null 2>&1 || gate_exit_code=$?

  # Assert
  test "${gate_exit_code}" -eq 1
}

test_exact_reviewed_warning_set_passes
test_vulnerability_fails_closed
test_new_warning_fails_closed
test_disappeared_reviewed_warning_requires_policy_cleanup
test_wrong_auditor_version_fails_before_scan
test_malformed_report_fails_closed
test_auditor_failure_fails_closed

printf '%s\n' 'Rust dependency audit gate tests passed.'
