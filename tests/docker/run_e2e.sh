#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SHUOZELI_ROOT="$(cd "$REPO_ROOT/../.." && pwd)"
COMPILERS_ROOT="$SHUOZELI_ROOT/compilers"

IMAGE="${SCHEMAHUB_DOCKER_E2E_IMAGE:-schemahub-docker-e2e:local}"
NETWORK="schemahub-e2e-$$"
SERVER="schemahub-e2e-server-$$"
CONTEXT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/schemahub-docker-context.XXXXXX")"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/schemahub-docker-work.XXXXXX")"

cleanup() {
  docker rm -f "$SERVER" >/dev/null 2>&1 || true
  docker network rm "$NETWORK" >/dev/null 2>&1 || true
  if [[ -d "$WORK_DIR" ]] && docker image inspect "$IMAGE" >/dev/null 2>&1; then
    docker run --rm -v "$WORK_DIR:/work" "$IMAGE" \
      chown -R "$(id -u):$(id -g)" /work >/dev/null 2>&1 || true
  fi
  rm -rf "$CONTEXT_DIR" "$WORK_DIR" || true
}
trap cleanup EXIT

copy_tree() {
  local src="$1"
  local dst="$2"
  mkdir -p "$dst"
  tar \
    --exclude='./target' \
    --exclude='./.git' \
    --exclude='./node_modules' \
    -C "$src" -cf - . | tar -C "$dst" -xf -
}

require_path() {
  if [[ ! -e "$1" ]]; then
    echo "missing required path: $1" >&2
    exit 2
  fi
}

require_path "$COMPILERS_ROOT/protobuf-rs/parser/Cargo.toml"
require_path "$COMPILERS_ROOT/protobuf-rs/schema/Cargo.toml"
require_path "$COMPILERS_ROOT/protobuf-rs/codegen/Cargo.toml"
require_path "$COMPILERS_ROOT/flatbuffers-rs/parser/Cargo.toml"
require_path "$COMPILERS_ROOT/flatbuffers-rs/schema/Cargo.toml"
require_path "$COMPILERS_ROOT/flatbuffers-rs/codegen/Cargo.toml"
require_path "$COMPILERS_ROOT/flatbuffers-rs/runtime/Cargo.toml"

mkdir -p \
  "$CONTEXT_DIR/shuozeli/codegen/schemahub" \
  "$CONTEXT_DIR/shuozeli/compilers/protobuf-rs" \
  "$CONTEXT_DIR/shuozeli/compilers/flatbuffers-rs"

copy_tree "$REPO_ROOT" "$CONTEXT_DIR/shuozeli/codegen/schemahub"
copy_tree "$COMPILERS_ROOT/protobuf-rs" "$CONTEXT_DIR/shuozeli/compilers/protobuf-rs"
copy_tree "$COMPILERS_ROOT/flatbuffers-rs" "$CONTEXT_DIR/shuozeli/compilers/flatbuffers-rs"

docker build \
  -f "$REPO_ROOT/tests/docker/Dockerfile.e2e" \
  -t "$IMAGE" \
  "$CONTEXT_DIR"

docker network create "$NETWORK" >/dev/null

docker run -d \
  --name "$SERVER" \
  --network "$NETWORK" \
  "$IMAGE" \
  schemahub-server --listen 0.0.0.0:50051 --db /tmp/schemahub.redb >/dev/null

for _ in $(seq 1 60); do
  if docker run --rm --network "$NETWORK" "$IMAGE" \
    bash -lc "</dev/tcp/$SERVER/50051" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if ! docker run --rm --network "$NETWORK" "$IMAGE" \
  bash -lc "</dev/tcp/$SERVER/50051" >/dev/null 2>&1; then
  echo "schemahub-server did not become reachable" >&2
  docker logs "$SERVER" >&2 || true
  exit 1
fi

cat >"$WORK_DIR/common.proto" <<'EOF'
syntax = "proto3";
package commerce.v1;

message Money {
  string currency_code = 1;
  int64 units = 2;
  int32 nanos = 3;
}
EOF

cat >"$WORK_DIR/order.proto" <<'EOF'
syntax = "proto3";
package commerce.v1;

import "acme/commerce/common.proto";

message Order {
  string id = 1;
  Money total = 2;
}
EOF

cat >"$WORK_DIR/build_record.fbs" <<'EOF'
namespace acme.commerce;

table BuildRecord {
  id: string;
  count: int;
}

