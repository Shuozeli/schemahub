#!/usr/bin/env bash

set -Eeuo pipefail

CODELAB_ROOT="$(
  cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1
  pwd
)"
REPO_ROOT="$(
  cd -- "${CODELAB_ROOT}/../.." >/dev/null 2>&1
  pwd
)"
EVIDENCE_ROOT="${SCHEMAHUB_CODELAB_EVIDENCE_ROOT:-$(
  mktemp -d /tmp/schemahub-real-world.XXXXXX
)}"

mkdir -p "${EVIDENCE_ROOT}"

(
  cd "${REPO_ROOT}"
  CARGO_INCREMENTAL=0 cargo build \
    --locked \
    --release \
    -p schemahub-server \
    -p schemahub-cli
)

for scenario in \
  rw-01-human-agent \
  rw-02-commerce \
  rw-03-mobile-telemetry \
  rw-04-concurrent-editors \
  rw-05-data-pipeline \
  rw-06-dependency-closure \
  rw-07-tenant-isolation
do
  SCHEMAHUB_CODELAB_SKIP_BUILD=1 \
    SCHEMAHUB_CODELAB_EVIDENCE_DIR="${EVIDENCE_ROOT}/${scenario}" \
    "${CODELAB_ROOT}/${scenario}/run.sh"
done

GA_REPORT_DIR="${SCHEMAHUB_GA_REPORT_DIR:-${EVIDENCE_ROOT}/ga-readiness}"
GA_SOURCE_REVISION="${SCHEMAHUB_GA_SOURCE_REVISION:-$(
  git -C "${REPO_ROOT}" rev-parse HEAD
)}"
GA_RUN_ID="${SCHEMAHUB_GA_RUN_ID:-local}"
GA_RUN_URL="${SCHEMAHUB_GA_RUN_URL:-local}"

"${REPO_ROOT}/scripts/render-ga-readiness-report.sh" \
  "${EVIDENCE_ROOT}" \
  "${GA_REPORT_DIR}" \
  "${GA_SOURCE_REVISION}" \
  "${GA_RUN_ID}" \
  "${GA_RUN_URL}"

printf 'All real-world codelabs passed. Evidence: %s\n' "${EVIDENCE_ROOT}"
printf 'GA readiness report: %s\n' "${GA_REPORT_DIR}"
