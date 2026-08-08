#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  validate-staging-attestation.sh \
    ATTESTATION VERSION SOURCE_REVISION CONTAINER_IMAGE \
    CONTAINER_DIGEST GA_READINESS_DIGEST

Validates a stable-release staging attestation against the exact source,
container, and retained GA evidence selected by the release workflow.
EOF
}

if [[ "$#" -ne 6 ]]; then
  usage
  exit 2
fi

ATTESTATION="$1"
VERSION="$2"
SOURCE_REVISION="$3"
CONTAINER_IMAGE="$4"
CONTAINER_DIGEST="$5"
GA_READINESS_DIGEST="$6"

for command_name in date jq; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "${command_name}" >&2
    exit 2
  fi
done

if [[ ! -s "${ATTESTATION}" ]]; then
  printf 'staging attestation is missing or empty: %s\n' "${ATTESTATION}" >&2
  exit 2
fi
if [[ ! "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'staging attestation is required only for a stable MAJOR.MINOR.PATCH release\n' >&2
  exit 2
fi
if [[ ! "${SOURCE_REVISION}" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; then
  printf 'source revision must be an immutable lowercase Git SHA\n' >&2
  exit 2
fi
if [[ ! "${CONTAINER_IMAGE}" =~ ^[a-z0-9.-]+(/[a-z0-9._-]+)+$ ]]; then
  printf 'container image must be a canonical registry/repository coordinate\n' >&2
  exit 2
fi
if [[ ! "${CONTAINER_DIGEST}" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  printf 'container digest must be an immutable sha256 coordinate\n' >&2
  exit 2
fi
if [[ ! "${GA_READINESS_DIGEST}" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  printf 'GA readiness digest must be an immutable sha256 coordinate\n' >&2
  exit 2
fi

jq -e \
  --arg version "${VERSION}" \
  --arg source_revision "${SOURCE_REVISION}" \
  --arg container_image "${CONTAINER_IMAGE}" \
  --arg container_digest "${CONTAINER_DIGEST}" \
  --arg ga_readiness_digest "${GA_READINESS_DIGEST}" '
  def exact_keys($expected):
    (keys | sort) == ($expected | sort);

  type == "object"
  and exact_keys([
    "schema_version",
    "attested_at",
    "release",
    "deployment",
    "identity",
    "acceptance",
    "evidence"
  ])
  and .schema_version == "schemahub.staging-attestation.v1"
  and (.attested_at | type == "string")
  and (.release | type == "object")
  and (.release | exact_keys([
    "version",
    "source_revision",
    "container_image",
    "container_digest",
    "ga_readiness_digest"
  ]))
  and .release.version == $version
  and .release.source_revision == $source_revision
  and .release.container_image == $container_image
  and .release.container_digest == $container_digest
  and .release.ga_readiness_digest == $ga_readiness_digest
  and (.deployment | type == "object")
  and (.deployment | exact_keys([
    "url",
    "backend",
    "exact_digest",
    "deployed_at"
  ]))
  and (.deployment.url
    | type == "string" and startswith("https://") and length <= 2048)
  and .deployment.backend == "postgres"
  and .deployment.exact_digest == true
  and (.deployment.deployed_at | type == "string")
  and (.identity | type == "object")
  and (.identity | exact_keys([
    "issuer",
    "development_credentials_used",
    "current_key_accepted",
    "next_key_accepted",
    "removed_key_rejected",
    "stale_keys_readyz_503",
    "stale_keys_credentials_rejected",
    "recovered_after_valid_jwks"
  ]))
  and (.identity.issuer
    | type == "string" and startswith("https://") and length <= 2048)
  and .identity.development_credentials_used == false
  and .identity.current_key_accepted == true
  and .identity.next_key_accepted == true
  and .identity.removed_key_rejected == true
  and .identity.stale_keys_readyz_503 == true
  and .identity.stale_keys_credentials_rejected == true
  and .identity.recovered_after_valid_jwks == true
  and (.acceptance | type == "object")
  and (.acceptance | exact_keys([
    "human_agent_workflow",
    "bundled_gui_same_origin",
    "restart_bytes_identical",
    "prior_candidate_bytes_identical",
    "corrupt_artifact_failed_closed",
    "list_dependents_live_pinned_hidden",
    "backup_restore_drill"
  ]))
  and .acceptance.human_agent_workflow == true
  and .acceptance.bundled_gui_same_origin == true
  and .acceptance.restart_bytes_identical == true
  and .acceptance.prior_candidate_bytes_identical == true
  and .acceptance.corrupt_artifact_failed_closed == true
  and .acceptance.list_dependents_live_pinned_hidden == true
  and .acceptance.backup_restore_drill == true
  and (.evidence | type == "object")
  and (.evidence | exact_keys([
    "url",
    "digest",
    "run_id",
    "operator"
  ]))
  and (.evidence.url
    | type == "string" and startswith("https://") and length <= 2048)
  and (.evidence.digest
    | type == "string" and test("^sha256:[0-9a-f]{64}$"))
  and (.evidence.run_id | type == "string" and length > 0 and length <= 256)
  and (.evidence.operator | type == "string" and length > 0 and length <= 256)
  and ([paths
    | select(.[-1] | type == "string")
    | .[-1]
    | ascii_downcase]
    | all(test("(^|_)(bearer|password|secret|token)($|_)") | not))
  and ([.. | strings]
    | all(
        test(
          "bearer[[:space:]]+[A-Za-z0-9._~-]+|eyJ[A-Za-z0-9_-]+[.]eyJ[A-Za-z0-9_-]+[.]";
          "i"
        )
        | not
      ))
' "${ATTESTATION}" >/dev/null || {
  printf 'staging attestation is malformed, incomplete, unsafe, or does not match the release\n' >&2
  exit 1
}

parse_utc_timestamp() {
  local label="$1"
  local value="$2"
  if [[ ! "${value}" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]]; then
    printf '%s must be a UTC RFC3339 timestamp ending in Z\n' "${label}" >&2
    exit 1
  fi
  date -u -d "${value}" +%s 2>/dev/null || {
    printf '%s is not a valid timestamp\n' "${label}" >&2
    exit 1
  }
}

ATTESTED_AT="$(
  jq -r '.attested_at' "${ATTESTATION}"
)"
DEPLOYED_AT="$(
  jq -r '.deployment.deployed_at' "${ATTESTATION}"
)"
ATTESTED_EPOCH="$(parse_utc_timestamp "attested_at" "${ATTESTED_AT}")"
DEPLOYED_EPOCH="$(parse_utc_timestamp "deployment.deployed_at" "${DEPLOYED_AT}")"
NOW_EPOCH="$(date -u +%s)"
MAX_AGE_SECONDS="$((7 * 24 * 60 * 60))"
MAX_CLOCK_SKEW_SECONDS=300

if (( ATTESTED_EPOCH > NOW_EPOCH + MAX_CLOCK_SKEW_SECONDS )); then
  printf 'staging attestation timestamp is in the future\n' >&2
  exit 1
fi
if (( NOW_EPOCH - ATTESTED_EPOCH > MAX_AGE_SECONDS )); then
  printf 'staging attestation is older than seven days\n' >&2
  exit 1
fi
if (( DEPLOYED_EPOCH > ATTESTED_EPOCH )); then
  printf 'staging deployment timestamp is after the attestation\n' >&2
  exit 1
fi
if (( ATTESTED_EPOCH - DEPLOYED_EPOCH > MAX_AGE_SECONDS )); then
  printf 'staging deployment is older than seven days at attestation time\n' >&2
  exit 1
fi

printf 'Staging attestation verified for %s at %s@%s\n' \
  "${VERSION}" \
  "${CONTAINER_IMAGE}" \
  "${CONTAINER_DIGEST}"
