#!/usr/bin/env bash
set -Eeuo pipefail

EXPECTED_AUDITOR_VERSION="0.22.2"
REPORT_FILE="$(mktemp /tmp/schemahub-cargo-audit.XXXXXX.json)"
trap 'rm -f -- "${REPORT_FILE}"' EXIT

if [[ -n "${SCHEMAHUB_CARGO_AUDIT_BIN:-}" ]]; then
  if [[ "${SCHEMAHUB_CARGO_AUDIT_BIN}" != /* ]] \
    || [[ ! -x "${SCHEMAHUB_CARGO_AUDIT_BIN}" ]]
  then
    printf 'SCHEMAHUB_CARGO_AUDIT_BIN must be an executable absolute path\n' >&2
    exit 2
  fi
  AUDITOR=("${SCHEMAHUB_CARGO_AUDIT_BIN}" audit)
else
  AUDITOR=(cargo audit)
fi

AUDITOR_VERSION="$("${AUDITOR[@]}" --version)"
if [[ ! "${AUDITOR_VERSION}" =~ ^cargo-audit(-audit)?[[:space:]]${EXPECTED_AUDITOR_VERSION}$ ]]
then
  printf 'cargo-audit version mismatch: expected %s, got %s\n' \
    "${EXPECTED_AUDITOR_VERSION}" \
    "${AUDITOR_VERSION}" >&2
  exit 2
fi

AUDIT_EXIT_CODE=0
"${AUDITOR[@]}" --json >"${REPORT_FILE}" || AUDIT_EXIT_CODE=$?
if [[ "${AUDIT_EXIT_CODE}" -ne 0 ]]; then
  printf 'cargo-audit failed with status %s\n' "${AUDIT_EXIT_CODE}" >&2
  jq '{
    vulnerabilities: .vulnerabilities,
    warnings: .warnings
  }' "${REPORT_FILE}" >&2 2>/dev/null || true
  exit 1
fi

if ! jq -e '
  def normalized_warnings:
    [
      .warnings
      | to_entries[]
      | .value[]
      | {
          kind: .kind,
          name: .package.name,
          version: .package.version,
          source: .package.source,
          advisory_id: (.advisory.id // null)
        }
    ]
    | sort_by(.kind, .name, .version);

  .vulnerabilities.found == false
  and .vulnerabilities.count == 0
  and .vulnerabilities.list == []
  and normalized_warnings == [
    {
      kind: "unmaintained",
      name: "paste",
      version: "1.0.15",
      source: "registry+https://github.com/rust-lang/crates.io-index",
      advisory_id: "RUSTSEC-2024-0436"
    },
    {
      kind: "yanked",
      name: "spin",
      version: "0.9.8",
      source: "registry+https://github.com/rust-lang/crates.io-index",
      advisory_id: null
    }
  ]
' "${REPORT_FILE}" >/dev/null
then
  printf 'Rust dependency audit has a vulnerability or unreviewed warning\n' >&2
  jq '{
    vulnerabilities: .vulnerabilities,
    warnings: [
      .warnings
      | to_entries[]
      | .value[]
      | {
          kind: .kind,
          name: .package.name,
          version: .package.version,
          advisory_id: (.advisory.id // null)
        }
    ]
  }' "${REPORT_FILE}" >&2 2>/dev/null || true
  exit 1
fi

printf '%s\n' \
  "Rust dependency audit passed with no vulnerabilities." \
  "Reviewed warnings: RUSTSEC-2024-0436 paste 1.0.15; yanked spin 0.9.8."
