#!/usr/bin/env bash
set -Eeuo pipefail

REPOSITORY_ROOT="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." >/dev/null 2>&1
  pwd
)"
cd "${REPOSITORY_ROOT}"

fail() {
  printf 'container supply-chain policy test failed: %s\n' "$*" >&2
  exit 1
}

assert_exact_line() {
  local expected="$1"
  local subject="$2"
  local count
  count="$(grep -Fxc -- "${expected}" Dockerfile || true)"
  if [[ "${count}" != "1" ]]; then
    fail "${subject}: expected one exact Dockerfile line, found ${count}"
  fi
}

test_release_bases_and_frontend_are_exact_multi_architecture_manifests() {
  # Arrange
  local external_from_count
  local digest_from_count

  # Act
  external_from_count="$(grep -Ec '^FROM ' Dockerfile || true)"
  digest_from_count="$(
    grep -Ec '^FROM [^[:space:]]+@sha256:[0-9a-f]{64} AS [a-z-]+$' \
      Dockerfile || true
  )"

  # Assert
  assert_exact_line \
    "# syntax=docker/dockerfile:1.7@sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e" \
    "Dockerfile frontend manifest"
  test "${external_from_count}" -eq 3
  test "${digest_from_count}" -eq 3
  assert_exact_line \
    "FROM node:24-bookworm-slim@sha256:6f7b03f7c2c8e2e784dcf9295400527b9b1270fd37b7e9a7285cf83b6951452d AS gui-builder" \
    "Node GUI builder manifest"
  assert_exact_line \
    "FROM rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS builder" \
    "Rust builder manifest"
  assert_exact_line \
    "FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS runtime" \
    "distroless runtime manifest"
}

test_container_build_tools_cannot_be_overridden() {
  # Arrange
  local install_count
  local pnpm_install_count
  local pnpm_version_arg_count
  local version_arg_count

  # Act
  install_count="$(
    grep -Fxc \
      '    cargo install cargo-auditable --locked --version 0.7.5 --force' \
      Dockerfile || true
  )"
  version_arg_count="$(
    grep -Ec '^ARG CARGO_AUDITABLE_VERSION=' Dockerfile || true
  )"
  pnpm_install_count="$(
    grep -Fxc \
      "    && corepack prepare pnpm@11.2.2 --activate \\" \
      Dockerfile || true
  )"
  pnpm_version_arg_count="$(
    grep -Ec '^ARG PNPM_VERSION=' Dockerfile || true
  )"

  # Assert
  test "${install_count}" -eq 1
  test "${version_arg_count}" -eq 0
  test "${pnpm_install_count}" -eq 1
  test "${pnpm_version_arg_count}" -eq 0
}

test_ci_helper_images_are_exact_manifests() {
  # Arrange
  local postgres_count
  local curl_count

  # Act
  postgres_count="$(
    grep -Fxc \
      '        image: postgres:17-bookworm@sha256:4f736ae292687621d4dbe0d499ffd024a36bd2ee7d8ca6f2ccd4c800f047b394' \
      .github/workflows/ci.yml || true
  )"
  curl_count="$(
    grep -Fxc \
      "CURL_IMAGE=\"\${SCHEMAHUB_CONTAINER_CURL_IMAGE:-curlimages/curl:8.14.1@sha256:9a1ed35addb45476afa911696297f8e115993df459278ed036182dd2cd22b67b}\"" \
      scripts/test-runtime-container.sh || true
  )"

  # Assert
  test "${postgres_count}" -eq 1
  test "${curl_count}" -eq 1
}

test_workflow_node_runtime_is_exact() {
  # Arrange
  local exact_count
  local moving_count

  # Act
  exact_count="$(
    {
      grep -RhE '^[[:space:]]+node-version: 24[.]18[.]0$' \
        .github/workflows || true
    } | wc -l | tr -d '[:space:]'
  )"
  moving_count="$(
    {
      grep -RhE '^[[:space:]]+node-version: 24$' \
        .github/workflows || true
    } | wc -l | tr -d '[:space:]'
  )"

  # Assert
  test "${exact_count}" -eq 3
  test "${moving_count}" -eq 0
}

test_external_actions_are_commit_pinned() {
  # Arrange
  local invalid_action=""
  local workflow
  local action

  # Act
  while IFS= read -r workflow; do
    while IFS= read -r action; do
      action="${action#*uses: }"
      if [[ "${action}" == ./* ]]; then
        continue
      fi
      action="${action%% *}"
      if [[ ! "${action}" =~ ^[^@[:space:]]+@[0-9a-f]{40}$ ]]; then
        invalid_action="${workflow}: ${action}"
        break 2
      fi
    done < <(grep -E '^[[:space:]]+uses: ' "${workflow}" || true)
  done < <(find .github/workflows -maxdepth 1 -type f -name '*.yml' -print | sort)

  # Assert
  if [[ -n "${invalid_action}" ]]; then
    fail "external action is not commit-pinned: ${invalid_action}"
  fi
}

test_release_bases_and_frontend_are_exact_multi_architecture_manifests
test_container_build_tools_cannot_be_overridden
test_ci_helper_images_are_exact_manifests
test_workflow_node_runtime_is_exact
test_external_actions_are_commit_pinned

printf '%s\n' 'container and workflow supply-chain policy tests passed.'
