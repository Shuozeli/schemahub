# Code Quality Findings

Audit of `v2-rearchitecture` @ `516639f`. Baseline before fixes: `cargo build
--workspace` clean, `cargo test --workspace` = 288 passed / 0 failed / 0
ignored. `cargo build --workspace --features schemahub-vcs/postgres` clean.

Priorities:

- **P0** — correctness, security, data-loss, crash. Must fix.
- **P1** — real bug, contract drift, or rule violation. Should fix before
  release.
- **P2** — clarity / minor perf / cleanup. May defer.

---

## 1. Data integrity (P0)

### GC silently swallows load errors and then sweeps

- **Location:** `crates/schemahub-vcs/src/lib.rs:1051-1054` (op load) and
  `lib.rs:1089-1092` (commit load) in `Vcs::gc`.
- **Also:** `lib.rs:1083-1085` — invalid hex `CommitId::try_from_hex` returns
  `None` and is silently skipped.
- **Problem:** GC marks reachable objects from every op view + every commit
  ancestor, then sweeps unreachable objects. If `loader.load_operation` or
  `store.get_commit_async` returns an error for a real reason (transient I/O
  glitch, partial corruption, schema mismatch — anything other than
  "missing"), the current code does `Err(_) => continue`. The reachability
  set is then *missing* the bookmarks/tags/trees/files that op or commit
  pinned, and the subsequent sweep happily deletes them. This converts a
  recoverable read error into permanent data loss.
- **Fix:** Propagate the error (`?`) instead of `continue`. If the GC is
  asked to walk an op that genuinely vanished, that's an inconsistency that
  must be reported, not papered over by deleting more data. Also map
  `try_from_hex` failures to `VcsError::Corrupt` instead of silently
  skipping.

---

## 2. Silent failure (P1)

### `SchemaService` swallows real VCS errors when loading the base

- **Location:** `crates/schemahub-server/src/services/schema.rs:88-92`
  (`create_schema`) and `:124-129` (`update_schema`).
- **Problem:** Both handlers do
  `core.vcs().load_schema(...).unwrap_or_default()` to "tolerate a missing
  base (first write)". But `VcsError` has many variants —
  `BookmarkNotFound`, `SchemaNotFound`, `TagNotFound` are the legitimate
  "fresh bookmark" cases; `Corrupt`, `ObjectDb`, `BadRef`, `Other` are real
  errors that this code drops on the floor. When the VCS is broken, the
  handler treats the repo as empty and writes a fresh commit, potentially
  destroying real content under the bookmark.
- **Fix:** Replace `unwrap_or_default()` with the existing `load_base`
  helper from `crates/schemahub-core/src/mutation/mod.rs:19-33`, which
  matches only the NotFound variants and propagates the rest. Either reuse
  it (move it to `pub(crate)` and call from server, or add a `Core` wrapper)
  or inline the same `match` in `schema.rs`.

### `BookmarkHandler::diff` swallows `list_schemas` errors

- **Location:** `crates/schemahub-server/src/services/bookmark.rs:109-114`
  in `diff`.
- **Problem:** When the caller doesn't pin `schema_path`, the handler
  computes the union of schemas at head and base via two
  `list_schemas` calls wrapped in `if let Ok(s) = ...`. Errors from one or
  both sides become an empty union and the client receives an empty diff
  instead of a fault. The fresh-bookmark cases here are again
  `BookmarkNotFound`/`SchemaNotFound`; everything else should propagate.
- **Fix:** Match on the specific NotFound variants like `mutation::load_base`
  does, and propagate other errors.

### `BearerTokenAuthn` swallows `MetadataValue::to_str` decode errors

- **Location:** `crates/schemahub-server/src/services/mod.rs:17-23` in
  `token_from`.
- **Problem:** `metadata().get("authorization").and_then(|v| v.to_str().ok())`
  silently maps a malformed (non-ASCII) Authorization header to `None`,
  i.e. "anonymous". A malformed header is a client bug or attempted
  bypass, not a missing one — anonymous fallthrough may grant unintended
  public-read access.
- **Fix:** Return a Status::unauthenticated when the header is present but
  un-decodable.

---

## 3. API / contract drift (P1)

### `CodegenService` never returns the resolved `at_commit`

- **Location:**
  `crates/schemahub-server/src/services/codegen.rs:53`
  (`get_descriptors`) and `:73` (`preview_codegen`).
