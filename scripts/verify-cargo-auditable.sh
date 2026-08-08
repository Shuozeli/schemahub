#!/usr/bin/env bash
set -Eeuo pipefail

AUDITABLE_VERSION="0.7.5"
AUDITABLE_ARCHIVE_SHA256="cd121127b91d68074770a620544182345d7db56d03dcbd85316ab11e54a5b1bc"
AUDITABLE_LOCK_SHA256="3a49de28391ca0e99709a96c64cd8e24f8f96d622f5a8360c2fbd5d8e0d9965e"
WORK_PARENT="${RUNNER_TEMP:-/tmp}"

if [[ -z "${SCHEMAHUB_CARGO_AUDIT_BIN:-}" ]] \
  || [[ "${SCHEMAHUB_CARGO_AUDIT_BIN}" != /* ]] \
  || [[ ! -x "${SCHEMAHUB_CARGO_AUDIT_BIN}" ]]
then
  printf 'SCHEMAHUB_CARGO_AUDIT_BIN must name the verified absolute auditor binary\n' >&2
  exit 2
fi

if [[ "${WORK_PARENT}" != /* ]] || [[ ! -d "${WORK_PARENT}" ]]; then
  printf 'cargo-auditable work parent must be an existing absolute directory: %s\n' \
    "${WORK_PARENT}" >&2
  exit 2
fi

WORK_DIR="$(mktemp -d "${WORK_PARENT%/}/schemahub-cargo-auditable.XXXXXX")"
trap 'rm -rf -- "${WORK_DIR}"' EXIT

ARCHIVE="${WORK_DIR}/cargo-auditable-${AUDITABLE_VERSION}.crate"
if [[ -n "${SCHEMAHUB_CARGO_AUDITABLE_ARCHIVE:-}" ]]; then
  if [[ "${SCHEMAHUB_CARGO_AUDITABLE_ARCHIVE}" != /* ]] \
    || [[ ! -f "${SCHEMAHUB_CARGO_AUDITABLE_ARCHIVE}" ]]
  then
    printf 'SCHEMAHUB_CARGO_AUDITABLE_ARCHIVE must be an absolute regular file\n' >&2
    exit 2
  fi
  cp -- "${SCHEMAHUB_CARGO_AUDITABLE_ARCHIVE}" "${ARCHIVE}"
else
  curl \
    --fail \
    --location \
    --proto '=https' \
    --retry 3 \
    --show-error \
    --silent \
    --tlsv1.2 \
    "https://static.crates.io/crates/cargo-auditable/cargo-auditable-${AUDITABLE_VERSION}.crate" \
    --output "${ARCHIVE}"
fi

ACTUAL_ARCHIVE_SHA256="$(sha256sum "${ARCHIVE}" | awk '{print $1}')"
if [[ "${ACTUAL_ARCHIVE_SHA256}" != "${AUDITABLE_ARCHIVE_SHA256}" ]]; then
  printf 'cargo-auditable source archive checksum mismatch: expected %s, got %s\n' \
    "${AUDITABLE_ARCHIVE_SHA256}" \
    "${ACTUAL_ARCHIVE_SHA256}" >&2
  exit 2
fi

ARCHIVE_LISTING="$(tar -tzf "${ARCHIVE}")"
SOURCE_NAME="cargo-auditable-${AUDITABLE_VERSION}"
if ! awk -v prefix="${SOURCE_NAME}/" '
  index($0, prefix) != 1 {
    invalid = 1
  }
  END {
    exit invalid
  }
' <<<"${ARCHIVE_LISTING}"
then
  printf 'cargo-auditable archive contains an entry outside %s/\n' \
    "${SOURCE_NAME}" >&2
  exit 2
fi

tar -xzf "${ARCHIVE}" -C "${WORK_DIR}"
SOURCE_DIR="${WORK_DIR}/${SOURCE_NAME}"
if [[ ! -f "${SOURCE_DIR}/Cargo.toml" ]] \
  || [[ ! -f "${SOURCE_DIR}/Cargo.lock" ]]
then
  printf 'cargo-auditable archive is missing its manifest or published lock\n' >&2
  exit 2
fi

ACTUAL_LOCK_SHA256="$(
  sha256sum "${SOURCE_DIR}/Cargo.lock" | awk '{print $1}'
)"
if [[ "${ACTUAL_LOCK_SHA256}" != "${AUDITABLE_LOCK_SHA256}" ]]; then
  printf 'cargo-auditable lock checksum mismatch: expected %s, got %s\n' \
    "${AUDITABLE_LOCK_SHA256}" \
    "${ACTUAL_LOCK_SHA256}" >&2
  exit 2
fi

cargo metadata \
  --manifest-path "${SOURCE_DIR}/Cargo.toml" \
  --locked \
  --format-version=1 >/dev/null
"${SCHEMAHUB_CARGO_AUDIT_BIN}" audit \
  --file "${SOURCE_DIR}/Cargo.lock" \
  --deny warnings

printf '%s\n' \
  "cargo-auditable ${AUDITABLE_VERSION} source and lock identities are exact." \
  "Its complete published dependency lock passed the verified RustSec auditor."
