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
TEST_ROOT="$(mktemp -d /tmp/schemahub-package-release-tests.XXXXXX)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

VERSION="1.2.3-rc.1"
TARGET="x86_64-unknown-linux-gnu"
SCHEMAHUB_REVISION="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
PROTOBUF_RS_REVISION="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
FLATBUFFERS_RS_REVISION="cccccccccccccccccccccccccccccccccccccccc"

prepare_fixture() {
  local name="$1"
  local fixture="${TEST_ROOT}/${name}"
  mkdir -p \
    "${fixture}/scripts" \
    "${fixture}/docs" \
    "${fixture}/apps/schemahub-gui/dist/assets" \
    "${fixture}/target/${TARGET}/release"
  cp "${REPO_ROOT}/scripts/package-release.sh" "${fixture}/scripts/"
  printf '# SchemaHub\n' >"${fixture}/README.md"
  printf '# Compatibility\n' >"${fixture}/docs/compatibility-policy.md"
  printf '<!doctype html><script type="module" src="/assets/app.js"></script>\n' \
    >"${fixture}/apps/schemahub-gui/dist/index.html"
  printf 'console.log("schemahub");\n' \
    >"${fixture}/apps/schemahub-gui/dist/assets/app.js"
  printf '<svg></svg>\n' >"${fixture}/apps/schemahub-gui/dist/favicon.svg"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    "case \"\${1:-}\" in" \
    '  --version) printf "schemahub-server 1.2.3-rc.1\n" ;;' \
    '  --print-openapi) printf "{\"openapi\":\"3.1.0\"}\n" ;;' \
    '  *) exit 2 ;;' \
    'esac' \
    >"${fixture}/target/${TARGET}/release/schemahub-server"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    "test \"\${1:-}\" = \"--version\"" \
    'printf "schemahub 1.2.3-rc.1\n"' \
    >"${fixture}/target/${TARGET}/release/schemahub"
  chmod +x \
    "${fixture}/scripts/package-release.sh" \
    "${fixture}/target/${TARGET}/release/schemahub-server" \
    "${fixture}/target/${TARGET}/release/schemahub"
  printf '%s\n' "${fixture}"
}

package_fixture() {
  local fixture="$1"
  local output_dir="${2:-${fixture}/output}"
  SCHEMAHUB_REVISION="${SCHEMAHUB_REVISION}" \
    PROTOBUF_RS_REVISION="${PROTOBUF_RS_REVISION}" \
    FLATBUFFERS_RS_REVISION="${FLATBUFFERS_RS_REVISION}" \
    "${fixture}/scripts/package-release.sh" \
      "${VERSION}" \
      "${TARGET}" \
      "${output_dir}"
}

test_release_archive_contains_the_exact_gui_bundle() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture bundled-gui)"
  local archive="${fixture}/output/schemahub-${VERSION}-${TARGET}.tar.gz"
  local extracted="${fixture}/extracted"
  local archive_entries
  local archive_details
  local sorted_entries

  # Act
  package_fixture "${fixture}" >/dev/null
  archive_entries="$(tar -tzf "${archive}")"
  archive_details="$(tar --numeric-owner -tvzf "${archive}")"
  sorted_entries="$(LC_ALL=C sort <<<"${archive_entries}")"

  # Assert
  test "${archive_entries}" = "${sorted_entries}"
  grep -Fxq \
    "schemahub-${VERSION}-${TARGET}/schemahub-gui/index.html" \
    <<<"${archive_entries}"
  grep -Fxq \
    "schemahub-${VERSION}-${TARGET}/schemahub-gui/assets/app.js" \
    <<<"${archive_entries}"
  mkdir -p "${extracted}"
  tar -xzf "${archive}" -C "${extracted}"
  grep -Fxq 'gui=schemahub-gui/index.html' \
    "${extracted}/schemahub-${VERSION}-${TARGET}/BUILD-METADATA.txt"
  cmp \
    "${fixture}/apps/schemahub-gui/dist/assets/app.js" \
    "${extracted}/schemahub-${VERSION}-${TARGET}/schemahub-gui/assets/app.js"
  grep -Eq \
    "^-rw-r--r-- 0/0 +[0-9]+ 2000-01-01 00:00 schemahub-${VERSION}-${TARGET}/BUILD-METADATA.txt$" \
    <<<"${archive_details}"
  grep -Eq \
    "^-rwxr-xr-x 0/0 +[0-9]+ 2000-01-01 00:00 schemahub-${VERSION}-${TARGET}/schemahub-server$" \
    <<<"${archive_details}"
}

test_release_archive_rejects_a_missing_gui_bundle() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture missing-gui)"
  rm -rf -- "${fixture}/apps/schemahub-gui/dist"
  local status=0

  # Act
  package_fixture "${fixture}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_release_archive_rejects_a_gui_symlink() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture gui-symlink)"
  ln -s /etc/passwd "${fixture}/apps/schemahub-gui/dist/host-file"
  local status=0

  # Act
  package_fixture "${fixture}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_release_archive_is_byte_reproducible() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture reproducible)"
  local first_output="${fixture}/first-output"
  local second_output="${fixture}/second-output"
  local first_archive="${first_output}/schemahub-${VERSION}-${TARGET}.tar.gz"
  local second_archive="${second_output}/schemahub-${VERSION}-${TARGET}.tar.gz"

  # Act
  package_fixture "${fixture}" "${first_output}" >/dev/null
  TZ=UTC find \
    "${fixture}/README.md" \
    "${fixture}/docs" \
    "${fixture}/apps/schemahub-gui/dist" \
    "${fixture}/target/${TARGET}/release" \
    -type f -exec touch -t 203001020304.05 {} +
  package_fixture "${fixture}" "${second_output}" >/dev/null

  # Assert
  cmp "${first_archive}" "${second_archive}"
}

test_release_archive_rejects_a_newline_path() {
  # Arrange
  local fixture
  fixture="$(prepare_fixture newline-path)"
  local unsafe_path="${fixture}/apps/schemahub-gui/dist/assets/unsafe"$'\n'"asset.js"
  printf 'console.log("unsafe");\n' >"${unsafe_path}"
  local status=0

  # Act
  package_fixture "${fixture}" >/dev/null 2>&1 || status=$?

  # Assert
  test "${status}" -eq 2
}

test_release_workflow_repackages_every_platform_archive() {
  # Arrange
  local workflow="${REPO_ROOT}/.github/workflows/release.yml"
  local package_invocations

  # Act
  package_invocations="$(grep -Fc 'scripts/package-release.sh \' "${workflow}")"

  # Assert
  test "${package_invocations}" -eq 2
  grep -Fq \
    'reproducibility_dir="$RUNNER_TEMP/schemahub-release-repro-$target"' \
    "${workflow}"
  grep -Fq 'cmp "$archive" "$reproducibility_archive"' "${workflow}"
}

test_release_archive_contains_the_exact_gui_bundle
test_release_archive_rejects_a_missing_gui_bundle
test_release_archive_rejects_a_gui_symlink
test_release_archive_is_byte_reproducible
test_release_archive_rejects_a_newline_path
test_release_workflow_repackages_every_platform_archive

printf 'Release package GUI and reproducibility contract tests passed.\n'
