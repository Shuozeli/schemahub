#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <version> <target-triple> <output-directory>" >&2
  exit 2
}

if [[ $# -ne 3 ]]; then
  usage
fi

VERSION="$1"
TARGET="$2"
OUTPUT_DIR="$3"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
  echo "invalid release version: $VERSION" >&2
  exit 2
fi
if [[ ! "$TARGET" =~ ^[0-9A-Za-z_.-]+$ ]]; then
  echo "invalid Rust target triple: $TARGET" >&2
  exit 2
fi

REVISION_PATTERN='^([0-9a-f]{40}|[0-9a-f]{64})$'
for revision_variable in \
  SCHEMAHUB_REVISION \
  PROTOBUF_RS_REVISION \
  FLATBUFFERS_RS_REVISION; do
  revision="${!revision_variable:-}"
  if [[ ! "$revision" =~ $REVISION_PATTERN ]]; then
    echo "$revision_variable must be an immutable 40- or 64-character lowercase Git commit SHA" >&2
    exit 2
  fi
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_DIR="$REPO_ROOT/target/$TARGET/release"
EXE_SUFFIX=""
if [[ "$TARGET" == *-windows-* ]]; then
  EXE_SUFFIX=".exe"
fi

SERVER_BIN="$BIN_DIR/schemahub-server$EXE_SUFFIX"
CLI_BIN="$BIN_DIR/schemahub$EXE_SUFFIX"
for binary in "$SERVER_BIN" "$CLI_BIN"; do
  if [[ ! -x "$binary" ]]; then
    echo "release binary is missing or not executable: $binary" >&2
    exit 2
  fi
done

EXPECTED_SERVER_VERSION="schemahub-server $VERSION"
EXPECTED_CLI_VERSION="schemahub $VERSION"
if [[ "$($SERVER_BIN --version)" != "$EXPECTED_SERVER_VERSION" ]]; then
  echo "server binary version does not match $VERSION" >&2
  exit 2
fi
if [[ "$($CLI_BIN --version)" != "$EXPECTED_CLI_VERSION" ]]; then
  echo "CLI binary version does not match $VERSION" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"
STAGE_DIR="$(mktemp -d "$OUTPUT_DIR/.schemahub-release.XXXXXX")"
PACKAGE_NAME="schemahub-$VERSION-$TARGET"

cleanup() {
  if [[ -n "${STAGE_DIR:-}" && -d "$STAGE_DIR" ]]; then
    rm -rf -- "$STAGE_DIR"
  fi
}
trap cleanup EXIT

mkdir -p "$STAGE_DIR/$PACKAGE_NAME"
cp "$SERVER_BIN" "$CLI_BIN" "$STAGE_DIR/$PACKAGE_NAME/"
cp "$REPO_ROOT/README.md" "$STAGE_DIR/$PACKAGE_NAME/"
OPENAPI_FILE="$STAGE_DIR/$PACKAGE_NAME/schemahub-http-openapi.json"
"$SERVER_BIN" --print-openapi >"$OPENAPI_FILE"
if [[ ! -s "$OPENAPI_FILE" ]]; then
  echo "generated OpenAPI document is empty" >&2
  exit 2
fi
if [[ -f "$REPO_ROOT/docs/compatibility-policy.md" ]]; then
  cp "$REPO_ROOT/docs/compatibility-policy.md" "$STAGE_DIR/$PACKAGE_NAME/"
fi
printf '%s\n' \
  "schemahub_version=$VERSION" \
  "target=$TARGET" \
  "schemahub_revision=$SCHEMAHUB_REVISION" \
  "protobuf_rs_revision=$PROTOBUF_RS_REVISION" \
  "flatbuffers_rs_revision=$FLATBUFFERS_RS_REVISION" \
  "http_openapi=schemahub-http-openapi.json" \
  >"$STAGE_DIR/$PACKAGE_NAME/BUILD-METADATA.txt"

tar -C "$STAGE_DIR" -czf "$OUTPUT_DIR/$PACKAGE_NAME.tar.gz" "$PACKAGE_NAME"
echo "$OUTPUT_DIR/$PACKAGE_NAME.tar.gz"
