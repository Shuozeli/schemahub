#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <version> <schemahub-revision> <protobuf-rs-revision> <flatbuffers-rs-revision> <container-image> <container-digest> <output-file>" >&2
  exit 2
}

if [[ $# -ne 7 ]]; then
  usage
fi

VERSION="$1"
SCHEMAHUB_REVISION="$2"
PROTOBUF_RS_REVISION="$3"
FLATBUFFERS_RS_REVISION="$4"
CONTAINER_IMAGE="$5"
CONTAINER_DIGEST="$6"
OUTPUT_FILE="$7"

SEMVER_PATTERN='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
REVISION_PATTERN='^([0-9a-f]{40}|[0-9a-f]{64})$'
IMAGE_PATTERN='^ghcr\.io/[a-z0-9][a-z0-9._/-]*$'
DIGEST_PATTERN='^sha256:[0-9a-f]{64}$'

if [[ ! "$VERSION" =~ $SEMVER_PATTERN ]]; then
  echo "invalid release version: $VERSION" >&2
  exit 2
fi
for revision_variable in \
  SCHEMAHUB_REVISION \
  PROTOBUF_RS_REVISION \
  FLATBUFFERS_RS_REVISION; do
  revision="${!revision_variable}"
  if [[ ! "$revision" =~ $REVISION_PATTERN ]]; then
    echo "$revision_variable must be an immutable 40- or 64-character lowercase Git commit SHA" >&2
    exit 2
  fi
done
if [[ ! "$CONTAINER_IMAGE" =~ $IMAGE_PATTERN ]]; then
  echo "container image must be a canonical lowercase GHCR repository" >&2
  exit 2
fi
if [[ ! "$CONTAINER_DIGEST" =~ $DIGEST_PATTERN ]]; then
  echo "container digest must be an immutable SHA-256 digest" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEMPLATE="$REPO_ROOT/docs/releases/$VERSION.md"
if [[ ! -f "$TEMPLATE" ]]; then
  echo "release notes are missing for $VERSION: $TEMPLATE" >&2
  exit 2
fi

CONTENTS="$(<"$TEMPLATE")"
for required_text in \
  "## Upgrade contract" \
  "Source version:" \
  "Target version:" \
  "Migration set:" \
  "Mixed-version allowance:" \
  "Rollback window:" \
  "## Compatibility changes" \
  "## Known issues" \
  "## Provenance"; do
  if [[ "$CONTENTS" != *"$required_text"* ]]; then
    echo "release notes are missing required text: $required_text" >&2
    exit 2
  fi
done
for token in \
  '{{SCHEMAHUB_VERSION}}' \
  '{{SCHEMAHUB_REVISION}}' \
  '{{PROTOBUF_RS_REVISION}}' \
  '{{FLATBUFFERS_RS_REVISION}}' \
  '{{CONTAINER_IMAGE}}' \
  '{{CONTAINER_DIGEST}}'; do
  if [[ "$CONTENTS" != *"$token"* ]]; then
    echo "release notes are missing required token: $token" >&2
    exit 2
  fi
done
if grep -Eiq '(^|[^A-Za-z])(TODO|TBD)([^A-Za-z]|$)' "$TEMPLATE"; then
  echo "release notes contain an unresolved TODO or TBD marker" >&2
  exit 2
fi

CONTENTS="${CONTENTS//'{{SCHEMAHUB_VERSION}}'/$VERSION}"
CONTENTS="${CONTENTS//'{{SCHEMAHUB_REVISION}}'/$SCHEMAHUB_REVISION}"
CONTENTS="${CONTENTS//'{{PROTOBUF_RS_REVISION}}'/$PROTOBUF_RS_REVISION}"
CONTENTS="${CONTENTS//'{{FLATBUFFERS_RS_REVISION}}'/$FLATBUFFERS_RS_REVISION}"
CONTENTS="${CONTENTS//'{{CONTAINER_IMAGE}}'/$CONTAINER_IMAGE}"
CONTENTS="${CONTENTS//'{{CONTAINER_DIGEST}}'/$CONTAINER_DIGEST}"
if grep -Eq '\{\{[A-Z0-9_]+\}\}' <<<"$CONTENTS"; then
  echo "release notes contain an unresolved template token" >&2
  exit 2
fi

OUTPUT_PARENT="$(dirname "$OUTPUT_FILE")"
mkdir -p "$OUTPUT_PARENT"
OUTPUT_PARENT="$(cd "$OUTPUT_PARENT" && pwd)"
OUTPUT_FILE="$OUTPUT_PARENT/$(basename "$OUTPUT_FILE")"
TEMP_FILE="$(mktemp "$OUTPUT_PARENT/.schemahub-release-notes.XXXXXX")"
cleanup() {
  rm -f -- "$TEMP_FILE"
}
trap cleanup EXIT
printf '%s\n' "$CONTENTS" >"$TEMP_FILE"
mv "$TEMP_FILE" "$OUTPUT_FILE"
echo "$OUTPUT_FILE"