root_type BuildRecord;
EOF

cli() {
  docker run --rm \
    --network "$NETWORK" \
    -v "$WORK_DIR:/work" \
    -w /work \
    "$IMAGE" \
    schemahub --server "http://$SERVER:50051" "$@"
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local label="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    echo "assertion failed: expected $label to contain '$needle'" >&2
    echo "$haystack" >&2
    exit 1
  fi
}

assert_not_contains() {
  local haystack="$1"
  local needle="$2"
  local label="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    echo "assertion failed: expected $label not to contain '$needle'" >&2
    echo "$haystack" >&2
    exit 1
  fi
}

cli schema create /work/common.proto --project acme --repo commerce --name common.proto
cli schema create /work/order.proto --project acme --repo commerce --name order.proto
cli schema create /work/build_record.fbs --project acme --repo commerce --name build_record.fbs
cli tag create acme/commerce release-2026-06-05 --branch main
cli branch create acme/commerce feature/shipping-note --from main
cli field add acme/commerce/order.proto Order shipping_note:string:3 --branch feature/shipping-note

main_before="$(cli schema pull acme/commerce/order.proto --branch main)"
feature_after="$(cli schema pull acme/commerce/order.proto --branch feature/shipping-note)"
assert_not_contains "$main_before" "shipping_note" "main before merge"
assert_contains "$feature_after" "shipping_note" "feature branch after mutation"

cli branch merge acme/commerce feature/shipping-note --into main --message "merge shipping note"

main_after="$(cli schema pull acme/commerce/order.proto --branch main)"
release_after="$(cli schema pull acme/commerce/order.proto --branch tag:release-2026-06-05)"
assert_contains "$main_after" "shipping_note" "main after merge"
assert_not_contains "$release_after" "shipping_note" "release tag after merge"

diff_out="$(cli diff acme/commerce tag:release-2026-06-05..main --schema-path order.proto)"
assert_contains "$diff_out" "modified Order" "release-to-main diff"

log_out="$(cli log acme/commerce --branch main --limit 5)"
assert_contains "$log_out" "commit " "commit log"

op_out="$(cli op log acme/commerce --limit 10)"
assert_contains "$op_out" "op " "operation log"

cli codegen preview acme/commerce/order.proto --branch main --lang rust >"$WORK_DIR/generated.rs"
assert_contains "$(cat "$WORK_DIR/generated.rs")" "shipping_note" "generated Rust"

cat >"$WORK_DIR/Cargo.toml" <<'EOF'
[package]
name = "schemahub-docker-generated-check"
version = "0.0.0"
edition = "2021"

[dependencies]
prost = "0.13"
EOF

mkdir -p "$WORK_DIR/src"
cat >"$WORK_DIR/src/lib.rs" <<'EOF'
#![allow(warnings)]
include!("../generated.rs");
EOF

docker run --rm \
  --network "$NETWORK" \
  -e CARGO_TARGET_DIR=/tmp/schemahub-generated-target \
  -v "$WORK_DIR:/work" \
  -w /work \
  "$IMAGE" \
  cargo check --quiet

cli codegen preview acme/commerce/build_record.fbs \
  --branch main \
  --lang rust \
  --rust-pluggable-buffer >"$WORK_DIR/generated_fbs.rs"
assert_contains "$(cat "$WORK_DIR/generated_fbs.rs")" "__flatc_rs_runtime" "FlatBuffers pluggable-buffer runtime"
assert_contains "$(cat "$WORK_DIR/generated_fbs.rs")" "root_as_build_record_in" "FlatBuffers pluggable-buffer root helper"

cat >"$WORK_DIR/Cargo.toml" <<'EOF'
[package]
name = "schemahub-docker-generated-flatbuffers-check"
version = "0.0.0"
edition = "2021"

[dependencies]
flatbuffers = "25.12.19"
flatc-rs-runtime = { path = "/workspace/shuozeli/compilers/flatbuffers-rs/runtime" }
EOF

cat >"$WORK_DIR/src/lib.rs" <<'EOF'
#![allow(warnings)]
include!("../generated_fbs.rs");
EOF

docker run --rm \
  --network "$NETWORK" \
  -e CARGO_TARGET_DIR=/tmp/schemahub-generated-fbs-target \
  -v "$WORK_DIR:/work" \
  -w /work \
  "$IMAGE" \
  cargo check --quiet

echo "Docker e2e passed."