- **Problem:** Both responses leave `at_commit: String::new()`. The proto
  field exists so callers can know which commit they got; clients that
  cache by commit (the usual descriptor-cache pattern) cannot tell what
  they fetched. Effectively a stale-cache bug for downstream tooling.
- **Fix:** Resolve the ref to a commit hash via `Core::log` (limit=1, at
  the same RefSpec) and populate `at_commit`. The history service uses
  the same pattern at `bookmark.rs:39-58` (`get_commit`).

### `AdminService.get_server_config` reports a hard-coded storage backend

- **Location:** `crates/schemahub-server/src/services/admin.rs:80`.
- **Problem:** `storage_backend: "redb".to_string()` ignores the actual
  configured backend. A postgres-deployed server lies about its backend.
- **Fix:** Thread the resolved `Config` (or just the backend string) into
  `AdminHandler::new` and return the real value.

### Project + repo RPCs are echo-only

- **Location:**
  `crates/schemahub-server/src/services/project.rs:106-171`
  (`create_repo`, `get_repo`, `update_repo`, `list_repos`).
- **Problem:** `CreateRepo` / `GetRepo` / `UpdateRepo` just echo the
  request body. `ListRepos` returns `repos: vec![]` for every project.
  They don't:
  1. authorize (no `authorize_repo_action` call),
  2. validate the project exists,
  3. persist anything,
  4. surface a real repo list.
  This is documented in the module header ("a `(project, repo)` springs
  into existence on the first write"). Acceptable design-wise, but the
  RPCs MUST at minimum authorize the project access and validate it
  exists, otherwise an unauthenticated caller learns about private
  projects by name and gets RPC-success affirmations.
- **Fix:** Add `authorize_repo_action(Read | ManageRepo, …)` gates on
  every repo RPC; reject when the project doesn't exist. `ListRepos`
  cannot enumerate without a repo registry — return `unimplemented`
  rather than empty success to make the gap visible to clients.

### CLI commands other than `project` don't send the bearer token

- **Location:** `crates/schemahub-cli/src/main.rs:75-119` —
  `Repo`/`Schema`/`Field`/`Branch`/`Tag`/`Log`/`Op`/`Undo`/`Resolve`/
  `Codegen`/`Diff` build their `RefServiceClient` etc. directly from
  `channel`, with no `bearer()` wrapping. Only
  `cmd/project.rs:154-163` (`bearer`) attaches the
  `Authorization: Bearer …` metadata, and only `project::run` is
  passed `&cfg.token`.
- **Problem:** With an authenticated server, every non-`project` CLI
  invocation is anonymous; writes are denied with permission errors
  even though `--token`/`SCHEMAHUB_TOKEN` was set. Even reads on
  private projects fail. This is a real user-facing functional break.
- **Fix:** Hoist `bearer` from `cmd/project.rs` into `cmd/mod.rs` (or
  a small `auth.rs`) and pass `&cfg.token` to every `run` function;
  wrap each request with `bearer(body, token)?` before calling the
  client. Update `main.rs` to forward the token to every match arm.

### CLI `config.token` warns it's used while marked dead code

- **Location:** `crates/schemahub-cli/src/config.rs:17-21`.
- **Problem:** `Config.token` is marked `#[allow(dead_code)]` — a
  bypass of the user's "no `#[allow(dead_code)]`" rule. The previous
  finding (CLI commands ignoring the token) is the direct cause: once
  every command attaches the token, the field is used and the
  `#[allow]` goes away.
- **Fix:** Remove `#[allow(dead_code)]` after the CLI bearer wiring
  fix lands — the field is then load-bearing in every command.

### CLI silently defaults on a bad config file

- **Location:** `crates/schemahub-cli/src/config.rs:52-57`,
  `:47-51`.
- **Problem:** `load_raw_config` swallows `read_to_string` errors and
  TOML parse errors via `unwrap_or_default()` — a typo in
  `~/.schemahub/config` becomes "no config", which is then a silent
  misconfig. User rules say "fail-fast over fail-safe".
- **Fix:** Make `load_raw_config` return `anyhow::Result<RawConfig>`
  and bubble parse errors out; only treat "file missing" as default.

---

## 4. Auth / audit (P1)

### Mutation handlers ignore the authenticated identity for audit author

- **Location:**
  `crates/schemahub-server/src/services/schema.rs:25,104,141,185,209`
  (`DEFAULT_AUTHOR = "schemahub"` always passed to `commit_write`).
  Same in
  `crates/schemahub-server/src/services/bookmark.rs:22,152,243,258,279,327`
  and
  `crates/schemahub-server/src/services/history.rs:17,101-105,166-170`
  (the latter two only fall back to `DEFAULT_AUTHOR` when the client
  doesn't supply `r.author`).
- **Problem:** The authenticated identity from the bearer token is
  resolved (`token_from`) and used for *authz*, but the *commit
  author* / *audit author* sent to the VCS is the hard-coded
  `"schemahub"` (or, worse, a client-supplied `r.author` in
  `undo`/`resolve_conflict` which the server takes on faith). This
  breaks the audit trail: every commit in the op-log appears
  attributable to `"schemahub"` regardless of who actually authenticated.
- **Fix:** Use `Core::resolve_identity(token)` (already exists, see
  `projects.rs:231`) and pass its `id()` (falling back to
  `DEFAULT_AUTHOR` only for `Identity::Anonymous`) as the commit
  author. Drop the client-supplied `r.author` paths entirely — they
  let any authenticated caller forge an arbitrary audit string.

### VCS ref ops silently drop the `author` parameter

- **Location:**
  `crates/schemahub-vcs/src/lib.rs:562` (`create_bookmark`), `:585`
  (`move_bookmark`), `:606` (`delete_bookmark`), `:641` (`create_tag`),
  `:662` (`delete_tag`), `:824` (`undo`). Each one has
  `let _ = author;` and uses jj's default metadata for the op-log
  record.
- **Problem:** Callers think they're attaching an author to the
  operation; the VCS quietly throws it away. The op-log audit trail
  is incomplete for every ref change.
- **Fix:** Pipe `author` into the jj transaction's operation
  metadata. The pattern used in `commit_write` already builds an
  `author_signature(author)` — for ref-only ops, set
  `tx.repo_mut().set_user_metadata` (or whatever the jj-lib API for
  per-op username is) so `OpRecord::author` reflects the requested
  identity.

---

## 5. Lifecycle / consistency (P1)

### `create_project` is not atomic between project-store and role-store

- **Location:** `crates/schemahub-core/src/projects.rs:43-60`.
- **Problem:** Sequence is:
  1. check `project_store.get(name).is_none()`,
  2. `project_store.set(meta)`,
  3. `role_store.set(name, creator, Owner)`.
  Step 1→2 is a TOCTOU race: two concurrent `create_project` calls
  for the same name both pass the existence check, then one wins.
  Step 2 vs step 3 is non-atomic: if `role_store.set` fails after a
  successful `project_store.set`, the project exists with zero
  Owners — the very invariant `guard_last_owner` later enforces.
- **Fix:** Either:
  (a) wrap both store writes in a single "transactional" path on the
  store traits (add `ProjectStore::create_with_owner` that does both
  atomically — easy for the in-memory and file backends if both
  trait impls are in the same struct), or
  (b) accept the file-backend race window but compensate: on
  `role_store.set` failure after `project_store.set`, immediately
  call `project_store.delete(name)` (which means adding `delete` to
  the trait) so we don't leave a half-created project. Document the
  TOCTOU explicitly if accepted.

### `EmptyProjectStore` / `EmptyRoleStore` silently swallow writes in production

- **Location:** `crates/schemahub-core/src/lib.rs:184-214`. Used in
  `Core::with_config` (`:100-116`) — the production path when
  `config.auth_enabled()` is false.
- **Problem:** The default-deployment path (no `[auth]` configured)
  installs Empty stores whose `set` returns `Ok(())` silently. A user
  who hits `CreateProject` on such a server sees success; subsequent
  `GetProject` returns "not found". Silent data loss for the
  noop-auth getting-started path.
- **Fix:** Either:
  (a) reject project mutation RPCs when running with EmptyStores
  ("project management requires `[auth]`"), or
  (b) install File-backed stores even when `[auth]` is absent (the
  registry doesn't *need* a token table to function — only authz
  decisions do), or
  (c) make the Empty stores' `set` return an `io::Error` so the
  failure is visible. (a) is the smallest behavior change; (b) is
  the most useful.

### `IdempotencyStore` grows unbounded

- **Location:**
  `crates/schemahub-core/src/mutation/idempotency.rs:17-40`.
- **Problem:** Every unique idempotency key the server has ever seen
  sticks in the in-process `HashMap` forever, with a full
  `MutationResponse` value (commit id + change id + conflict list).
  A long-running server eventually OOMs; an unauthenticated client
  on a public-read deployment cannot reach this, but any authorized
  writer can.
- **Fix:** Add a TTL / max-size bound to the store. Either: (a)
  bounded LRU (e.g. `lru` crate; or hand-rolled) capped at N entries,
  or (b) drop entries older than a configurable retention. The
  config already has a `idempotency_ttl_hours` field
  (`admin.rs:78`) — wire it.

---

## 6. std-trait drift (P1)

### `from_str` methods on `HttpMethod` / `ParameterLocation` / `JsonSchemaType` should be `std::str::FromStr`

- **Location:**
  `crates/schemahub-compiler-openapi/src/ast.rs:31`,
  `:69`,
  `:102`.
- **Problem:** Each enum has an inherent `fn from_str(&str) -> Option<Self>`.
  Clippy flags `method_from_str` because the signature *almost*
  matches the std trait but isn't it — callers can't use the
  `"x".parse::<HttpMethod>()` idiom and can't write generic code that
  expects `FromStr`. User rule: "implement std traits where signatures
  match".
- **Fix:** Replace each with `impl FromStr for X { type Err =
  ParseError; … }` (or `()` if we just want `Option`-ish behavior via
  `.parse().ok()`). Update the one direct caller
  (`parser.rs:201` — `HttpMethod::from_str(method_str).unwrap()`)
  to `.parse()?` or `.parse().expect(…)` once the trait is in scope.

---

## 7. Defensive coding (P1)

### `tree_from_proto` / `tree_value_from_proto` unwrap on optional proto fields

- **Location:** `crates/schemahub-vcs/src/jj_backend.rs:280`, `:281`,
  `:317`.
- **Problem:** Three `.unwrap()` calls on `proto.value` /
  `proto_entry.value` inside async backend code that handles
  untrusted bytes read from the ObjectDb. Protobuf message fields
  are always optional on the wire; a malformed or partially written
  tree blob can legitimately have `value = None`, and the unwrap
  becomes a panic inside a tonic worker (server crash). Same for
  `RepoPathComponentBuf::new(...).unwrap()` on a bad name byte.
- **Fix:** Return `BackendResult` / propagate
  `BackendError::Other("malformed tree value …")`. Match the existing
  `to_other_err` helper at line 51.

### Op-heads store silently drops invalid hex ids

- **Location:** `crates/schemahub-vcs/src/jj_op_heads.rs:49`
  (`filter_map(|hex| OperationId::try_from_hex(hex))`).
- **Problem:** If the op-heads ref blob is corrupted (e.g. partial
  write), one bad line silently disappears and the head set
  shrinks. `update_op_heads` then writes back a smaller set —
  permanent loss of an op-graph head.
- **Fix:** Return `OpHeadsStoreError::Read` on the first bad hex line
  so the corruption is reported to the caller, not papered over.

---

## 8. Style / clippy fixes (P2)

### Stale `debug_assert!` over a constant

- **Location:** `crates/schemahub-server/src/wire.rs:209`
  (`debug_assert!(tag::ADD_FIELD > 0)`).
- **Problem:** Always-true assertion on a compile-time constant.
  Clippy: `assertions_on_constants`. Pure noise.
- **Fix:** Delete (or convert to `const { assert!(...) }` if you want
  to keep the intent).

### Unused-import sort closure

- **Location:** `crates/schemahub-compiler-protobuf/src/printer.rs:114`,
  `crates/schemahub-compiler-flatbuffers/src/parse.rs:62`.
- **Problem:** `.sort_by(|a, b| a.key.cmp(&b.key))` could be
  `.sort_by_key(|x| x.key.clone())` or `.sort_by(|a, b| a.key.cmp(...))`
  cleanup per clippy.
- **Fix:** Apply `sort_by_key`.

### `for add in value.adds() { if let Some(v) = add { … } }`

- **Location:** `crates/schemahub-vcs/src/lib.rs:905-911`.
- **Problem:** Clippy: `unnecessary_filter_map` — the outer iter
  yields `Option<…>`; the manual `if let Some` is equivalent to
  `.flatten()` or `.filter_map(...)`.
- **Fix:** `value.adds().flatten()` then handle in the closure.

### Redundant closure

- **Location:** `crates/schemahub-vcs/src/jj_op_heads.rs:49`.
- **Problem:** `|hex| OperationId::try_from_hex(hex)` — clippy
  suggests `OperationId::try_from_hex` directly.
- **Fix:** Pass the function value. (Note: if we change to fail-fast
  per finding 7, this whole closure changes anyway.)

### Dead-code "kept for future use" markers

- **Location:**
  `crates/schemahub-core/src/mutation/closure.rs:77`
  (`pub(crate) fn require_resolved`),
  `crates/schemahub-vcs/src/jj_op_store.rs:335`
  (`type _ReachableViews = HashSet<ViewId>`).
- **Problem:** User rule: "no `#[allow(dead_code)]`". Both are
  speculative scaffolding with no caller.
- **Fix:** Delete the dead items; re-add them with a real caller
  when needed.

### `Result::Err` is "very large" warnings on wire conversions

- **Location:** clippy hits at
  `crates/schemahub-server/src/services/project.rs:271`,
  `crates/schemahub-server/src/services/schema.rs:45`,
  `crates/schemahub-server/src/wire.rs:76,104,223,297,349,366`.
- **Problem:** `tonic::Status` is ~176 bytes; returning it by value
  bloats every Result's discriminant. Cosmetic; tonic uses Status
  by value throughout, so following clippy here would diverge from
  tonic's idiom.
- **Fix:** Allowed convention — skip. Documented here so future
  reviewers can see this was considered.

### `pg_db.rs` spawns a fresh OS thread per DB call

- **Location:**
  `crates/schemahub-vcs/src/pg_db.rs:135-153` (`block_on`).
- **Problem:** Every ObjectDb method call hops via
  `thread::spawn` + `oneshot` to escape the caller's tokio context
  and reach the dedicated runtime. At schema-registry QPS (low,
  documented), fine. Worth flagging.
- **Fix:** Not actionable in this pass. Documented as a known cost.

### `Core::op_log` loads the full op-log to return the last N

- **Location:**
  `crates/schemahub-core/src/history.rs:38-44`.
- **Problem:** `vcs.list_operations` returns oldest→newest, then we
  `drain(..ops.len() - n)`. For a repo with thousands of ops, every
  paged op-log read is O(total).
- **Fix:** Add `Vcs::list_operations_tail(n)` that walks the op
  parent chain back N steps. Defer unless someone reports it.

### Config silently skips malformed `[repos.*]` keys

- **Location:** `crates/schemahub-server/src/config.rs:272`.
- **Problem:** `let Some((project, repo)) = key.split_once('/') else {
  continue; };` — a typo'd key disappears at startup. User rules:
  "fail-fast".
- **Fix:** Emit a startup error: "[repos.{key}] is not 'project/repo'".

### OpenAPI parser silently drops non-string keys / unknown JSON-schema types

- **Location:** `crates/schemahub-compiler-openapi/src/parser.rs:73,
  86, 100, 113, 126, 417, 420`.
- **Problem:** `as_str().unwrap_or_default()` and
  `JsonSchemaType::from_str(...).unwrap_or_default()` quietly turn
  malformed input into empty strings / no types. P2 because OpenAPI
  3.x requires these as strings — typed YAML loaders rarely produce
  the broken form — but still hides real input bugs.
- **Fix:** Return `ParseError::Other(...)` for non-string keys and
  unknown type strings.

---

## Test gaps (P1)

- No e2e tests for `ProjectService.{create_repo, get_repo,
  update_repo, list_repos}` — see finding §3 ("echo-only repo
  RPCs"). Status quo is "they always succeed regardless of project
  state" and no test asserts that. Add one
  e2e test per RPC that exercises the (future) authorize path.
- No tests for `AdminService.get_server_config` against a
  configured backend; the hardcoded "redb" bug is undetected.
- No tests for CLI bearer forwarding — the only command that
  forwards is `project`, and only its happy path is exercised
  through unit tests in `cmd/project.rs`. An e2e test that hits a
  RBAC-enabled server with `schemahub schema list …` would catch
  the missing-bearer regression.

---

## Known issues (noted, not fixed)

- **`Mutex.lock().unwrap()` throughout `auth_files.rs`,
  `auth_impls.rs` test stubs, and `idempotency.rs`** — standard
  Rust idiom for poisoned-lock panic. Not a real concern.
- **`tonic::Status` is large** (see §8) — tonic-idiomatic, leave
  as-is.
- **`PgObjectDb` thread-per-call** (see §8) — design-acknowledged.
- **`flatbuffers/blob.rs:287` "panic in test"** — actually
  inside a `#[test]`. Fine.
- **Author field unused on `commit_write` calls that pass
  `DEFAULT_AUTHOR`** — this is a *consequence* of finding §4
  ("server hardcodes author"); fixing that finding fixes this
  implicitly. No separate fix.
