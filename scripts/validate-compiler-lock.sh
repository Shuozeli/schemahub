#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <protobuf|flatbuffers> <revision|--development>" >&2
  exit 2
}

if [[ $# -ne 2 ]]; then
  usage
fi

COMPILER="$1"
COORDINATE="$2"
REVISION_PATTERN='^([0-9a-f]{40}|[0-9a-f]{64})$'

MODE="exact"
if [[ "$COORDINATE" == "--development" ]]; then
  MODE="development"
elif [[ ! "$COORDINATE" =~ $REVISION_PATTERN ]]; then
  echo "$COMPILER revision must be an immutable 40- or 64-character lowercase Git commit SHA" >&2
  exit 2
fi

case "$COMPILER" in
  protobuf)
    REPOSITORY_URL="https://github.com/Shuozeli/protobuf-rs.git"
    PACKAGES=(protoc-rs-parser protoc-rs-schema protoc-rs-codegen)
    ;;
  flatbuffers)
    REPOSITORY_URL="https://github.com/Shuozeli/flatbuffers-rs.git"
    PACKAGES=(flatc-rs-parser flatc-rs-schema flatc-rs-codegen)
    ;;
  *)
    usage
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCK_FILE="$SCRIPT_DIR/../Cargo.lock"
EXPECTED_PREFIX="git+$REPOSITORY_URL?rev="

package_source() {
  local package="$1"
  awk -v target="$package" '
    BEGIN {
      RS = ""
      FS = "\n"
      matches = 0
    }
    {
      name = ""
      source = ""
      for (line = 1; line <= NF; line++) {
        if ($line ~ /^name = "/) {
          name = $line
          sub(/^name = "/, "", name)
          sub(/"$/, "", name)
        } else if ($line ~ /^source = "/) {
          source = $line
          sub(/^source = "/, "", source)
          sub(/"$/, "", source)
        }
      }
      if (name == target) {
        matches++
        result = source == "" ? "<path>" : source
      }
    }
    END {
      if (matches != 1) {
        printf "expected exactly one Cargo.lock entry for %s, found %d\n", target, matches > "/dev/stderr"
        exit 3
      }
      print result
    }
  ' "$LOCK_FILE"
}

SOURCES=()
for package in "${PACKAGES[@]}"; do
  SOURCES+=("$(package_source "$package")")
done

if [[ "$MODE" == "exact" ]]; then
  EXPECTED_SOURCE="$EXPECTED_PREFIX$COORDINATE#$COORDINATE"
  for index in "${!PACKAGES[@]}"; do
    package="${PACKAGES[$index]}"
    source="${SOURCES[$index]}"
    if [[ "$source" == "<path>" ]]; then
      echo "$package is a path dependency; release builds require an immutable Git revision" >&2
      exit 2
    fi
    if [[ "$source" != "$EXPECTED_SOURCE" ]]; then
      echo "$package resolves from an unexpected source: $source" >&2
      echo "expected: $EXPECTED_SOURCE" >&2
      exit 2
    fi
  done
  exit 0
fi

path_count=0
for source in "${SOURCES[@]}"; do
  if [[ "$source" == "<path>" ]]; then
    path_count=$((path_count + 1))
  fi
done
if [[ "$path_count" -eq "${#PACKAGES[@]}" ]]; then
  echo "path"
  exit 0
fi
if [[ "$path_count" -ne 0 ]]; then
  echo "$COMPILER compiler crates mix path and Git dependencies" >&2
  exit 2
fi

LOCKED_REVISION=""
for index in "${!PACKAGES[@]}"; do
  package="${PACKAGES[$index]}"
  source="${SOURCES[$index]}"
  if [[ "$source" != "$EXPECTED_PREFIX"* ]]; then
    echo "$package resolves from a non-canonical source: $source" >&2
    exit 2
  fi
  coordinate="${source#"$EXPECTED_PREFIX"}"
  if [[ "$coordinate" != *#* || "${coordinate#*#}" == *#* ]]; then
    echo "$package does not resolve from one immutable Git coordinate: $source" >&2
    exit 2
  fi
  declared_revision="${coordinate%%#*}"
  resolved_revision="${coordinate#*#}"
  if [[ ! "$declared_revision" =~ $REVISION_PATTERN || "$resolved_revision" != "$declared_revision" ]]; then
    echo "$package does not resolve from one immutable Git revision: $source" >&2
    exit 2
  fi
  if [[ -n "$LOCKED_REVISION" && "$declared_revision" != "$LOCKED_REVISION" ]]; then
    echo "$COMPILER compiler crates resolve from different Git revisions" >&2
    exit 2
  fi
  LOCKED_REVISION="$declared_revision"
done

echo "$LOCKED_REVISION"
