#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
VERIFIER="${SCRIPT_DIR}/verify-cargo-auditable.sh"
TEST_ROOT="$(mktemp -d /tmp/schemahub-cargo-auditable-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT
FAKE_AUDITOR="${TEST_ROOT}/cargo-audit"
printf '%s\n' '#!/usr/bin/env bash' 'exit 99' >"${FAKE_AUDITOR}"
chmod +x "${FAKE_AUDITOR}"

test_missing_verified_auditor_fails_closed() {
  # Arrange
  local verifier_exit_code=0

  # Act
  SCHEMAHUB_CARGO_AUDIT_BIN="" \
    "${VERIFIER}" >/dev/null 2>&1 || verifier_exit_code=$?

  # Assert
  test "${verifier_exit_code}" -eq 2
}

test_relative_archive_override_fails_closed() {
  # Arrange
  local verifier_exit_code=0

  # Act
  SCHEMAHUB_CARGO_AUDIT_BIN="${FAKE_AUDITOR}" \
  SCHEMAHUB_CARGO_AUDITABLE_ARCHIVE="cargo-auditable.crate" \
    "${VERIFIER}" >/dev/null 2>&1 || verifier_exit_code=$?

  # Assert
  test "${verifier_exit_code}" -eq 2
}

test_archive_checksum_mismatch_fails_before_audit() {
  # Arrange
  local invalid_archive="${TEST_ROOT}/cargo-auditable-0.7.5.crate"
  local verifier_exit_code=0
  printf 'not the reviewed cargo-auditable crate\n' >"${invalid_archive}"

  # Act
  SCHEMAHUB_CARGO_AUDIT_BIN="${FAKE_AUDITOR}" \
  SCHEMAHUB_CARGO_AUDITABLE_ARCHIVE="${invalid_archive}" \
    "${VERIFIER}" >/dev/null 2>&1 || verifier_exit_code=$?

  # Assert
  test "${verifier_exit_code}" -eq 2
}

test_missing_verified_auditor_fails_closed
test_relative_archive_override_fails_closed
test_archive_checksum_mismatch_fails_before_audit

printf '%s\n' 'cargo-auditable supply-chain failure-contract tests passed.'
