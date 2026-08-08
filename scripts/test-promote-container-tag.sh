#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
PROMOTER="${SCRIPT_DIR}/promote-container-tag.sh"
FAKE_DOCKER="${SCRIPT_DIR}/../tests/integration/fake-docker-imagetools.sh"
RELEASE_WORKFLOW="${SCRIPT_DIR}/../.github/workflows/release.yml"
TEST_ROOT="$(mktemp -d /tmp/schemahub-container-promotion-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

CONTAINER_IMAGE="ghcr.io/shuozeli/schemahub"
VERSION="1.0.0"
EXPECTED_DIGEST="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
OTHER_DIGEST="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

run_promotion() {
  local mode="$1"
  local state="${TEST_ROOT}/${mode}.state"
  local log="${TEST_ROOT}/${mode}.log"

  FAKE_DOCKER_MODE="${mode}" \
  FAKE_DOCKER_STATE="${state}" \
  FAKE_DOCKER_LOG="${log}" \
  FAKE_DOCKER_EXPECTED_DIGEST="${EXPECTED_DIGEST}" \
  FAKE_DOCKER_OTHER_DIGEST="${OTHER_DIGEST}" \
  SCHEMAHUB_DOCKER_COMMAND="${FAKE_DOCKER}" \
    "${PROMOTER}" \
      "${CONTAINER_IMAGE}" \
      "${VERSION}" \
      "${EXPECTED_DIGEST}"
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

test_missing_tag_is_created_from_exact_digest() {
  # Arrange
  local log="${TEST_ROOT}/missing.log"
  local state="${TEST_ROOT}/missing.state"

  # Act
  run_promotion missing >"${TEST_ROOT}/missing-output.txt"

  # Assert
  test -f "${state}"
  grep -Fxq \
    "create ${CONTAINER_IMAGE}:${VERSION} ${CONTAINER_IMAGE}@${EXPECTED_DIGEST}" \
    "${log}"
  grep -Fq 'Container tag verified' "${TEST_ROOT}/missing-output.txt"
}

test_matching_tag_is_idempotent() {
  # Arrange
  local log="${TEST_ROOT}/matching.log"

  # Act
  run_promotion matching >"${TEST_ROOT}/matching-output.txt"

  # Assert
  test ! -e "${log}"
  grep -Fq 'Container tag verified' "${TEST_ROOT}/matching-output.txt"
}

test_mismatched_existing_tag_fails_without_overwrite() {
  # Arrange
  local status=0
  local log="${TEST_ROOT}/mismatch.log"

  # Act
  run_promotion mismatch >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
  test ! -e "${log}"
}

test_mismatched_candidate_digest_fails() {
  # Arrange
  local status=0

  # Act
  run_promotion bad-source >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
  test ! -e "${TEST_ROOT}/bad-source.log"
}

test_unavailable_candidate_digest_fails() {
  # Arrange
  local status=0

  # Act
  run_promotion missing-source >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
  test ! -e "${TEST_ROOT}/missing-source.log"
}

test_wrong_digest_after_creation_fails() {
  # Arrange
  local status=0
  local log="${TEST_ROOT}/wrong-after-create.log"

  # Act
  run_promotion wrong-after-create >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
  test "$(wc -l <"${log}")" -eq 1
}

test_malformed_version_fails_before_registry_access() {
  # Arrange
  local status=0

  # Act
  SCHEMAHUB_DOCKER_COMMAND="${FAKE_DOCKER}" \
    "${PROMOTER}" \
      "${CONTAINER_IMAGE}" \
      latest \
      "${EXPECTED_DIGEST}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_release_workflow_defers_version_tag_until_required_gates() {
  # Arrange
  local container_job
  local staging_job
  local assembly_job
  local promotion_job
  local publish_job
  container_job="$(extract_workflow_job container)"
  staging_job="$(extract_workflow_job staging)"
  assembly_job="$(extract_workflow_job assemble)"
  promotion_job="$(extract_workflow_job container_tag)"
  publish_job="$(extract_workflow_job publish)"

  # Act
  local premature_version_tag=0
  grep -Fq \
    '${{ needs.metadata.outputs.image }}:${{ needs.metadata.outputs.version }}' \
    <<<"${container_job}" || premature_version_tag=$?

  # Assert
  test "${premature_version_tag}" -eq 1
  grep -Fq \
    '${{ needs.metadata.outputs.image }}:candidate-${{ github.run_id }}-${{ github.run_attempt }}' \
    <<<"${container_job}"
  grep -Fq \
    'image: ${{ needs.metadata.outputs.image }}@${{ steps.build.outputs.digest }}' \
    <<<"${container_job}"
  grep -Fq 'needs: [metadata, binaries, container]' <<<"${staging_job}"
  grep -Fq \
    'needs: [metadata, binaries, container, staging]' \
    <<<"${assembly_job}"
  grep -Fq "needs.staging.result == 'success'" <<<"${assembly_job}"
  grep -Fq 'name: verified-release-assembly' <<<"${assembly_job}"
  grep -Fq \
    'needs: [metadata, container, staging, assemble]' \
    <<<"${promotion_job}"
  grep -Fq "needs.assemble.result == 'success'" <<<"${promotion_job}"
  grep -Fq 'scripts/promote-container-tag.sh' <<<"${promotion_job}"
  grep -Fq \
    'needs: [metadata, assemble, container_tag]' \
    <<<"${publish_job}"
  grep -Fq "needs.container_tag.result == 'success'" <<<"${publish_job}"
}

test_missing_tag_is_created_from_exact_digest
test_matching_tag_is_idempotent
test_mismatched_existing_tag_fails_without_overwrite
test_mismatched_candidate_digest_fails
test_unavailable_candidate_digest_fails
test_wrong_digest_after_creation_fails
test_malformed_version_fails_before_registry_access
test_release_workflow_defers_version_tag_until_required_gates

printf 'Immutable container tag promotion contract tests passed.\n'
