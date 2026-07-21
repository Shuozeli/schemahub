#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <version> <protobuf-rs-revision> <flatbuffers-rs-revision>" >&2
  exit 2
}

if [[ $# -ne 3 ]]; then
  usage
fi

VERSION="$1"
PROTOBUF_RS_REVISION="$2"
FLATBUFFERS_RS_REVISION="$3"

SEMVER_PATTERN='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
REVISION_PATTERN='^([0-9a-f]{40}|[0-9a-f]{64})$'

if [[ ! "$VERSION" =~ $SEMVER_PATTERN ]]; then
  echo "release version must be MAJOR.MINOR.PATCH[-PRERELEASE]: $VERSION" >&2
  exit 2
fi

validate_revision() {
  local label="$1"
  local revision="$2"
  if [[ ! "$revision" =~ $REVISION_PATTERN ]]; then
    echo "$label must be an immutable 40- or 64-character lowercase Git commit SHA" >&2
    exit 2
  fi
}

validate_revision "protobuf-rs revision" "$PROTOBUF_RS_REVISION"
validate_revision "flatbuffers-rs revision" "$FLATBUFFERS_RS_REVISION"
