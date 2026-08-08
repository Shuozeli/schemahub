#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
REPO_ROOT="$(
  cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1
  pwd
)"
TEST_ROOT="$(mktemp -d /tmp/schemahub-release-notes-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

SCHEMAHUB_REVISION="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
PROTOBUF_REVISION="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
FLATBUFFERS_REVISION="cccccccccccccccccccccccccccccccccccccccc"
CONTAINER_IMAGE="ghcr.io/shuozeli/schemahub"
CONTAINER_DIGEST="sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"

prepare_fixture() {
  local name="$1"
  local fixture="${TEST_ROOT}/${name}"
  mkdir -p "${fixture}/scripts" "${fixture}/docs/releases"
  cp "${REPO_ROOT}/scripts/render-release-notes.sh" "${fixture}/scripts/"
  cp "${REPO_ROOT}"/docs/releases/*.md "${fixture}/docs/releases/"
  printf '%s\n' "${fixture}"
}

render_notes() {
  local fixture="$1"
  local version="$2"
  local output="$3"
  "${fixture}/scripts/render-release-notes.sh" \
    "${version}" \
    "${SCHEMAHUB_REVISION}" \
    "${PROTOBUF_REVISION}" \
    "${FLATBUFFERS_REVISION}" \
    "${CONTAINER_IMAGE}" \
    "${CONTAINER_DIGEST}" \
    "${output}"
}

test_stable_release_renders_exact_contract() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture stable)"
  local output="${fixture}/output/1.0.0.md"

  # Act
  render_notes "${fixture}" 1.0.0 "${output}" >/dev/null

  # Assert
  grep -Fxq '# SchemaHub 1.0.0' "${output}"
  grep -Fq "SchemaHub revision: \`${SCHEMAHUB_REVISION}\`" "${output}"
  grep -Fq "${CONTAINER_IMAGE}@${CONTAINER_DIGEST}" "${output}"
  grep -Fq 'schemahub-staging-attestation.json' "${output}"
  if grep -Eq '\{\{[A-Z0-9_]+\}\}' "${output}"; then
    printf 'rendered stable notes retained a template token\n' >&2
    return 1
  fi
}

test_prerelease_does_not_require_staging_section() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture prerelease)"
  local output="${fixture}/output/0.9.0-rc.1.md"

  # Act
  render_notes "${fixture}" 0.9.0-rc.1 "${output}" >/dev/null

  # Assert
  grep -Fxq '# SchemaHub 0.9.0-rc.1' "${output}"
  if grep -Fxq '## Staging acceptance' "${output}"; then
    printf 'prerelease fixture unexpectedly contains the stable section\n' >&2
    return 1
  fi
}

test_stable_release_without_staging_section_fails() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture missing-staging)"
  local template="${fixture}/docs/releases/1.0.0.md"
  awk '$0 != "## Staging acceptance"' "${template}" \
    >"${fixture}/docs/releases/1.0.0-edited.md"
  mv "${fixture}/docs/releases/1.0.0-edited.md" "${template}"
  local status=0

  # Act
  render_notes "${fixture}" 1.0.0 "${fixture}/output.md" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_stable_release_without_deferred_container_tag_fails() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture missing-deferred-container-tag)"
  local template="${fixture}/docs/releases/1.0.0.md"
  sed 's/versioned container tag/release image reference/' \
    "${template}" >"${fixture}/docs/releases/1.0.0-edited.md"
  mv "${fixture}/docs/releases/1.0.0-edited.md" "${template}"
  local status=0

  # Act
  render_notes "${fixture}" 1.0.0 "${fixture}/output.md" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_1_0_release_without_frozen_boundary_fails() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture missing-boundary)"
  local template="${fixture}/docs/releases/1.0.0.md"
  sed 's/global multi-repository transaction/global coordination operation/' \
    "${template}" >"${fixture}/docs/releases/1.0.0-edited.md"
  mv "${fixture}/docs/releases/1.0.0-edited.md" "${template}"
  local status=0

  # Act
  render_notes "${fixture}" 1.0.0 "${fixture}/output.md" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_release_without_bundled_gui_contract_fails() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture missing-bundled-gui)"
  local template="${fixture}/docs/releases/0.9.0-rc.1.md"
  sed 's/bundled GUI/version-matched console/g' \
    "${template}" >"${fixture}/docs/releases/0.9.0-rc.1-edited.md"
  mv "${fixture}/docs/releases/0.9.0-rc.1-edited.md" "${template}"
  local status=0

  # Act
  render_notes "${fixture}" 0.9.0-rc.1 "${fixture}/output.md" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_unresolved_marker_fails() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture unresolved-marker)"
  printf '\nTODO: resolve before publication\n' \
    >>"${fixture}/docs/releases/1.0.0.md"
  local status=0

  # Act
  render_notes "${fixture}" 1.0.0 "${fixture}/output.md" \
    >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_stable_release_renders_exact_contract
test_prerelease_does_not_require_staging_section
test_stable_release_without_staging_section_fails
test_stable_release_without_deferred_container_tag_fails
test_1_0_release_without_frozen_boundary_fails
test_release_without_bundled_gui_contract_fails
test_unresolved_marker_fails

printf 'Versioned release-note contract tests passed.\n'
