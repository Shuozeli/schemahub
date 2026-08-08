#!/usr/bin/env bash

set -Eeuo pipefail

if [[ "$#" -ne 2 ]]; then
  printf '%s\n' \
    'Usage: validate-staging-environment.sh ENVIRONMENT_JSON DEPLOYMENT_POLICIES_JSON' \
    >&2
  exit 2
fi

ENVIRONMENT_JSON="$1"
DEPLOYMENT_POLICIES_JSON="$2"

if ! command -v jq >/dev/null 2>&1; then
  printf 'required command is missing: jq\n' >&2
  exit 2
fi
if [[ ! -s "${ENVIRONMENT_JSON}" ]]; then
  printf 'GitHub staging environment response is missing or empty: %s\n' \
    "${ENVIRONMENT_JSON}" >&2
  exit 2
fi
if [[ ! -s "${DEPLOYMENT_POLICIES_JSON}" ]]; then
  printf 'GitHub deployment policy response is missing or empty: %s\n' \
    "${DEPLOYMENT_POLICIES_JSON}" >&2
  exit 2
fi

jq -e '
  type == "object"
  and .name == "schemahub-production-staging"
  and (.protection_rules | type == "array")
  and (
    [.protection_rules[]
      | select(
          .type == "required_reviewers"
          and .prevent_self_review == true
          and (.reviewers | type == "array" and length > 0)
        )]
    | length == 1
  )
  and (.deployment_branch_policy | type == "object")
  and .deployment_branch_policy.protected_branches == false
  and .deployment_branch_policy.custom_branch_policies == true
' "${ENVIRONMENT_JSON}" >/dev/null || {
  printf '%s\n' \
    'schemahub-production-staging must require a non-self reviewer and custom release-tag policy' \
    >&2
  exit 1
}

jq -e '
  type == "object"
  and .total_count == 1
  and (.branch_policies | type == "array" and length == 1)
  and .branch_policies[0].name == "v*.*.*"
  and ((.branch_policies[0].type // "tag") == "tag")
' "${DEPLOYMENT_POLICIES_JSON}" >/dev/null || {
  printf '%s\n' \
    'schemahub-production-staging must allow only the v*.*.* release-tag policy' \
    >&2
  exit 1
}

printf 'Protected GitHub staging environment and release-tag policy verified.\n'
