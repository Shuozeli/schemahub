#!/usr/bin/env bash
# Integration test script for SchemaHub
# Starts a temporary server, runs each test case, reports PASS/FAIL

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN="$REPO_ROOT/target/debug/schemahub"
SERVER_BIN="$REPO_ROOT/target/debug/schemahub-server"
PROTO_DIR="$REPO_ROOT/crates/schemahub-api/proto"
TEST_DIR="$SCRIPT_DIR"

TMPDIR_LOCAL="$(mktemp -d)"
DB_FILE="$TMPDIR_LOCAL/test.db"
SERVER_LOG="$TMPDIR_LOCAL/server.log"
SERVER_PID=""
SERVER_PORT=59100
SERVER_ADDR="127.0.0.1:$SERVER_PORT"
SERVER_URL="http://127.0.0.1:$SERVER_PORT"
CLI="$BIN --server $SERVER_URL"
GRPC="grpcurl -plaintext -import-path $PROTO_DIR"

PASS=0
FAIL=0
declare -a RESULTS=()

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$TMPDIR_LOCAL"
}
trap cleanup EXIT

start_server() {
  "$SERVER_BIN" --listen "$SERVER_ADDR" --db "$DB_FILE" >"$SERVER_LOG" 2>&1 &
  SERVER_PID=$!
  for i in $(seq 1 40); do
    if grep -q "Listening" "$SERVER_LOG" 2>/dev/null; then
      break
    fi
    sleep 0.2
  done
  sleep 0.3
}

grpc_call() {
  local proto="$1"
  local method="$2"
  local data="$3"
  $GRPC -proto "schemahub/v1/${proto}" -d "$data" "$SERVER_ADDR" "$method" 2>&1
}

pass() {
  local name="$1"
  PASS=$((PASS + 1))
  RESULTS+=("PASS  $name")
  printf "  [PASS] %s\n" "$name"
}

fail() {
  local name="$1"
  local reason="${2:-}"
  FAIL=$((FAIL + 1))
  RESULTS+=("FAIL  $name${reason:+ | $reason}")
  printf "  [FAIL] %s%s\n" "$name" "${reason:+ | $reason}"
}

# ─── Start server ─────────────────────────────────────────────────────────────

printf "Starting schemahub-server on %s ...\n" "$SERVER_ADDR"
start_server
printf "Server PID: %s\n\n" "$SERVER_PID"

# ─── Setup: Initialize project and repo via CLI `repo init` ───────────────────

echo "=== Setup: Project & Repo Initialization (via CLI repo init) ==="

