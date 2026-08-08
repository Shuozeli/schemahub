#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
VERIFIER="${SCRIPT_DIR}/verify-release-assembly.sh"
RELEASE_WORKFLOW="${SCRIPT_DIR}/../.github/workflows/release.yml"
TEST_ROOT="$(mktemp -d /tmp/schemahub-release-assembly-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

make_valid_assembly() {
  local output="$1"
  mkdir -p "${output}"
  printf 'release notes\n' >"${output}/RELEASE-NOTES.md"
  printf 'archive bytes\n' >"${output}/schemahub-1.0.0-linux.tar.gz"
  printf '{"spdxVersion":"SPDX-2.3"}\n' \
    >"${output}/schemahub-distribution.spdx.json"
  (
    cd -- "${output}"
    sha256sum \
      RELEASE-NOTES.md \
      schemahub-1.0.0-linux.tar.gz \
      schemahub-distribution.spdx.json \
      >SHA256SUMS
  )
}

copy_valid_assembly() {
  local name="$1"
  local output="${TEST_ROOT}/${name}"
  cp -R "${TEST_ROOT}/valid" "${output}"
  printf '%s\n' "${output}"
}

extract_workflow_job() {
  local job_name="$1"
  awk -v header="  ${job_name}:" '
    $0 == header {
      found = 1
    }
    found && $0 != header && $0 ~ /^  [A-Za-z0-9_]+:$/ {
      exit
    }
    found {
      print
    }
    END {
      if (!found) {
        exit 1
      }
    }
  ' "${RELEASE_WORKFLOW}"
}

test_exact_checksums_and_file_set_pass() {
  # Arrange
  local assembly="${TEST_ROOT}/valid"
  make_valid_assembly "${assembly}"

  # Act
  "${VERIFIER}" "${assembly}" >"${TEST_ROOT}/valid-output.txt"

  # Assert
  grep -Fq 'Release assembly verified: 3 checksummed files.' \
    "${TEST_ROOT}/valid-output.txt"
}

test_tampered_bytes_fail() {
  # Arrange
  local assembly
  assembly="$(copy_valid_assembly tampered)"
  printf 'tampered\n' >>"${assembly}/RELEASE-NOTES.md"

  # Act
  local status=0
  "${VERIFIER}" "${assembly}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_unchecksummed_extra_file_fails() {
  # Arrange
  local assembly
  assembly="$(copy_valid_assembly extra)"
  printf 'not in the manifest\n' >"${assembly}/unexpected.txt"

  # Act
  local status=0
  "${VERIFIER}" "${assembly}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_missing_file_fails() {
  # Arrange
  local assembly
  assembly="$(copy_valid_assembly missing)"
  rm -- "${assembly}/schemahub-1.0.0-linux.tar.gz"

  # Act
  local status=0
  "${VERIFIER}" "${assembly}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_duplicate_checksum_filename_fails() {
  # Arrange
  local assembly
  assembly="$(copy_valid_assembly duplicate)"
  head -n 1 "${assembly}/SHA256SUMS" >>"${assembly}/SHA256SUMS"

  # Act
  local status=0
  "${VERIFIER}" "${assembly}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_unsafe_checksum_path_fails() {
  # Arrange
  local assembly
  assembly="$(copy_valid_assembly unsafe-path)"
  digest="$(
    sha256sum "${assembly}/RELEASE-NOTES.md" \
      | awk '{ print $1 }'
  )"
  printf '%s  ../RELEASE-NOTES.md\n' "${digest}" \
    >"${assembly}/SHA256SUMS"

  # Act
  local status=0
  "${VERIFIER}" "${assembly}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_nested_entry_fails() {
  # Arrange
  local assembly
  assembly="$(copy_valid_assembly nested)"
  mkdir "${assembly}/nested"

  # Act
  local status=0
  "${VERIFIER}" "${assembly}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_release_workflow_verifies_before_and_after_transfer() {
  # Arrange
  local assembly_job
  local publish_job
  assembly_job="$(extract_workflow_job assemble)"
  publish_job="$(extract_workflow_job publish)"

  # Act
  local assembly_verifier_count
  local publish_verifier_count
  assembly_verifier_count="$(
    grep -Fc 'source/scripts/verify-release-assembly.sh dist' \
      <<<"${assembly_job}"
  )"
  publish_verifier_count="$(
    grep -Fc 'source/scripts/verify-release-assembly.sh dist' \
      <<<"${publish_job}"
  )"

  # Assert
  test "${assembly_verifier_count}" -eq 1
  test "${publish_verifier_count}" -eq 1
  grep -Fq \
    'artifact-digest: ${{ steps.release-assembly.outputs.artifact-digest }}' \
    <<<"${assembly_job}"
  grep -Fq \
    'ASSEMBLY_ARTIFACT_DIGEST: ${{ needs.assemble.outputs.artifact-digest }}' \
    <<<"${publish_job}"
  grep -Fq \
    '[[ "$ASSEMBLY_ARTIFACT_DIGEST" =~ ^[0-9a-f]{64}$ ]]' \
    <<<"${publish_job}"
}

test_exact_checksums_and_file_set_pass
test_tampered_bytes_fail
test_unchecksummed_extra_file_fails
test_missing_file_fails
test_duplicate_checksum_filename_fails
test_unsafe_checksum_path_fails
test_nested_entry_fails
test_release_workflow_verifies_before_and_after_transfer

printf 'Release assembly verification contract tests passed.\n'
