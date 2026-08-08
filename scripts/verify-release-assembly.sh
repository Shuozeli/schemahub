#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
  printf 'Usage: verify-release-assembly.sh ASSEMBLY_DIRECTORY\n' >&2
}

if [[ "$#" -ne 1 ]]; then
  usage
  exit 2
fi

ASSEMBLY_DIRECTORY="$1"
for command_name in cmp comm find sha256sum sort; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "${command_name}" >&2
    exit 2
  fi
done
if [[ ! -d "${ASSEMBLY_DIRECTORY}" ]]; then
  printf 'release assembly directory does not exist: %s\n' \
    "${ASSEMBLY_DIRECTORY}" >&2
  exit 2
fi
ASSEMBLY_DIRECTORY="$(
  cd -- "${ASSEMBLY_DIRECTORY}" >/dev/null 2>&1
  pwd
)"
CHECKSUM_FILE="${ASSEMBLY_DIRECTORY}/SHA256SUMS"
CHECKSUM_LINE_PATTERN='^[0-9a-f]{64}  [A-Za-z0-9][A-Za-z0-9._-]*$'
SAFE_FILE_PATTERN='^[A-Za-z0-9][A-Za-z0-9._-]*$'
if [[ ! -s "${CHECKSUM_FILE}" || ! -f "${CHECKSUM_FILE}" ]]; then
  printf 'release assembly SHA256SUMS is missing or empty\n' >&2
  exit 1
fi

UNEXPECTED_ENTRY="$(
  find "${ASSEMBLY_DIRECTORY}" \
    -mindepth 1 \
    -maxdepth 1 \
    ! -type f \
    -print \
    -quit
)"
if [[ -n "${UNEXPECTED_ENTRY}" ]]; then
  printf 'release assembly contains a non-file entry: %s\n' \
    "${UNEXPECTED_ENTRY}" >&2
  exit 1
fi

TEMP_DIRECTORY="$(mktemp -d /tmp/schemahub-release-assembly.XXXXXX)"
trap 'rm -rf -- "${TEMP_DIRECTORY}"' EXIT
EXPECTED_NAMES="${TEMP_DIRECTORY}/expected-names.txt"
EXPECTED_NAMES_SORTED="${TEMP_DIRECTORY}/expected-names-sorted.txt"
EXPECTED_NAMES_UNIQUE="${TEMP_DIRECTORY}/expected-names-unique.txt"
ACTUAL_NAMES="${TEMP_DIRECTORY}/actual-names.txt"
ACTUAL_NAMES_SORTED="${TEMP_DIRECTORY}/actual-names-sorted.txt"
: >"${EXPECTED_NAMES}"
: >"${ACTUAL_NAMES}"

CHECKSUM_COUNT=0
while IFS= read -r checksum_line || [[ -n "${checksum_line}" ]]; do
  if [[ ! "${checksum_line}" =~ ${CHECKSUM_LINE_PATTERN} ]]; then
    printf 'release assembly contains a malformed checksum entry\n' >&2
    exit 1
  fi
  file_name="${checksum_line#*  }"
  if [[ "${file_name}" == "SHA256SUMS" ]]; then
    printf 'SHA256SUMS must not list itself\n' >&2
    exit 1
  fi
  printf '%s\n' "${file_name}" >>"${EXPECTED_NAMES}"
  CHECKSUM_COUNT="$((CHECKSUM_COUNT + 1))"
done <"${CHECKSUM_FILE}"
if [[ "${CHECKSUM_COUNT}" -eq 0 ]]; then
  printf 'release assembly contains no checksum entries\n' >&2
  exit 1
fi

LC_ALL=C sort "${EXPECTED_NAMES}" >"${EXPECTED_NAMES_SORTED}"
LC_ALL=C sort -u "${EXPECTED_NAMES}" >"${EXPECTED_NAMES_UNIQUE}"
if ! cmp -s "${EXPECTED_NAMES_SORTED}" "${EXPECTED_NAMES_UNIQUE}"; then
  printf 'release assembly contains duplicate checksum filenames\n' >&2
  exit 1
fi

while IFS= read -r -d '' file_path; do
  file_name="${file_path##*/}"
  if [[ "${file_name}" == "SHA256SUMS" ]]; then
    continue
  fi
  if [[ ! "${file_name}" =~ ${SAFE_FILE_PATTERN} ]]; then
    printf 'release assembly contains an unsafe filename: %s\n' \
      "${file_name}" >&2
    exit 1
  fi
  printf '%s\n' "${file_name}" >>"${ACTUAL_NAMES}"
done < <(
  find "${ASSEMBLY_DIRECTORY}" \
    -mindepth 1 \
    -maxdepth 1 \
    -type f \
    -print0
)
LC_ALL=C sort "${ACTUAL_NAMES}" >"${ACTUAL_NAMES_SORTED}"
if ! cmp -s "${EXPECTED_NAMES_SORTED}" "${ACTUAL_NAMES_SORTED}"; then
  printf 'release assembly file set does not match SHA256SUMS:\n' >&2
  comm -3 \
    "${EXPECTED_NAMES_SORTED}" \
    "${ACTUAL_NAMES_SORTED}" >&2
  exit 1
fi

(
  cd -- "${ASSEMBLY_DIRECTORY}"
  sha256sum --check --strict --quiet SHA256SUMS
) || {
  printf 'release assembly checksum verification failed\n' >&2
  exit 1
}

printf 'Release assembly verified: %s checksummed files.\n' \
  "${CHECKSUM_COUNT}"
