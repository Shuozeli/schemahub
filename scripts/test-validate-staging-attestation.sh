#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
VALIDATOR="${SCRIPT_DIR}/validate-staging-attestation.sh"
ENVIRONMENT_VALIDATOR="${SCRIPT_DIR}/validate-staging-environment.sh"
TEST_ROOT="$(mktemp -d /tmp/schemahub-staging-attestation-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

VERSION="1.0.0"
SOURCE_REVISION="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
CONTAINER_IMAGE="ghcr.io/shuozeli/schemahub"
CONTAINER_DIGEST="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
GA_READINESS_DIGEST="sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
EVIDENCE_DIGEST="sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

write_valid_attestation() {
  local output="$1"
  jq -n \
    --arg now "${NOW}" \
    --arg version "${VERSION}" \
    --arg source_revision "${SOURCE_REVISION}" \
    --arg container_image "${CONTAINER_IMAGE}" \
    --arg container_digest "${CONTAINER_DIGEST}" \
    --arg ga_readiness_digest "${GA_READINESS_DIGEST}" \
    --arg evidence_digest "${EVIDENCE_DIGEST}" '{
      schema_version: "schemahub.staging-attestation.v1",
      attested_at: $now,
      release: {
        version: $version,
        source_revision: $source_revision,
        container_image: $container_image,
        container_digest: $container_digest,
        ga_readiness_digest: $ga_readiness_digest
      },
      deployment: {
        url: "https://schemahub-staging.example.com",
        backend: "postgres",
        exact_digest: true,
        deployed_at: $now
      },
      identity: {
        issuer: "https://identity.example.com",
        development_credentials_used: false,
        current_key_accepted: true,
        next_key_accepted: true,
        removed_key_rejected: true,
        stale_keys_readyz_503: true,
        stale_keys_credentials_rejected: true,
        recovered_after_valid_jwks: true
      },
      acceptance: {
        human_agent_workflow: true,
        bundled_gui_same_origin: true,
        restart_bytes_identical: true,
        prior_candidate_bytes_identical: true,
        corrupt_artifact_failed_closed: true,
        list_dependents_live_pinned_hidden: true,
        backup_restore_drill: true
      },
      evidence: {
        url: "https://github.com/Shuozeli/schemahub/actions/runs/1",
        digest: $evidence_digest,
        run_id: "staging-acceptance-1",
        operator: "release-owner"
      }
    }' >"${output}"
}

run_validation() {
  local file="$1"
  "${VALIDATOR}" \
    "${file}" \
    "${VERSION}" \
    "${SOURCE_REVISION}" \
    "${CONTAINER_IMAGE}" \
    "${CONTAINER_DIGEST}" \
    "${GA_READINESS_DIGEST}"
}

write_valid_environment() {
  local output="$1"
  jq -n '{
    name: "schemahub-production-staging",
    protection_rules: [
      {
        type: "required_reviewers",
        prevent_self_review: true,
        reviewers: [
          {
            type: "User",
            reviewer: {
              login: "release-reviewer"
            }
          }
        ]
      },
      {
        type: "branch_policy"
      }
    ],
    deployment_branch_policy: {
      protected_branches: false,
      custom_branch_policies: true
    }
  }' >"${output}"
}

write_valid_deployment_policies() {
  local output="$1"
  jq -n '{
    total_count: 1,
    branch_policies: [
      {
        id: 1,
        node_id: "test-policy",
        name: "v*.*.*",
        type: "tag"
      }
    ]
  }' >"${output}"
}

run_environment_validation() {
  local environment="$1"
  local deployment_policies="$2"
  "${ENVIRONMENT_VALIDATOR}" "${environment}" "${deployment_policies}"
}

test_matching_attestation_passes() {
  # Arrange
  local attestation="${TEST_ROOT}/valid.json"
  write_valid_attestation "${attestation}"

  # Act
  run_validation "${attestation}" >"${TEST_ROOT}/valid-output.txt"

  # Assert
  grep -q 'Staging attestation verified' "${TEST_ROOT}/valid-output.txt"
}

