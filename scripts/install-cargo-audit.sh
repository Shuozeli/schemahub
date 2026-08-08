#!/usr/bin/env bash
set -Eeuo pipefail

AUDITOR_VERSION="0.22.2"
AUDITOR_ARCHIVE_SHA256="700c2b240f7fd330c24b675fe429f73a5b676531fcc6300400b2b67f155ba12a"
AUDITOR_LOCK_SHA256="02b6d4858475e8028b9e35aa7e86de2b06ae42df9432c9a5e6037d01e0ed9947"

SCRIPT_DIR="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
REPOSITORY_ROOT="$(cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"
REVIEWED_LOCK="${REPOSITORY_ROOT}/tools/cargo-audit/Cargo.lock"
WORK_PARENT="${RUNNER_TEMP:-/tmp}"

if [[ "${WORK_PARENT}" != /* ]] || [[ ! -d "${WORK_PARENT}" ]]; then
  printf 'cargo-audit work parent must be an existing absolute directory: %s\n' \
    "${WORK_PARENT}" >&2
  exit 2
fi

if [[ ! -f "${REVIEWED_LOCK}" ]]; then
  printf 'reviewed cargo-audit lock is missing: %s\n' "${REVIEWED_LOCK}" >&2
  exit 2
fi

ACTUAL_LOCK_SHA256="$(sha256sum "${REVIEWED_LOCK}" | awk '{print $1}')"
if [[ "${ACTUAL_LOCK_SHA256}" != "${AUDITOR_LOCK_SHA256}" ]]; then
  printf 'reviewed cargo-audit lock checksum mismatch: expected %s, got %s\n' \
    "${AUDITOR_LOCK_SHA256}" \
    "${ACTUAL_LOCK_SHA256}" >&2
  exit 2
fi

WORK_DIR="$(mktemp -d "${WORK_PARENT%/}/schemahub-cargo-audit.XXXXXX")"
trap 'rm -rf -- "${WORK_DIR}"' EXIT

ARCHIVE="${WORK_DIR}/cargo-audit-${AUDITOR_VERSION}.crate"
if [[ -n "${SCHEMAHUB_CARGO_AUDIT_ARCHIVE:-}" ]]; then
  if [[ "${SCHEMAHUB_CARGO_AUDIT_ARCHIVE}" != /* ]] \
    || [[ ! -f "${SCHEMAHUB_CARGO_AUDIT_ARCHIVE}" ]]
  then
    printf 'SCHEMAHUB_CARGO_AUDIT_ARCHIVE must be an absolute regular file\n' >&2
    exit 2
  fi
  cp -- "${SCHEMAHUB_CARGO_AUDIT_ARCHIVE}" "${ARCHIVE}"
else
  curl \
    --fail \
    --location \
    --proto '=https' \
    --retry 3 \
    --show-error \
    --silent \
    --tlsv1.2 \
    "https://static.crates.io/crates/cargo-audit/cargo-audit-${AUDITOR_VERSION}.crate" \
    --output "${ARCHIVE}"
fi

ACTUAL_ARCHIVE_SHA256="$(sha256sum "${ARCHIVE}" | awk '{print $1}')"
if [[ "${ACTUAL_ARCHIVE_SHA256}" != "${AUDITOR_ARCHIVE_SHA256}" ]]; then
  printf 'cargo-audit source archive checksum mismatch: expected %s, got %s\n' \
    "${AUDITOR_ARCHIVE_SHA256}" \
    "${ACTUAL_ARCHIVE_SHA256}" >&2
  exit 2
fi

ARCHIVE_LISTING="$(tar -tzf "${ARCHIVE}")"
SOURCE_NAME="cargo-audit-${AUDITOR_VERSION}"
if ! awk -v prefix="${SOURCE_NAME}/" '
  index($0, prefix) != 1 {
    invalid = 1
  }
  END {
    exit invalid
  }
' <<<"${ARCHIVE_LISTING}"
then
  printf 'cargo-audit archive contains an entry outside %s/\n' \
    "${SOURCE_NAME}" >&2
  exit 2
fi

tar -xzf "${ARCHIVE}" -C "${WORK_DIR}"
SOURCE_DIR="${WORK_DIR}/${SOURCE_NAME}"
if [[ ! -f "${SOURCE_DIR}/Cargo.toml" ]] \
  || [[ ! -f "${SOURCE_DIR}/Cargo.lock" ]]
then
  printf 'cargo-audit archive is missing its manifest or published lock\n' >&2
  exit 2
fi

mv -- "${SOURCE_DIR}/Cargo.lock" "${SOURCE_DIR}/Cargo.lock.published"
cp -- "${REVIEWED_LOCK}" "${SOURCE_DIR}/Cargo.lock"
cargo metadata \
  --manifest-path "${SOURCE_DIR}/Cargo.toml" \
  --locked \
  --format-version=1 >/dev/null

INSTALL_ARGS=(
  install
  --path "${SOURCE_DIR}"
  --locked
  --force
)
if [[ -n "${SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT:-}" ]]; then
  if [[ "${SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT}" != /* ]]; then
    printf 'SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT must be an absolute path\n' >&2
    exit 2
  fi
  mkdir -p -- "${SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT}"
  INSTALL_ARGS+=(--root "${SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT}")
fi

CARGO_INCREMENTAL=0 cargo "${INSTALL_ARGS[@]}"

if [[ -n "${SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT:-}" ]]; then
  AUDITOR=(
    "${SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT}/bin/cargo-audit"
    audit
  )
else
  AUDITOR=(cargo audit)
fi

AUDITOR_VERSION_OUTPUT="$("${AUDITOR[@]}" --version)"
if [[ ! "${AUDITOR_VERSION_OUTPUT}" =~ ^cargo-audit(-audit)?[[:space:]]${AUDITOR_VERSION}$ ]]
then
  printf 'installed cargo-audit version mismatch: expected %s, got %s\n' \
    "${AUDITOR_VERSION}" \
    "${AUDITOR_VERSION_OUTPUT}" >&2
  exit 2
fi

"${AUDITOR[@]}" \
  --file "${SOURCE_DIR}/Cargo.lock" \
  --deny warnings

printf '%s\n' \
  "Installed cargo-audit ${AUDITOR_VERSION} from the checksummed crates.io source." \
  "The exact reviewed auditor dependency lock passed its own RustSec audit."
