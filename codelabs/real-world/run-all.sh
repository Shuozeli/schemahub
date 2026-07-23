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
  rw-02-commerce \
  rw-03-mobile-telemetry \
  rw-04-concurrent-editors \
  rw-05-data-pipeline
do
  SCHEMAHUB_CODELAB_SKIP_BUILD=1 \
    SCHEMAHUB_CODELAB_EVIDENCE_DIR="${EVIDENCE_ROOT}/${scenario}" \
    "${CODELAB_ROOT}/${scenario}/run.sh"
done

printf 'All real-world codelabs passed. Evidence: %s\n' "${EVIDENCE_ROOT}"
