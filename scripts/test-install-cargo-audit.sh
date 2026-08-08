#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
INSTALLER="${SCRIPT_DIR}/install-cargo-audit.sh"
TEST_ROOT="$(mktemp -d /tmp/schemahub-cargo-audit-installer-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

test_relative_archive_override_fails_before_install() {
  # Arrange
  local installer_exit_code=0

  # Act
  SCHEMAHUB_CARGO_AUDIT_ARCHIVE="cargo-audit.crate" \
  SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT="${TEST_ROOT}/relative-archive-install" \
    "${INSTALLER}" >/dev/null 2>&1 || installer_exit_code=$?

  # Assert
  test "${installer_exit_code}" -eq 2
  test ! -e "${TEST_ROOT}/relative-archive-install/bin/cargo-audit"
}

test_archive_checksum_mismatch_fails_before_extraction() {
  # Arrange
  local invalid_archive="${TEST_ROOT}/cargo-audit-0.22.2.crate"
  local install_root="${TEST_ROOT}/checksum-install"
  local installer_exit_code=0
  printf 'not the reviewed cargo-audit crate\n' >"${invalid_archive}"

  # Act
  SCHEMAHUB_CARGO_AUDIT_ARCHIVE="${invalid_archive}" \
  SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT="${install_root}" \
    "${INSTALLER}" >/dev/null 2>&1 || installer_exit_code=$?

  # Assert
  test "${installer_exit_code}" -ne 0
  test ! -e "${install_root}/bin/cargo-audit"
}

test_relative_archive_override_fails_before_install
test_archive_checksum_mismatch_fails_before_extraction

printf '%s\n' 'cargo-audit installer failure-contract tests passed.'