test_release_coordinate_mismatch_fails() {
  # Arrange
  local attestation="${TEST_ROOT}/wrong-digest.json"
  write_valid_attestation "${attestation}"
  jq '.release.container_digest = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"' \
    "${attestation}" >"${TEST_ROOT}/wrong-digest-edited.json"

  # Act
  local status=0
  run_validation "${TEST_ROOT}/wrong-digest-edited.json" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_development_credentials_fail() {
  # Arrange
  local attestation="${TEST_ROOT}/development-credentials.json"
  write_valid_attestation "${attestation}"
  jq '.identity.development_credentials_used = true' \
    "${attestation}" >"${TEST_ROOT}/development-credentials-edited.json"

  # Act
  local status=0
  run_validation "${TEST_ROOT}/development-credentials-edited.json" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_incomplete_identity_drill_fails() {
  # Arrange
  local attestation="${TEST_ROOT}/identity-failure.json"
  write_valid_attestation "${attestation}"
  jq '.identity.stale_keys_readyz_503 = false' \
    "${attestation}" >"${TEST_ROOT}/identity-failure-edited.json"

  # Act
  local status=0
  run_validation "${TEST_ROOT}/identity-failure-edited.json" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_incomplete_product_acceptance_fails() {
  # Arrange
  local attestation="${TEST_ROOT}/acceptance-failure.json"
  write_valid_attestation "${attestation}"
  jq '.acceptance.prior_candidate_bytes_identical = false' \
    "${attestation}" >"${TEST_ROOT}/acceptance-failure-edited.json"

  # Act
  local status=0
  run_validation "${TEST_ROOT}/acceptance-failure-edited.json" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_missing_bundled_gui_acceptance_fails() {
  # Arrange
  local attestation="${TEST_ROOT}/gui-acceptance-failure.json"
  write_valid_attestation "${attestation}"
  jq '.acceptance.bundled_gui_same_origin = false' \
    "${attestation}" >"${TEST_ROOT}/gui-acceptance-failure-edited.json"

  # Act
  local status=0
  run_validation "${TEST_ROOT}/gui-acceptance-failure-edited.json" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_stale_attestation_fails() {
  # Arrange
  local attestation="${TEST_ROOT}/stale.json"
  write_valid_attestation "${attestation}"
  jq '
    .attested_at = "2020-01-01T00:00:00Z"
    | .deployment.deployed_at = "2020-01-01T00:00:00Z"
  ' "${attestation}" >"${TEST_ROOT}/stale-edited.json"

  # Act
  local status=0
  run_validation "${TEST_ROOT}/stale-edited.json" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_credential_material_fails() {
  # Arrange
  local attestation="${TEST_ROOT}/credential-material.json"
  write_valid_attestation "${attestation}"
  jq '.evidence.operator = "Bearer leaked-credential"' \
    "${attestation}" >"${TEST_ROOT}/credential-material-edited.json"

  # Act
  local status=0
  run_validation "${TEST_ROOT}/credential-material-edited.json" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_protected_environment_passes() {
  # Arrange
  local environment="${TEST_ROOT}/protected-environment.json"
  local deployment_policies="${TEST_ROOT}/protected-deployment-policies.json"
  write_valid_environment "${environment}"
  write_valid_deployment_policies "${deployment_policies}"

  # Act
  run_environment_validation "${environment}" "${deployment_policies}" \
    >"${TEST_ROOT}/protected-environment-output.txt"

  # Assert
  grep -q 'release-tag policy verified' \
    "${TEST_ROOT}/protected-environment-output.txt"
}

test_environment_without_reviewers_fails() {
  # Arrange
  local environment="${TEST_ROOT}/no-reviewers-environment.json"
  local deployment_policies="${TEST_ROOT}/no-reviewers-deployment-policies.json"
  write_valid_environment "${environment}"
  write_valid_deployment_policies "${deployment_policies}"
  jq '.protection_rules[0].reviewers = []' \
    "${environment}" >"${TEST_ROOT}/no-reviewers-environment-edited.json"

  # Act
  local status=0
  run_environment_validation \
    "${TEST_ROOT}/no-reviewers-environment-edited.json" \
    "${deployment_policies}" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_environment_allowing_self_review_fails() {
  # Arrange
  local environment="${TEST_ROOT}/self-review-environment.json"
  local deployment_policies="${TEST_ROOT}/self-review-deployment-policies.json"
  write_valid_environment "${environment}"
  write_valid_deployment_policies "${deployment_policies}"
  jq '.protection_rules[0].prevent_self_review = false' \
    "${environment}" >"${TEST_ROOT}/self-review-environment-edited.json"

  # Act
  local status=0
  run_environment_validation \
    "${TEST_ROOT}/self-review-environment-edited.json" \
    "${deployment_policies}" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_environment_without_custom_tag_policy_fails() {
  # Arrange
  local environment="${TEST_ROOT}/open-environment.json"
  local deployment_policies="${TEST_ROOT}/open-deployment-policies.json"
  write_valid_environment "${environment}"
  write_valid_deployment_policies "${deployment_policies}"
  jq '
    .deployment_branch_policy.protected_branches = true
    | .deployment_branch_policy.custom_branch_policies = false
  ' "${environment}" >"${TEST_ROOT}/open-environment-edited.json"

  # Act
  local status=0
  run_environment_validation \
    "${TEST_ROOT}/open-environment-edited.json" \
    "${deployment_policies}" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_environment_without_a_deployment_policy_fails() {
  # Arrange
  local environment="${TEST_ROOT}/missing-policy-environment.json"
  local deployment_policies="${TEST_ROOT}/missing-policy.json"
  write_valid_environment "${environment}"
  jq -n '{total_count: 0, branch_policies: []}' \
    >"${deployment_policies}"

  # Act
  local status=0
  run_environment_validation "${environment}" "${deployment_policies}" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 1
}

test_environment_with_a_broad_deployment_policy_fails() {
  # Arrange
  local environment="${TEST_ROOT}/broad-policy-environment.json"
  local deployment_policies="${TEST_ROOT}/broad-policy.json"
  write_valid_environment "${environment}"
  write_valid_deployment_policies "${deployment_policies}"
  jq '.branch_policies[0].name = "*"' \
    "${deployment_policies}" >"${TEST_ROOT}/broad-policy-edited.json"

  # Act
  local status=0
  run_environment_validation \
    "${environment}" \
    "${TEST_ROOT}/broad-policy-edited.json" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_environment_with_a_branch_policy_type_fails() {
  # Arrange
  local environment="${TEST_ROOT}/branch-policy-environment.json"
  local deployment_policies="${TEST_ROOT}/branch-policy.json"
  write_valid_environment "${environment}"
  write_valid_deployment_policies "${deployment_policies}"
  jq '.branch_policies[0].type = "branch"' \
    "${deployment_policies}" >"${TEST_ROOT}/branch-policy-edited.json"

  # Act
  local status=0
  run_environment_validation \
    "${environment}" \
    "${TEST_ROOT}/branch-policy-edited.json" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_environment_with_an_extra_deployment_policy_fails() {
  # Arrange
  local environment="${TEST_ROOT}/extra-policy-environment.json"
  local deployment_policies="${TEST_ROOT}/extra-policy.json"
  write_valid_environment "${environment}"
  write_valid_deployment_policies "${deployment_policies}"
  jq '
    .total_count = 2
    | .branch_policies += [{
        id: 2,
        node_id: "extra-policy",
        name: "main",
        type: "branch"
      }]
  ' "${deployment_policies}" >"${TEST_ROOT}/extra-policy-edited.json"

  # Act
  local status=0
  run_environment_validation \
    "${environment}" \
    "${TEST_ROOT}/extra-policy-edited.json" >/dev/null 2>&1 \
    || status=$?

  # Assert
  test "${status}" -eq 1
}

test_matching_attestation_passes
test_release_coordinate_mismatch_fails
test_development_credentials_fail
test_incomplete_identity_drill_fails
test_incomplete_product_acceptance_fails
test_missing_bundled_gui_acceptance_fails
test_stale_attestation_fails
test_credential_material_fails
test_protected_environment_passes
test_environment_without_reviewers_fails
test_environment_allowing_self_review_fails
test_environment_without_custom_tag_policy_fails
test_environment_without_a_deployment_policy_fails
test_environment_with_a_broad_deployment_policy_fails
test_environment_with_a_branch_policy_type_fails
test_environment_with_an_extra_deployment_policy_fails

printf 'Stable-release staging and protected-environment contract tests passed.\n'
