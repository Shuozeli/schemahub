# schemahub-vcs — Decisions

## jj-lib (path A): **adopted**

`schemahub-vcs` is built on the **real Jujutsu library** (`jj-lib = "0.41"`,
default features off — no `git`). We implement jj's `Backend` and `OpStore`
traits over our `ObjectDb` and drive all writes through jj's
`RepoLoader → Transaction → MutableRepo / CommitBuilder / MergedTreeBuilder`.
An earlier iteration hand-rolled a "jj-style" model over redb; that has been
replaced. The crate's public `Vcs` API (consumed by core/server/cli) is
unchanged.

### What jj-lib gives us, persisted to our DB

- **Commits / trees / files** — content-addressed (`CommitId`/`TreeId`/`FileId`)
  via jj's blake2b hashing, each commit carrying a stable `ChangeId`.
- **First-class conflicts** — a tree entry can *be* a conflicted (multi-side)
  merge, stored inline rather than rejected. We map these to `ConflictSides`.
- **Operation log** — every write is one jj `Operation` over a `View`
  (bookmarks/tags/heads); `undo` restores the parent operation's view.
- **Merge** — jj's `merge_commit_trees` / `MergedTree::merge` produce the
  per-declaration three-way merge with first-class conflicts.

### Mapping (where each piece lives)

| jj concept | schemahub impl | storage |
|---|---|---|
| `backend::Backend` | `jj_backend::DbBackend` | files/trees/commits → `ObjectDb` (`File`/`Tree`/`Commit` kinds), keyed by jj's blake2b id via `put_object_at`/`put_op_at`; proto-encoded exactly like jj's `SimpleBackend` |
| `op_store::OpStore` | `jj_op_store::DbOpStore` | operations → per-repo op-log (`put_op_at`); views → `ObjectKind::View`, keyed by jj's `ViewId`. Views/ops use a reduced serde form covering exactly the fields schemahub touches (heads, local bookmarks, local tags, wc pointers) — schemahub has no git/remotes |
| `op_heads_store::OpHeadsStore` | `jj_op_heads::DbOpHeadsStore` | the current operation-head id(s) in the `ObjectDb` ref table (`set_ref`/`get_ref`) — durable, the substrate for `undo` and reload-at-head across `Vcs` instances |
| `IndexStore` | jj's `DefaultIndexStore` | a per-`Vcs` **temp dir**. The index is a pure cache, rebuilt from the (DB-backed) op-log on load; it holds no durable schemahub state |
| `SubmoduleStore` | jj's `DefaultSubmoduleStore` | stub; schemahub has no submodules. Its path arg is ignored by jj |
| `RepoLoader`/`MutableRepo`/`Transaction` | used directly | — |

Per-declaration storage (design.md §4.2): a schema file is a jj subtree
`<schema-file>/`; each declaration is `<schema-file>/<Decl>` (a `DeclBlob`),
`<schema-file>/__meta__` is the `MetaBlob`. One jj repo per `(project, repo)`,
keyed by a `"project/repo"` prefix on the op-log/refs; content objects dedup
globally (jj's content addressing makes this inherent).

### Index / op-heads / submodule choices

- **OpHeads is DB-backed** because the head pointer is durable state that must
  survive process restarts (it anchors undo and reload-at-head). It is tiny
  (a newline-joined list of hex op ids in the per-repo ref table).
- **Index + submodule stores use jj's filesystem defaults on a per-`Vcs` scratch
  dir.** The index is reconstructable from the op-log, so a fresh temp dir per
  `Vcs` is correct and loses nothing (verified by
  `redb_state_survives_reopening_the_database`: a new `Vcs` over the same redb
  file reads back all commits/bookmarks/op-log). Reimplementing jj's
  `Index`/`ReadonlyIndex`/`MutableIndex` (ancestry, revsets, prefix resolution)
  was deliberately avoided — it is a large surface with no schemahub-specific
  requirement.

### Async bridge

jj-lib's `Backend`/`OpStore`/repo APIs are `#[async_trait]`. The `Vcs` public
API is **synchronous**. Each `Vcs` owns a **dedicated** `tokio` current-thread
runtime (`Store::block_on`) and never blocks on an ambient/shared runtime
(infra rule: dedicated runtime for workers). The runtime lives as long as the
`Vcs`.

### Shims / deviations

- **Conflict id `ObjectKind`** is retained for backward compatibility but unused
  by the backend: jj represents conflicts inline as conflicted (multi-side)
  trees, not as separate conflict objects.
- **`gc`** is implemented at the `Vcs`/`ObjectDb` level (mark-and-sweep over
  commits/trees/files/views reachable from every op's view). jj's per-backend
  `Backend::gc`/`OpStore::gc` are no-ops; op-log retention is required for undo.
- No genuine jj-lib wall was hit — every `Vcs` method is expressed through real
  jj-lib types and write paths.

## New public primitives (additive; nothing removed/renamed)

- `commit_log(project, repo, at_ref, limit) -> Vec<CommitRecord>` — a real
  commit/change-graph walk (was previously faked from the op-log).
- `commit_write_multi(...)` — touch several schema files atomically in one
  commit / one operation. The single-file `commit_write` delegates to it.
- `delete_bookmark(...)` / `delete_tag(...)`.

## Persistence seam = `ObjectDb` trait

Two impls ship: `RedbObjectDb` (embedded default) and `MemoryObjectDb` (tests /
core unit tests). The trait gained `put_object_at` / `put_op_at` (store under a
*caller-supplied* id, since jj computes its own blake2b ids) and `Symlink`/`View`
object kinds. A `postgres` impl remains future work (P3) — redb stays default.