# S01: repo init acme/platform
OUT=$($CLI repo init acme/platform --public 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "Created repo"; then
  pass "S01: repo init acme/platform"
else
  fail "S01: repo init acme/platform" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# S02: repo init is idempotent — re-running same project should skip project creation
OUT=$($CLI repo init acme/analytics 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "already exists"; then
  pass "S02: repo init acme/analytics skips existing project"
else
  fail "S02: repo init acme/analytics skips existing project" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# Setup second repo for isolation tests
$CLI repo init other/schemas >/dev/null 2>&1

echo ""

# ─── Group 1: Schema Lifecycle ────────────────────────────────────────────────

echo "=== Group 1: Schema Lifecycle ==="

# T01 — create user.proto
OUT=$($CLI schema create --project acme --repo platform "$TEST_DIR/user.proto" 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T01: schema create user.proto on main"
else
  fail "T01: schema create user.proto on main" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T02 — pull returns human-readable proto source
OUT=$($CLI schema pull acme/platform/user.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "message User"; then
  pass "T02: schema pull user.proto returns proto source text"
else
  fail "T02: schema pull user.proto returns proto source text" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# T03 — schema update
sed 's/string email = 3;/string email = 3;\n  string phone = 4;/' "$TEST_DIR/user.proto" > "$TMPDIR_LOCAL/user_updated.proto"
OUT=$($CLI schema update --project acme --repo platform "$TMPDIR_LOCAL/user_updated.proto" --name user.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T03: schema update user.proto (add phone field)"
else
  fail "T03: schema update user.proto (add phone field)" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T04 — pull after update contains new field
OUT=$($CLI schema pull acme/platform/user.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "phone"; then
  pass "T04: schema pull after update contains 'phone' field"
else
  fail "T04: schema pull after update contains 'phone' field" "exit=$EXIT | $(echo "$OUT" | head -3)"
fi

# T05 — create order.proto
OUT=$($CLI schema create --project acme --repo platform "$TEST_DIR/order.proto" 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T05: schema create order.proto"
else
  fail "T05: schema create order.proto" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T06 — create FlatBuffers schema
OUT=$($CLI schema create --project acme --repo platform "$TEST_DIR/item.fbs" 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T06: schema create item.fbs (FlatBuffers)"
else
  fail "T06: schema create item.fbs (FlatBuffers)" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# ─── Group 2: Field Mutations ─────────────────────────────────────────────────

echo ""
echo "=== Group 2: Field Mutations ==="

# T07 — field add
OUT=$($CLI field add acme/platform/user.proto User age:int32:5 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T07: field add age:int32:5 to User"
else
  fail "T07: field add age:int32:5 to User" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T08 — pull shows newly added field in source text
OUT=$($CLI schema pull acme/platform/user.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "age"; then
  pass "T08: schema pull shows added field 'age'"
else
  fail "T08: schema pull shows added field 'age'" "exit=$EXIT | $(echo "$OUT" | head -3)"
fi

# T09 — field rename
OUT=$($CLI field rename acme/platform/user.proto User age years_old 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T09: field rename age -> years_old"
else
  fail "T09: field rename age -> years_old" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T10 — pull shows renamed field in source text
OUT=$($CLI schema pull acme/platform/user.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "years_old"; then
  pass "T10: schema pull shows renamed field 'years_old'"
else
  fail "T10: schema pull shows renamed field 'years_old'" "exit=$EXIT | $(echo "$OUT" | head -3)"
fi

# T11 — field remove
OUT=$($CLI field remove acme/platform/user.proto User years_old 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T11: field remove years_old"
else
  fail "T11: field remove years_old" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T12 — verify removed field is not active (may be reserved)
OUT=$($CLI schema pull acme/platform/user.proto 2>&1)
if ! echo "$OUT" | grep -qE "years_old\s*=\s*5[^0-9]"; then
  pass "T12: field 'years_old = 5' no longer active after remove"
else
  fail "T12: field 'years_old = 5' no longer active after remove" "$(echo "$OUT" | head -5)"
fi

# ─── Group 3: Branch Management ───────────────────────────────────────────────

echo ""
echo "=== Group 3: Branch Management ==="

# T13 — branch create
OUT=$($CLI branch create acme/platform feature/new-api 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T13: branch create feature/new-api"
else
  fail "T13: branch create feature/new-api" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T14 — branch list
OUT=$($CLI branch list acme/platform 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "feature/new-api"; then
  pass "T14: branch list shows feature/new-api"
else
  fail "T14: branch list shows feature/new-api" "exit=$EXIT | $(echo "$OUT" | head -3)"
fi

# T15 — create schema on feature branch
OUT=$($CLI schema create --project acme --repo platform --branch feature/new-api "$TEST_DIR/user.proto" --name user_v2.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T15: schema create user_v2.proto on feature/new-api branch"
else
  fail "T15: schema create user_v2.proto on feature/new-api branch" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T16 — branch merge
OUT=$($CLI branch merge acme/platform feature/new-api --into main 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T16: branch merge feature/new-api into main"
else
  fail "T16: branch merge feature/new-api into main" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T17 — merged schema visible on main
OUT=$($CLI schema pull acme/platform/user_v2.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T17: merged schema user_v2.proto visible on main"
else
  fail "T17: merged schema user_v2.proto visible on main" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T18 — branch delete
OUT=$($CLI branch delete acme/platform feature/new-api 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T18: branch delete feature/new-api"
else
  fail "T18: branch delete feature/new-api" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T19 — deleted branch absent from list
OUT=$($CLI branch list acme/platform 2>&1)
if ! echo "$OUT" | grep -qF "feature/new-api"; then
  pass "T19: deleted branch not in branch list"
else
  fail "T19: deleted branch not in branch list" "$(echo "$OUT" | head -3)"
fi

# ─── Group 4: Tag Management ──────────────────────────────────────────────────

echo ""
echo "=== Group 4: Tag Management ==="

# T20 — tag create
OUT=$($CLI tag create acme/platform v1.0.0 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T20: tag create v1.0.0"
else
  fail "T20: tag create v1.0.0" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T21 — tag list
OUT=$($CLI tag list acme/platform 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "v1.0.0"; then
  pass "T21: tag list shows v1.0.0"
else
  fail "T21: tag list shows v1.0.0" "exit=$EXIT | $(echo "$OUT" | head -3)"
fi

# T22 — tag create with message
OUT=$($CLI tag create acme/platform v1.1.0 --message "Release 1.1" 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T22: tag create v1.1.0 with message"
else
  fail "T22: tag create v1.1.0 with message" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T23 — tag delete
OUT=$($CLI tag delete acme/platform v1.1.0 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T23: tag delete v1.1.0"
else
  fail "T23: tag delete v1.1.0" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T24 — deleted tag absent from list
OUT=$($CLI tag list acme/platform 2>&1)
if ! echo "$OUT" | grep -qF "v1.1.0"; then
  pass "T24: deleted tag v1.1.0 not in tag list"
else
  fail "T24: deleted tag v1.1.0 not in tag list" "$(echo "$OUT" | head -3)"
fi

# ─── Group 5: Log ─────────────────────────────────────────────────────────────

echo ""
echo "=== Group 5: Log ==="

# T25 — log shows commits
OUT=$($CLI log acme/platform 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]] && [[ -n "$OUT" ]]; then
  pass "T25: log acme/platform returns commit history"
else
  fail "T25: log acme/platform returns commit history" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T26 — log with limit
OUT=$($CLI log acme/platform --limit 2 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T26: log with --limit 2"
else
  fail "T26: log with --limit 2" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T27 — log output contains commit hashes
OUT=$($CLI log acme/platform --limit 5 2>&1)
COMMIT_COUNT=$(echo "$OUT" | grep -cE "^commit [0-9a-f]" 2>/dev/null || true)
if [[ $COMMIT_COUNT -gt 0 ]]; then
  pass "T27: log output contains 'commit <hash>' lines ($COMMIT_COUNT found)"
else
  fail "T27: log output contains 'commit <hash>' lines" "no matches | $(echo "$OUT" | head -3)"
fi

# ─── Group 6: Codegen ─────────────────────────────────────────────────────────

echo ""
echo "=== Group 6: Codegen ==="

# T28 — codegen get protobuf
OUT=$($CLI codegen get acme/platform/user.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T28: codegen get user.proto (Protobuf) succeeds"
else
  fail "T28: codegen get user.proto (Protobuf) succeeds" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T29 — codegen get FlatBuffers
OUT=$($CLI codegen get acme/platform/item.fbs 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T29: codegen get item.fbs (FlatBuffers) succeeds"
else
  fail "T29: codegen get item.fbs (FlatBuffers) succeeds" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T30 — codegen get non-existent schema fails
OUT=$($CLI codegen get acme/platform/nonexistent.proto 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T30: codegen get non-existent schema returns error"
else
  fail "T30: codegen get non-existent schema returns error" "expected non-zero exit"
fi

# ─── Group 7: Diff ────────────────────────────────────────────────────────────

echo ""
echo "=== Group 7: Diff ==="

# Setup: create diff-test branch with a new schema
$CLI branch create acme/platform diff-test >/dev/null 2>&1
$CLI schema create --project acme --repo platform --branch diff-test "$TEST_DIR/user.proto" --name diff_test.proto >/dev/null 2>&1

# T31 — diff main..diff-test
OUT=$($CLI diff acme/platform main..diff-test 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T31: diff main..diff-test succeeds"
else
  fail "T31: diff main..diff-test succeeds" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T32 — diff with schema-path filter
OUT=$($CLI diff acme/platform main..diff-test --schema-path diff_test.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T32: diff with --schema-path filter"
else
  fail "T32: diff with --schema-path filter" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T33 — diff shows added schema
OUT=$($CLI diff acme/platform main..diff-test 2>&1)
if echo "$OUT" | grep -qF "diff_test.proto"; then
  pass "T33: diff output mentions 'diff_test.proto' as new schema"
else
  fail "T33: diff output mentions 'diff_test.proto' as new schema" "$(echo "$OUT" | head -5)"
fi

# ─── Group 8: Schema Delete ───────────────────────────────────────────────────

echo ""
echo "=== Group 8: Schema Delete ==="

# T34 — delete user_v2.proto
OUT=$($CLI schema delete acme/platform/user_v2.proto --force 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T34: schema delete user_v2.proto --force"
else
  fail "T34: schema delete user_v2.proto --force" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T35 — pull deleted schema fails
OUT=$($CLI schema pull acme/platform/user_v2.proto 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T35: pull deleted schema returns error"
else
  fail "T35: pull deleted schema returns error" "expected error, got success"
fi

# ─── Group 9: Error Handling ──────────────────────────────────────────────────

echo ""
echo "=== Group 9: Error Handling ==="

# T36 — duplicate schema create fails
OUT=$($CLI schema create --project acme --repo platform "$TEST_DIR/user.proto" 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T36: duplicate schema create returns error"
else
  fail "T36: duplicate schema create returns error" "expected failure, got success"
fi

# T37 — field add to non-existent schema fails
OUT=$($CLI field add acme/platform/ghost.proto User name:string:1 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T37: field add to non-existent schema returns error"
else
  fail "T37: field add to non-existent schema returns error" "expected failure"
fi

# T38 — field add duplicate field number fails
OUT=$($CLI field add acme/platform/user.proto User dup_id:string:1 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T38: field add with duplicate field number returns error"
else
  fail "T38: field add with duplicate field number returns error" "expected failure"
fi

# T39 — duplicate branch create fails
OUT=$($CLI branch create acme/platform diff-test 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T39: duplicate branch create returns error"
else
  fail "T39: duplicate branch create returns error" "expected failure, got: $(echo "$OUT" | head -1)"
fi

# T40 — delete non-existent branch fails
OUT=$($CLI branch delete acme/platform nonexistent-branch 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T40: delete non-existent branch returns error"
else
  fail "T40: delete non-existent branch returns error" "expected failure"
fi

# T41 — log on non-existent repo fails
OUT=$($CLI log ghost/norepo 2>&1)
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T41: log on non-existent repo returns error"
else
  fail "T41: log on non-existent repo returns error" "expected failure | $(echo "$OUT" | head -1)"
fi

# ─── Group 10: Multi-repo Isolation ──────────────────────────────────────────

echo ""
echo "=== Group 10: Multi-repo Isolation ==="

# T42 — create schema in other/schemas
OUT=$($CLI schema create --project other --repo schemas "$TEST_DIR/user.proto" 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T42: schema create in second project 'other/schemas'"
else
  fail "T42: schema create in second project 'other/schemas'" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T43 — pull from other/schemas independent of acme/platform
OUT=$($CLI schema pull other/schemas/user.proto 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T43: schema pull from 'other/schemas' works independently"
else
  fail "T43: schema pull from 'other/schemas' works independently" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# T44 — branch list for second repo
OUT=$($CLI branch list other/schemas 2>&1)
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T44: branch list for 'other/schemas' works"
else
  fail "T44: branch list for 'other/schemas' works" "exit=$EXIT | $(echo "$OUT" | head -1)"
fi

# ─── Group 11: gRPC Exploration API ──────────────────────────────────────────

echo ""
echo "=== Group 11: gRPC Exploration API ==="

# T45 — ListSchemas
OUT=$(grpc_call exploration_service.proto schemahub.v1.ExplorationService/ListSchemas \
  '{"project":"acme","repo":"platform"}')
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "user.proto"; then
  pass "T45: gRPC ListSchemas returns schemas"
else
  fail "T45: gRPC ListSchemas returns schemas" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# T46 — ListDeclarations
OUT=$(grpc_call exploration_service.proto schemahub.v1.ExplorationService/ListDeclarations \
  '{"project":"acme","repo":"platform","schema_path":"user.proto"}')
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "User"; then
  pass "T46: gRPC ListDeclarations returns 'User' message"
else
  fail "T46: gRPC ListDeclarations returns 'User' message" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# T47 — Search
OUT=$(grpc_call exploration_service.proto schemahub.v1.ExplorationService/Search \
  '{"project":"acme","repo":"platform","query":"User"}')
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T47: gRPC Search for 'User' succeeds"
else
  fail "T47: gRPC Search for 'User' succeeds" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# T48 — GetDeclaration
OUT=$(grpc_call exploration_service.proto schemahub.v1.ExplorationService/GetDeclaration \
  '{"project":"acme","repo":"platform","schema_path":"user.proto","declaration_name":"User"}')
EXIT=$?
if [[ $EXIT -eq 0 ]]; then
  pass "T48: gRPC GetDeclaration for 'User' succeeds"
else
  fail "T48: gRPC GetDeclaration for 'User' succeeds" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# ─── Group 12: gRPC Project API ──────────────────────────────────────────────

echo ""
echo "=== Group 12: gRPC Project API ==="

# T49 — ListProjects
OUT=$(grpc_call project_service.proto schemahub.v1.ProjectService/ListProjects '{}')
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "acme"; then
  pass "T49: gRPC ListProjects returns 'acme'"
else
  fail "T49: gRPC ListProjects returns 'acme'" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# T50 — ListRepos
OUT=$(grpc_call project_service.proto schemahub.v1.ProjectService/ListRepos \
  '{"project":"acme"}')
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF "platform"; then
  pass "T50: gRPC ListRepos returns 'platform' in acme"
else
  fail "T50: gRPC ListRepos returns 'platform' in acme" "exit=$EXIT | $(echo "$OUT" | head -2)"
fi

# T51 — GetRepo
OUT=$(grpc_call project_service.proto schemahub.v1.ProjectService/GetRepo \
  '{"project":"acme","repo":"platform"}')
EXIT=$?
if [[ $EXIT -eq 0 ]] && echo "$OUT" | grep -qF '"defaultBranch": "main"'; then
  pass "T51: gRPC GetRepo shows defaultBranch=main"
else
  fail "T51: gRPC GetRepo shows defaultBranch=main" "exit=$EXIT | $(echo "$OUT" | head -3)"
fi

# T52 — duplicate project creation fails
OUT=$(grpc_call project_service.proto schemahub.v1.ProjectService/CreateProject \
  '{"name":"acme","is_public":true}')
EXIT=$?
if [[ $EXIT -ne 0 ]]; then
  pass "T52: duplicate project creation returns error"
else
  fail "T52: duplicate project creation returns error" "expected failure"
fi

# ─── Report ───────────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════════════════════"
echo "  Integration Test Report"
echo "════════════════════════════════════════════════════════"
echo ""
for r in "${RESULTS[@]}"; do
  printf "  %s\n" "$r"
done
echo ""
printf "  Total: %d  PASS: %d  FAIL: %d\n" "$((PASS + FAIL))" "$PASS" "$FAIL"
echo "════════════════════════════════════════════════════════"

if [[ $FAIL -gt 0 ]]; then
  exit 1
else
  exit 0
fi
