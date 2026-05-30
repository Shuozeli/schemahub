//! `schemahub-vcs` — the format-agnostic version-control layer
//! (crate-structure.md §3.2), built on the **real Jujutsu library** (`jj-lib`).
//!
//! This crate implements jj-lib's [`Backend`](jj_lib::backend::Backend)
//! ([`DbBackend`](jj_backend::DbBackend)) and
//! [`OpStore`](jj_lib::op_store::OpStore) ([`DbOpStore`](jj_op_store::DbOpStore))
//! over a swappable [`ObjectDb`] (redb default, in-memory for tests), and drives
//! all writes through jj's `RepoLoader` → `Transaction` → `MutableRepo` /
//! `CommitBuilder` / `MergedTreeBuilder`. Commits get stable jj `ChangeId`s,
//! merges produce jj first-class conflicts, and every write is one jj
//! `Operation` in the op-log (the substrate for `undo`). See `DECISIONS.md`.
//!
//! Per-declaration storage (design.md §4.2): a schema file is a jj subtree
//! `<schema-file>/`; each declaration is a file entry `<schema-file>/<Decl>`
//! holding the `DeclBlob` bytes, and `<schema-file>/__meta__` holds the
//! `MetaBlob`. One jj repo per `(project, repo)`; objects dedup globally; the
//! op-log and refs are partitioned per repo.

pub mod bookmark;
pub mod jj_backend;
pub mod jj_op_heads;
pub mod jj_op_store;
pub mod memory_db;
pub mod object_db;
#[cfg(feature = "postgres")]
pub mod pg_db;
pub mod redb_db;
pub mod repo;

use std::sync::Arc;

pub use memory_db::MemoryObjectDb;
pub use object_db::{
    ObjectDb, ObjectDbError, ObjectDbResult, ObjectId, ObjectKind, OpId,
};
#[cfg(feature = "postgres")]
pub use pg_db::PgObjectDb;
pub use redb_db::RedbObjectDb;

use jj_lib::backend::{CommitId, CopyId, FileId, Signature, Timestamp, TreeValue};
use jj_lib::merge::Merge;
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_store::RefTarget;
use jj_lib::ref_name::RefName;
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::repo_path::RepoPathBuf;
use schemahub_types::{ConflictSides, DeclBlob, MetaBlob, MutationEffect, SchemaObjects};
use thiserror::Error;

use crate::repo::Store;

/// `__meta__` entry name within a schema-file subtree.
const META_NAME: &str = "__meta__";

/// Operation-metadata attribute key under which the schemahub-resolved audit
/// author (the authenticated identity for the request, set by the server's
/// `resolve_author`) is stored on every op-log entry. Read back by
/// [`Vcs::list_operations`] and exposed in [`OpRecord::author`].
pub(crate) const AUTHOR_ATTRIBUTE: &str = "schemahub.author";

#[derive(Debug, Error)]
pub enum VcsError {
    #[error("object store error: {0}")]
    ObjectDb(object_db::ObjectDbError),
    #[error("object not found")]
    ObjectNotFound,
    #[error("declaration not found: {0}")]
    DeclNotFound(String),
    #[error("schema file not found: {0}")]
    SchemaNotFound(String),
    #[error("bookmark not found: {0}")]
    BookmarkNotFound(String),
    #[error("bookmark already exists: {0}")]
    BookmarkExists(String),
    #[error("tag not found: {0}")]
    TagNotFound(String),
    #[error("ref does not resolve to a commit: {0}")]
    BadRef(String),
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("declaration {decl} is not conflicted")]
    NotConflicted { decl: String },
    #[error("corrupt repository data: {0}")]
    Corrupt(String),
    #[error("vcs error: {0}")]
    Other(String),
}

pub type VcsResult<T> = Result<T, VcsError>;

/// The full ref namespace for resolving reads (`at_ref` in the read API): a
/// bookmark name, a tag name, or a raw commit id (hex).
#[derive(Clone, Debug)]
pub enum RefSpec {
    Bookmark(String),
    Tag(String),
    Commit(String),
}

impl RefSpec {
    pub fn bookmark(name: impl Into<String>) -> Self {
        RefSpec::Bookmark(name.into())
    }
    pub fn commit(id: impl Into<String>) -> Self {
        RefSpec::Commit(id.into())
    }
}

/// The result of a write: the new commit and its stable change id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub commit_id: String,
    pub change_id: String,
    /// Declarations that landed conflicted (same-decl concurrent edits), as
    /// `<schema_path>/<decl>` paths or bare decl names within the touched file.
    pub conflicted_decls: Vec<String>,
}

/// A summarized operation-log entry returned to callers (the audit record).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpRecord {
    pub op_id: String,
    pub parents: Vec<String>,
    pub description: String,
    pub author: String,
    pub timestamp: String,
}

/// A commit/change graph node returned by [`Vcs::commit_log`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    pub commit_id: String,
    pub change_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub message: String,
    pub timestamp: String,
}

/// The schemahub-shaped VCS handle — the contract `schemahub-core` consumes.
///
/// All methods are synchronous; the jj async backend is bridged via a dedicated
/// runtime owned by the inner [`Store`] (see `repo.rs`). A `(project, repo)`
/// pair scopes the op-log and bookmarks; content objects dedup globally.
pub struct Vcs {
    store: Store,
}

impl Vcs {
    /// Construct the VCS over a concrete object store.
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self {
            store: Store::new(db),
        }
    }

    /// The per-repo key used to scope the op-log and refs.
    fn repo_key(project: &str, repo: &str) -> String {
        format!("{project}/{repo}")
    }

    // ── Ref resolution ────────────────────────────────────────────────────────

    /// Resolve a [`RefSpec`] to a [`CommitId`] against the repo's current view.
    fn resolve_ref(&self, repo: &Arc<ReadonlyRepo>, at: &RefSpec) -> VcsResult<CommitId> {
        let view = repo.view();
        match at {
            RefSpec::Bookmark(name) => {
                let target = view.get_local_bookmark(RefName::new(name));
                target
                    .added_ids()
                    .next()
                    .cloned()
                    .ok_or_else(|| VcsError::BookmarkNotFound(name.clone()))
            }
            RefSpec::Tag(name) => {
                let target = view.get_local_tag(RefName::new(name));
                target
                    .added_ids()
                    .next()
                    .cloned()
                    .ok_or_else(|| VcsError::TagNotFound(name.clone()))
            }
            RefSpec::Commit(id_hex) => CommitId::try_from_hex(id_hex)
                .ok_or_else(|| VcsError::BadRef(id_hex.clone())),
        }
    }

    /// Resolve a ref for a write, tolerating a missing bookmark (returns None so
    /// a fresh bookmark can be created).
    fn try_resolve(&self, repo: &Arc<ReadonlyRepo>, at: &RefSpec) -> VcsResult<Option<CommitId>> {
        match self.resolve_ref(repo, at) {
            Ok(id) => Ok(Some(id)),
            Err(VcsError::BookmarkNotFound(_)) | Err(VcsError::TagNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn map_jj<T, E: std::fmt::Display>(r: Result<T, E>) -> VcsResult<T> {
        r.map_err(|e| VcsError::Other(e.to_string()))
    }

    // ── Reads ─────────────────────────────────────────────────────────────────

    /// Reassemble a schema file's objects (meta + clean decls) at a ref.
    /// Conflicted declarations are omitted; inspect them via [`Vcs::read_conflict`].
    pub fn load_schema(
        &self,
        project: &str,
        repo: &str,
        schema_path: &str,
        at_ref: &RefSpec,
    ) -> VcsResult<SchemaObjects> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&jj_repo, at_ref)?;
        let commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&commit_id)))?;
        let tree = commit.tree();

        // Enumerate the schema subtree's direct entries.
        let prefix = format!("{schema_path}/");
        let mut found_schema = false;
        let mut meta = MetaBlob::default();
        let mut decls = std::collections::BTreeMap::new();
        for (path, value) in tree.entries() {
            let value = Self::map_jj(value)?;
            let internal = path.as_internal_file_string();
            let Some(rest) = internal.strip_prefix(&prefix) else {
                continue;
            };
            if rest.contains('/') {
                continue; // not a direct child of the schema subtree
            }
            // `rest` is the single encoded decl-name component; decode it back to
            // the original name (which itself may contain `/`).
            let name = decode_decl_name(rest);
            found_schema = true;
            // Conflicted entries are surfaced via read_conflict, not here.
            let Ok(Some(TreeValue::File { id, .. })) = value.into_resolved() else {
                continue;
            };
            let bytes = self.read_file(&jj_repo, &id)?;
            if name == META_NAME {
                meta = MetaBlob::new(bytes);
            } else {
                decls.insert(name, DeclBlob::new(bytes));
            }
        }
        if !found_schema {
            return Err(VcsError::SchemaNotFound(schema_path.to_string()));
        }
        Ok(SchemaObjects { meta, decls })
    }

    /// List schema-file names present at a ref.
    pub fn list_schemas(
        &self,
        project: &str,
        repo: &str,
        at_ref: &RefSpec,
    ) -> VcsResult<Vec<String>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&jj_repo, at_ref)?;
        let commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&commit_id)))?;
        let tree = commit.tree();
        let mut schemas = std::collections::BTreeSet::new();
        for (path, value) in tree.entries() {
            Self::map_jj(value)?;
            let internal = path.as_internal_file_string();
            if let Some((schema, _rest)) = internal.split_once('/') {
                schemas.insert(schema.to_string());
            }
        }
        Ok(schemas.into_iter().collect())
    }

    /// List declaration names (excluding `__meta__`) in a schema file at a ref.
    pub fn list_declarations(
        &self,
        project: &str,
        repo: &str,
        schema_path: &str,
        at_ref: &RefSpec,
    ) -> VcsResult<Vec<String>> {
        let objs = self.load_schema(project, repo, schema_path, at_ref)?;
        Ok(objs.decls.keys().cloned().collect())
    }

    /// Fetch one declaration's blob at a ref. Errors if the declaration is
    /// conflicted (use [`Vcs::read_conflict`]).
    pub fn get_declaration(
        &self,
        project: &str,
        repo: &str,
        schema_path: &str,
        decl: &str,
        at_ref: &RefSpec,
    ) -> VcsResult<DeclBlob> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&jj_repo, at_ref)?;
        let commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&commit_id)))?;
        let tree = commit.tree();
        let path = decl_path(schema_path, decl)?;
        let value = Self::map_jj(self.store.block_on(tree.path_value(&path)))?;
        match value.into_resolved() {
            Ok(Some(TreeValue::File { id, .. })) => {
                Ok(DeclBlob::new(self.read_file(&jj_repo, &id)?))
            }
            Ok(Some(_)) => Err(VcsError::Corrupt(format!("{schema_path}/{decl} is not a file"))),
            Ok(None) => Err(VcsError::DeclNotFound(decl.to_string())),
            Err(_) => Err(VcsError::NotConflicted {
                decl: format!("{decl} is conflicted; use read_conflict"),
            }),
        }
    }

    /// Read a file blob by jj FileId.
    fn read_file(&self, repo: &Arc<ReadonlyRepo>, id: &FileId) -> VcsResult<Vec<u8>> {
        use tokio::io::AsyncReadExt as _;
        let bytes = self.store.block_on(async {
            let mut reader = repo
                .store()
                .read_file(jj_lib::repo_path::RepoPath::root(), id)
                .await?;
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).await.map_err(|e| {
                jj_lib::backend::BackendError::Other(Box::new(e))
            })?;
            Ok::<_, jj_lib::backend::BackendError>(buf)
        });
        Self::map_jj(bytes)
    }

    /// Write a file blob, returning its jj FileId.
    fn write_file(&self, repo: &Arc<ReadonlyRepo>, bytes: &[u8]) -> VcsResult<FileId> {
        let id = self.store.block_on(async {
            let mut cursor = std::io::Cursor::new(bytes);
            repo.store()
                .write_file(jj_lib::repo_path::RepoPath::root(), &mut cursor)
                .await
        });
        Self::map_jj(id)
    }

    // ── Writes ────────────────────────────────────────────────────────────────

    /// Apply a [`MutationEffect`] to one `schema_path` under a new commit, move
    /// the `bookmark`, and record exactly one operation (design.md §5.1).
    #[allow(clippy::too_many_arguments)]
    pub fn commit_write(
        &self,
        project: &str,
        repo: &str,
        bookmark: &str,
        schema_path: &str,
        base_ref: &RefSpec,
        effect: MutationEffect,
        author: &str,
        message: &str,
    ) -> VcsResult<WriteResult> {
        self.commit_write_multi(
            project,
            repo,
            bookmark,
            base_ref,
            vec![(schema_path.to_string(), effect)],
            author,
            message,
        )
    }

    /// Like [`Vcs::commit_write`] but touches several schema files atomically in
    /// one commit / one operation.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_write_multi(
        &self,
        project: &str,
        repo: &str,
        bookmark: &str,
        base_ref: &RefSpec,
        effects: Vec<(String, MutationEffect)>,
        author: &str,
        message: &str,
    ) -> VcsResult<WriteResult> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;

        let base_id = self.try_resolve(&jj_repo, base_ref)?;
        let current_tip = self
            .resolve_ref(&jj_repo, &RefSpec::bookmark(bookmark))
            .ok();

        // 1. Build the writer's tree from `base` + the effects.
        let base_commit = match &base_id {
            Some(id) => Some(Self::map_jj(
                self.store.block_on(jj_repo.store().get_commit_async(id)),
            )?),
            None => None,
        };
        let base_tree = match &base_commit {
            Some(c) => c.tree(),
            None => jj_repo.store().empty_merged_tree(),
        };
        let mut builder = jj_lib::merged_tree_builder::MergedTreeBuilder::new(base_tree);
        for (schema_path, effect) in &effects {
            self.apply_effect(&jj_repo, &mut builder, schema_path, effect)?;
        }
        let writer_tree = Self::map_jj(self.store.block_on(builder.write_tree()))?;

        // 2. Determine parents: the current bookmark tip (if any). When the tip
        // moved under the writer relative to `base`, create a merge commit so jj
        // produces first-class conflicts at declaration granularity.
        let signature = author_signature(author);
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);

        let (final_tree, parents) = match &current_tip {
            Some(tip) if Some(tip) == base_id.as_ref() => (writer_tree, vec![tip.clone()]),
            Some(tip) => {
                // Merge the writer's tree with the tip's tree over their base.
                let tip_commit = Self::map_jj(
                    self.store.block_on(jj_repo.store().get_commit_async(tip)),
                )?;
                let merged = self.merge_trees(
                    &jj_repo,
                    base_commit.as_ref(),
                    &tip_commit.tree(),
                    &writer_tree,
                )?;
                (merged, vec![tip.clone()])
            }
            None => {
                let parents = base_id.iter().cloned().collect::<Vec<_>>();
                let parents = if parents.is_empty() {
                    vec![jj_repo.store().root_commit_id().clone()]
                } else {
                    parents
                };
                (writer_tree, parents)
            }
        };

        let conflicted = self.conflicted_decls(&final_tree)?;

        let commit = Self::map_jj(self.store.block_on(
            tx.repo_mut()
                .new_commit(parents, final_tree)
                .set_author(signature.clone())
                .set_committer(signature)
                .set_description(message)
                .write(),
        ))?;

        // 3. Move bookmark + heads.
        self.set_bookmark_in_tx(&mut tx, bookmark, commit.id().clone());
        Self::map_jj(self.store.block_on(tx.repo_mut().add_head(&commit)))?;
        self.commit_tx(tx, &format!("commit_write {bookmark}: {message}"))?;

        Ok(WriteResult {
            commit_id: commit.id().hex(),
            change_id: commit.change_id().reverse_hex(),
            conflicted_decls: conflicted,
        })
    }

    /// Apply one effect's meta/upserts/removes into the merged-tree builder.
    fn apply_effect(
        &self,
        repo: &Arc<ReadonlyRepo>,
        builder: &mut jj_lib::merged_tree_builder::MergedTreeBuilder,
        schema_path: &str,
        effect: &MutationEffect,
    ) -> VcsResult<()> {
        if let Some(meta) = &effect.meta {
            let id = self.write_file(repo, meta.as_bytes())?;
            builder.set_or_remove(decl_path(schema_path, META_NAME)?, resolved_file(id));
        }
        for (name, blob) in &effect.upserts {
            let id = self.write_file(repo, blob.as_bytes())?;
            builder.set_or_remove(decl_path(schema_path, name)?, resolved_file(id));
        }
        for name in &effect.removes {
            builder.set_or_remove(decl_path(schema_path, name)?, Merge::absent());
        }
        Ok(())
    }

    /// Three-way merge `ours` and `theirs` over `base` using jj's tree merge,
    /// yielding a (possibly conflicted) [`MergedTree`].
    fn merge_trees(
        &self,
        repo: &Arc<ReadonlyRepo>,
        base: Option<&jj_lib::commit::Commit>,
        theirs: &jj_lib::merged_tree::MergedTree,
        ours: &jj_lib::merged_tree::MergedTree,
    ) -> VcsResult<jj_lib::merged_tree::MergedTree> {
        let base_tree = match base {
            Some(c) => c.tree(),
            None => repo.store().empty_merged_tree(),
        };
        // Three-way merge: adds = [ours, theirs], removes = [base], expressed as
        // the alternating term vec [ours, base, theirs]. Labels are unused.
        let merge = Merge::from_vec(vec![
            (ours.clone(), String::new()),
            (base_tree, String::new()),
            (theirs.clone(), String::new()),
        ]);
        let merged = self.store.block_on(jj_lib::merged_tree::MergedTree::merge(merge));
        Self::map_jj(merged)
    }

    /// The conflicted declaration names in a tree. Each conflict is a
    /// `<schema>/<decl>` path; we return the bare `<decl>` basename (matching the
    /// `commit_write` contract — the touched schema file is known to the caller).
    fn conflicted_decls(
        &self,
        tree: &jj_lib::merged_tree::MergedTree,
    ) -> VcsResult<Vec<String>> {
        let mut out = Vec::new();
        for (path, value) in tree.conflicts() {
            Self::map_jj(value)?; // surface read errors
            let internal = path.as_internal_file_string();
            // The basename is the single encoded decl-name component; decode it.
            let component = internal.rsplit('/').next().unwrap_or(internal);
            out.push(decode_decl_name(component));
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn set_bookmark_in_tx(
        &self,
        tx: &mut jj_lib::transaction::Transaction,
        bookmark: &str,
        commit: CommitId,
    ) {
        tx.repo_mut()
            .set_local_bookmark_target(RefName::new(bookmark), RefTarget::normal(commit));
    }

    fn commit_tx(
        &self,
        tx: jj_lib::transaction::Transaction,
        description: &str,
    ) -> VcsResult<()> {
        Self::map_jj(self.store.block_on(tx.commit(description.to_string()))).map(|_| ())
    }

    /// Stamp the schemahub-resolved audit author onto a transaction's op-log
    /// metadata. jj's `UserSettings::operation_username` would otherwise
    /// supply a stable but anonymous \"jj\" / hostname value; the
    /// `AUTHOR_ATTRIBUTE` is preferred by [`Vcs::list_operations`] over
    /// jj's default so the op-log records *who* drove the change.
    fn record_author(tx: &mut jj_lib::transaction::Transaction, author: &str) {
        if !author.is_empty() {
            tx.set_attribute(AUTHOR_ATTRIBUTE.to_string(), author.to_string());
        }
    }

    // ── Bookmarks & tags ──────────────────────────────────────────────────────

    /// Create a new bookmark pointing at the commit `from` resolves to.
    pub fn create_bookmark(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        from: &RefSpec,
        author: &str,
    ) -> VcsResult<String> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo.view().get_local_bookmark(RefName::new(name)).is_present() {
            return Err(VcsError::BookmarkExists(name.to_string()));
        }
        let commit = self.resolve_ref(&jj_repo, from)?;
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_bookmark_target(RefName::new(name), RefTarget::normal(commit.clone()));
        self.commit_tx(tx, &format!("create_bookmark {name}"))?;
        Ok(commit.hex())
    }

    /// Move an existing bookmark to the commit `to` resolves to.
    pub fn move_bookmark(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        to: &RefSpec,
        author: &str,
    ) -> VcsResult<String> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo.view().get_local_bookmark(RefName::new(name)).is_absent() {
            return Err(VcsError::BookmarkNotFound(name.to_string()));
        }
        let commit = self.resolve_ref(&jj_repo, to)?;
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_bookmark_target(RefName::new(name), RefTarget::normal(commit.clone()));
        self.commit_tx(tx, &format!("move_bookmark {name}"))?;
        Ok(commit.hex())
    }

    /// Delete a bookmark.
    pub fn delete_bookmark(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        author: &str,
    ) -> VcsResult<()> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo.view().get_local_bookmark(RefName::new(name)).is_absent() {
            return Err(VcsError::BookmarkNotFound(name.to_string()));
        }
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_bookmark_target(RefName::new(name), RefTarget::absent());
        self.commit_tx(tx, &format!("delete_bookmark {name}"))
    }

    /// List bookmarks (name → target commit ids) at the current view.
    pub fn list_bookmarks(&self, project: &str, repo: &str) -> VcsResult<Vec<(String, Vec<String>)>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        Ok(jj_repo
            .view()
            .local_bookmarks()
            .map(|(name, target)| {
                (
                    name.as_str().to_string(),
                    target.added_ids().map(|id| id.hex()).collect(),
                )
            })
            .collect())
    }

    /// Create a tag (name → commit pin) at the commit `at` resolves to.
    pub fn create_tag(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        at: &RefSpec,
        author: &str,
    ) -> VcsResult<String> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit = self.resolve_ref(&jj_repo, at)?;
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_tag_target(RefName::new(name), RefTarget::normal(commit.clone()));
        self.commit_tx(tx, &format!("create_tag {name}"))?;
        Ok(commit.hex())
    }

    /// Delete a tag.
    pub fn delete_tag(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        author: &str,
    ) -> VcsResult<()> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo.view().get_local_tag(RefName::new(name)).is_absent() {
            return Err(VcsError::TagNotFound(name.to_string()));
        }
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_tag_target(RefName::new(name), RefTarget::absent());
        self.commit_tx(tx, &format!("delete_tag {name}"))
    }

    /// List tags (name → commit id) at the current view.
    pub fn list_tags(&self, project: &str, repo: &str) -> VcsResult<Vec<(String, String)>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        Ok(jj_repo
            .view()
            .local_tags()
            .filter_map(|(name, target)| {
                target
                    .added_ids()
                    .next()
                    .map(|id| (name.as_str().to_string(), id.hex()))
            })
            .collect())
    }

    // ── Operation log & undo ──────────────────────────────────────────────────

    /// List the operation log for a repo, ordered oldest→newest along the
    /// current head's parent chain (design.md §4.4).
    pub fn list_operations(&self, project: &str, repo: &str) -> VcsResult<Vec<OpRecord>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let loader = jj_repo.loader();
        let root_op_id = loader.op_store().root_operation_id().clone();

        // Walk the operation parent chain from head back to (but excluding) root.
        let mut chain = Vec::new();
        let mut cursor = vec![jj_repo.operation().clone()];
        let mut seen = std::collections::HashSet::new();
        while let Some(op) = cursor.pop() {
            if op.id() == &root_op_id || !seen.insert(op.id().clone()) {
                continue;
            }
            let meta = op.metadata();
            // Prefer the schemahub-stamped author attribute (the authenticated
            // identity that drove the change) over jj's default
            // `meta.username` (a stable but anonymous hostname/jj string).
            let author = meta
                .attributes
                .get(AUTHOR_ATTRIBUTE)
                .cloned()
                .unwrap_or_else(|| meta.username.clone());
            chain.push(OpRecord {
                op_id: op.id().hex(),
                parents: op.parent_ids().iter().map(|p| p.hex()).collect(),
                description: meta.description.clone(),
                author,
                timestamp: meta.time.end.timestamp.0.to_string(),
            });
            for parent_id in op.parent_ids() {
                if parent_id != &root_op_id {
                    let parent = Self::map_jj(self.store.block_on(loader.load_operation(parent_id)))?;
                    cursor.push(parent);
                }
            }
        }
        // The chain is in newest→oldest discovery order; reverse to oldest→newest.
        chain.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(chain)
    }

    /// Undo the next-older change, walking content history back MONOTONICALLY
    /// (append-only — design.md §4.4). This is a linear-undo stack, not jj's bare
    /// op-toggle: repeated `undo` keeps stepping further back rather than redoing
    /// the previous undo. Records a new operation whose view equals the target
    /// state and returns the op id of the change that was undone (rolled past).
    ///
    /// Semantics: let the linear op chain hold content ops `[C_n .. C_1]`
    /// (newest→oldest) interleaved with `u` consecutive `undo` ops at the head.
    /// The current displayed state is content op `C_(n-u)`; this call advances to
    /// `C_(n-u-1)`, or to the empty/initial state once the oldest write is undone.
    /// `NothingToUndo` once there is nothing older to roll back to.
    pub fn undo(&self, project: &str, repo: &str, author: &str) -> VcsResult<String> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let loader = jj_repo.loader();
        let root_op_id = loader.op_store().root_operation_id().clone();

        // Walk the linear op chain from head toward root, classifying each op as
        // an `undo` op (description starts with "undo ") or a content op.
        let mut content_ops: Vec<jj_lib::operation::Operation> = Vec::new();
        let mut leading_undos = 0usize;
        let mut seen_content = false;
        let mut cursor = Some(jj_repo.operation().clone());
        while let Some(op) = cursor {
            if op.id() == &root_op_id {
                break;
            }
            let is_undo = op.metadata().description.starts_with("undo ");
            if is_undo {
                // Only count undo ops that are CONSECUTIVE at the head (before
                // any content op is seen) toward the current walk-back depth.
                if !seen_content {
                    leading_undos += 1;
                }
            } else {
                seen_content = true;
                content_ops.push(op.clone());
            }
            // Follow the single non-root parent (the chain is linear).
            cursor = match op.parent_ids().iter().find(|p| **p != root_op_id) {
                Some(parent_id) => {
                    Some(Self::map_jj(self.store.block_on(loader.load_operation(parent_id)))?)
                }
                None => None,
            };
        }

        // No content to roll back at all.
        if content_ops.is_empty() {
            return Err(VcsError::NothingToUndo);
        }

        // Target the change `leading_undos + 1` steps back from the newest write.
        // depth in [0, len): restore that content op's own view (state after it).
        // depth == len: restore the empty/initial state (parent of the oldest op).
        // depth  > len: already at empty — nothing left to undo.
        let depth = leading_undos + 1;
        if depth > content_ops.len() {
            return Err(VcsError::NothingToUndo);
        }
        // The op identifying the change being undone (the one whose effect we are
        // rolling past): the content op currently displayed, at index `leading_undos`.
        let undone_op_id = content_ops[leading_undos].id().hex();

        let target_view = if depth < content_ops.len() {
            // Restore the view recorded AFTER the target content op.
            let target_op = &content_ops[depth];
            let target_repo = Self::map_jj(self.store.block_on(loader.load_at(target_op)))?;
            target_repo.view().clone()
        } else {
            // depth == len: restore the state BEFORE the oldest write — i.e. its
            // operation parent's view (the empty/initial repo).
            let oldest = &content_ops[content_ops.len() - 1];
            let parent_id = oldest
                .parent_ids()
                .iter()
                .find(|p| **p != root_op_id)
                .cloned();
            match parent_id {
                Some(pid) => {
                    let parent_op =
                        Self::map_jj(self.store.block_on(loader.load_operation(&pid)))?;
                    let parent_repo =
                        Self::map_jj(self.store.block_on(loader.load_at(&parent_op)))?;
                    parent_repo.view().clone()
                }
                None => {
                    // The oldest write's only parent is the op-store root: restore
                    // the root operation's (empty) view.
                    let root_op =
                        Self::map_jj(self.store.block_on(loader.load_operation(&root_op_id)))?;
                    let root_repo =
                        Self::map_jj(self.store.block_on(loader.load_at(&root_op)))?;
                    root_repo.view().clone()
                }
            }
        };

        // Record a new operation whose view equals the target state.
        let mut tx = jj_repo.start_transaction();
        tx.repo_mut().set_view(target_view.store_view().clone());
        Self::record_author(&mut tx, author);
        self.commit_tx(tx, &format!("undo {undone_op_id}"))?;
        Ok(undone_op_id)
    }

    // ── Commit log ────────────────────────────────────────────────────────────

    /// Walk the real commit/change graph from a ref (newest→oldest), up to
    /// `limit` commits. Unlike the op-log, this is the content history.
    pub fn commit_log(
        &self,
        project: &str,
        repo: &str,
        at_ref: &RefSpec,
        limit: usize,
    ) -> VcsResult<Vec<CommitRecord>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let start = self.resolve_ref(&jj_repo, at_ref)?;
        let root_id = jj_repo.store().root_commit_id().clone();

        let mut out = Vec::new();
        let mut queue = vec![start];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = queue.pop() {
            if id == root_id || !seen.insert(id.clone()) {
                continue;
            }
            if out.len() >= limit {
                break;
            }
            let commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&id)))?;
            out.push(CommitRecord {
                commit_id: commit.id().hex(),
                change_id: commit.change_id().reverse_hex(),
                parents: commit.parent_ids().iter().map(|p| p.hex()).collect(),
                author: commit.author().name.clone(),
                message: commit.description().to_string(),
                timestamp: commit.author().timestamp.timestamp.0.to_string(),
            });
            for parent in commit.parent_ids() {
                if *parent != root_id {
                    queue.push(parent.clone());
                }
            }
        }
        // Newest-first by timestamp.
        out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(out)
    }

    // ── Conflicts ─────────────────────────────────────────────────────────────

    /// Read the competing sides of a conflicted declaration at a ref.
    pub fn read_conflict(
        &self,
        project: &str,
        repo: &str,
        schema_path: &str,
        decl: &str,
        at_ref: &RefSpec,
    ) -> VcsResult<ConflictSides> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&jj_repo, at_ref)?;
        let commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&commit_id)))?;
        let tree = commit.tree();
        let path = decl_path(schema_path, decl)?;
        let value = Self::map_jj(self.store.block_on(tree.path_value(&path)))?;
        if value.is_resolved() {
            return Err(VcsError::NotConflicted {
                decl: decl.to_string(),
            });
        }
        // removes() = the base/negative sides; adds() = the positive sides.
        let base = value
            .removes()
            .find_map(|opt| opt.as_ref())
            .and_then(|v| self.tree_value_blob(&jj_repo, v).transpose())
            .transpose()?;
        let mut sides = Vec::new();
        for add in value.adds() {
            if let Some(v) = add {
                if let Some(blob) = self.tree_value_blob(&jj_repo, v)? {
                    sides.push(blob);
                }
            }
        }
        Ok(ConflictSides { base, sides })
    }

    fn tree_value_blob(
        &self,
        repo: &Arc<ReadonlyRepo>,
        value: &TreeValue,
    ) -> VcsResult<Option<DeclBlob>> {
        match value {
            TreeValue::File { id, .. } => Ok(Some(DeclBlob::new(self.read_file(repo, id)?))),
            _ => Ok(None),
        }
    }

    /// Replace a conflicted declaration with a resolved blob under a new commit,
    /// move the bookmark, and record one operation.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_conflict(
        &self,
        project: &str,
        repo: &str,
        bookmark: &str,
        schema_path: &str,
        decl: &str,
        resolved: DeclBlob,
        author: &str,
        message: &str,
    ) -> VcsResult<WriteResult> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let tip = self
            .resolve_ref(&jj_repo, &RefSpec::bookmark(bookmark))
            .map_err(|_| VcsError::BookmarkNotFound(bookmark.to_string()))?;
        let tip_commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&tip)))?;

        let file_id = self.write_file(&jj_repo, resolved.as_bytes())?;
        let mut builder = jj_lib::merged_tree_builder::MergedTreeBuilder::new(tip_commit.tree());
        builder.set_or_remove(decl_path(schema_path, decl)?, resolved_file(file_id));
        let new_tree = Self::map_jj(self.store.block_on(builder.write_tree()))?;

        let signature = author_signature(author);
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        let commit = Self::map_jj(self.store.block_on(
            tx.repo_mut()
                .new_commit(vec![tip.clone()], new_tree)
                .set_author(signature.clone())
                .set_committer(signature)
                .set_description(message)
                .write(),
        ))?;
        self.set_bookmark_in_tx(&mut tx, bookmark, commit.id().clone());
        Self::map_jj(self.store.block_on(tx.repo_mut().add_head(&commit)))?;
        self.commit_tx(tx, &format!("resolve_conflict {schema_path}/{decl}"))?;

        Ok(WriteResult {
            commit_id: commit.id().hex(),
            change_id: commit.change_id().reverse_hex(),
            conflicted_decls: vec![],
        })
    }

    // ── Merge ─────────────────────────────────────────────────────────────────

    /// Merge bookmark `src` into bookmark `dst`, producing first-class conflicts
    /// rather than failing (design.md §6). Creates a merge commit (two parents)
    /// whose tree is jj's auto-merge over the merge base; same-declaration
    /// divergence becomes a stored jj conflict. Moves `dst`.
    pub fn merge(
        &self,
        project: &str,
        repo: &str,
        src: &str,
        dst: &str,
        author: &str,
    ) -> VcsResult<WriteResult> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let src_id = self
            .resolve_ref(&jj_repo, &RefSpec::bookmark(src))
            .map_err(|_| VcsError::BookmarkNotFound(src.to_string()))?;
        let dst_id = self
            .resolve_ref(&jj_repo, &RefSpec::bookmark(dst))
            .map_err(|_| VcsError::BookmarkNotFound(dst.to_string()))?;

        let dst_commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&dst_id)))?;
        let src_commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&src_id)))?;

        // jj's merge_commit_trees does the N-way merge over the index-derived
        // base, yielding first-class conflicts inline.
        let merged_tree = Self::map_jj(self.store.block_on(
            jj_lib::rewrite::merge_commit_trees(jj_repo.as_ref(), &[dst_commit.clone(), src_commit.clone()]),
        ))?;
        let conflicted = self.conflicted_decls(&merged_tree)?;

        let signature = author_signature(author);
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        let commit = Self::map_jj(self.store.block_on(
            tx.repo_mut()
                .new_commit(vec![dst_id.clone(), src_id.clone()], merged_tree)
                .set_author(signature.clone())
                .set_committer(signature)
                .set_description(format!("merge {src} into {dst}"))
                .write(),
        ))?;
        self.set_bookmark_in_tx(&mut tx, dst, commit.id().clone());
        Self::map_jj(self.store.block_on(tx.repo_mut().add_head(&commit)))?;
        self.commit_tx(tx, &format!("merge {src} into {dst}"))?;

        Ok(WriteResult {
            commit_id: commit.id().hex(),
            change_id: commit.change_id().reverse_hex(),
            conflicted_decls: conflicted,
        })
    }

    // ── GC ────────────────────────────────────────────────────────────────────

    /// Mark-and-sweep garbage collection over the [`ObjectDb`]: marks every
    /// object reachable from the repos' bookmark/tag/head targets and the full
    /// op-log (so undo keeps working), then sweeps unreachable
    /// File/Tree/Commit/View objects. Returns the number of objects swept.
    pub fn gc(&self, repos: &[(String, String)]) -> VcsResult<usize> {
        use std::collections::HashSet;

        let mut reachable_commits: HashSet<String> = HashSet::new();
        let mut reachable_views: HashSet<String> = HashSet::new();

        for (project, repo) in repos {
            let repo_key = Self::repo_key(project, repo);
            let jj_repo = self.store.load_repo(&repo_key)?;
            let loader = jj_repo.loader();
            let root_op_id = loader.op_store().root_operation_id().clone();

            // Every operation's view is a GC root (op-log retention for undo).
            // A failure here is fail-fast: silently skipping a load would mean
            // the op's bookmarks/tags/heads are absent from the reachable set,
            // and the sweep below would happily delete still-pinned objects.
            for op_id in self.store.db.list_ops(&repo_key)? {
                let op_id = jj_lib::op_store::OperationId::new(op_id.0);
                if op_id == root_op_id {
                    continue;
                }
                let op = Self::map_jj(self.store.block_on(loader.load_operation(&op_id)))?;
                let view = Self::map_jj(self.store.block_on(op.view()))?;
                reachable_views.insert(op.view_id().hex());
                for head in view.heads() {
                    reachable_commits.insert(head.hex());
                }
                for (_n, target) in view.local_bookmarks() {
                    for id in target.added_ids() {
                        reachable_commits.insert(id.hex());
                    }
                }
                for (_n, target) in view.local_tags() {
                    for id in target.added_ids() {
                        reachable_commits.insert(id.hex());
                    }
                }
            }
        }

        // Walk commit ancestry → mark commits + their root trees + files.
        let mut reachable_trees: HashSet<String> = HashSet::new();
        let mut reachable_files: HashSet<String> = HashSet::new();
        let any_repo = match repos.first() {
            Some((p, r)) => self.store.load_repo(&Self::repo_key(p, r))?,
            None => return Ok(0),
        };
        let root_id = any_repo.store().root_commit_id().clone();
        let mut queue: Vec<String> = reachable_commits.iter().cloned().collect();
        while let Some(c_hex) = queue.pop() {
            let cid = CommitId::try_from_hex(&c_hex)
                .ok_or_else(|| VcsError::Corrupt(format!("malformed reachable commit hex: {c_hex}")))?;
            if cid == root_id {
                continue;
            }
            // Fail-fast on read errors — silently skipping here would leave
            // this commit's trees + files unmarked, and the sweep would then
            // drop content the operation log still references.
            let commit = Self::map_jj(self.store.block_on(any_repo.store().get_commit_async(&cid)))?;
            for tree_id in commit.tree_ids().iter() {
                self.mark_tree(&any_repo, tree_id, &mut reachable_trees, &mut reachable_files)?;
            }
            for parent in commit.parent_ids() {
                if *parent != root_id && reachable_commits.insert(parent.hex()) {
                    queue.push(parent.hex());
                }
            }
        }

        // Sweep.
        let mut swept = 0;
        swept += self.sweep(ObjectKind::Commit, &reachable_commits)?;
        swept += self.sweep(ObjectKind::Tree, &reachable_trees)?;
        swept += self.sweep(ObjectKind::File, &reachable_files)?;
        swept += self.sweep(ObjectKind::View, &reachable_views)?;
        Ok(swept)
    }

    fn mark_tree(
        &self,
        repo: &Arc<ReadonlyRepo>,
        tree_id: &jj_lib::backend::TreeId,
        trees: &mut std::collections::HashSet<String>,
        files: &mut std::collections::HashSet<String>,
    ) -> VcsResult<()> {
        if !trees.insert(tree_id.hex()) {
            return Ok(());
        }
        let tree = Self::map_jj(self.store.block_on(
            repo.store()
                .get_tree(jj_lib::repo_path::RepoPathBuf::root(), tree_id),
        ))?;
        for entry in tree.entries_non_recursive() {
            match entry.value() {
                TreeValue::File { id, .. } => {
                    files.insert(id.hex());
                }
                TreeValue::Tree(sub) => {
                    self.mark_tree(repo, sub, trees, files)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn sweep(
        &self,
        kind: ObjectKind,
        keep: &std::collections::HashSet<String>,
    ) -> VcsResult<usize> {
        let mut swept = 0;
        for id in self.store.db.list_objects(kind)? {
            if !keep.contains(&id.to_hex()) {
                self.store.db.delete_object(kind, &id)?;
                swept += 1;
            }
        }
        Ok(swept)
    }
}

// ── free helpers ──────────────────────────────────────────────────────────────

/// Encode a declaration name into a SINGLE jj path component.
///
/// jj treats `/` as a tree-path separator, so a raw decl name like
/// `path:/users` would otherwise split into multiple components and be lost on
/// read (see `load_schema`'s direct-child filter). We percent-encode the two
/// characters that would break round-tripping — `%` (the escape char itself,
/// encoded first to keep the scheme collision-free) and `/` (the separator) —
/// leaving every other byte untouched so common names stay human-readable.
/// Inverse of [`decode_decl_name`].
fn encode_decl_name(name: &str) -> String {
    name.replace('%', "%25").replace('/', "%2F")
}

/// Decode a jj path component back into the exact original declaration name.
/// Inverse of [`encode_decl_name`]: undo `/` first, then the escape char, so a
/// literal `%2F` in the original name (stored as `%252F`) is not mistaken for an
/// encoded separator.
fn decode_decl_name(component: &str) -> String {
    component.replace("%2F", "/").replace("%25", "%")
}

/// Build the `<schema>/<encoded-name>` repo path for a declaration or
/// `__meta__`. The decl-name component is encoded so names containing `/` (or
/// `%`) map to a single jj path component under the schema subtree and decode
/// back to the exact original name. The schema-file level is NOT encoded.
fn decl_path(schema: &str, name: &str) -> VcsResult<RepoPathBuf> {
    let encoded = encode_decl_name(name);
    RepoPathBuf::from_internal_string(format!("{schema}/{encoded}"))
        .map_err(|e| VcsError::Corrupt(format!("bad path {schema}/{encoded}: {e}")))
}

/// A resolved (non-conflicted) file tree value for `MergedTreeBuilder`.
fn resolved_file(id: FileId) -> Merge<Option<TreeValue>> {
    Merge::normal(TreeValue::File {
        id,
        executable: false,
        copy_id: CopyId::placeholder(),
    })
}

/// A jj signature for `author` with the current timestamp.
fn author_signature(author: &str) -> Signature {
    Signature {
        name: author.to_string(),
        email: format!("{author}@schemahub"),
        timestamp: Timestamp::now(),
    }
}

#[cfg(test)]
mod tests;
