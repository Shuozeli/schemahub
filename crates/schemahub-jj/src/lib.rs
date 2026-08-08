//! `schemahub-jj` — the format-agnostic JJ layer
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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use memory_db::MemoryObjectDb;
pub use object_db::{
    ObjectDb, ObjectDbError, ObjectDbLockGuard, ObjectDbResult, ObjectId, ObjectKind, OpId,
    RecordMutation,
};
#[cfg(feature = "postgres")]
pub use pg_db::PgObjectDb;
pub use redb_db::RedbObjectDb;

use jj_lib::backend::{BackendError, CommitId, CopyId, FileId, Signature, Timestamp, TreeValue};
use jj_lib::merge::{Merge, MergedTreeValue};
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
/// [`Jj::list_operations`] and exposed in [`OpRecord::author`].
pub(crate) const AUTHOR_ATTRIBUTE: &str = "schemahub.author";

#[derive(Debug, Error)]
pub enum JjError {
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
    #[error("tag already exists: {0}")]
    TagExists(String),
    #[error("ref does not resolve to a commit: {0}")]
    BadRef(String),
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("declaration {decl} is not conflicted")]
    NotConflicted { decl: String },
    #[error("corrupt repository data: {0}")]
    Corrupt(String),
    #[error("jj error: {0}")]
    Other(String),
}

pub type JjResult<T> = Result<T, JjError>;

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
    /// The JJ operation that atomically published this commit/bookmark view.
    pub operation_id: String,
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
    /// Structured correlation metadata stamped by SchemaHub workflows.
    pub attributes: BTreeMap<String, String>,
}

/// One final schema-file write within an atomic multi-file commit.
#[derive(Clone, Debug)]
pub enum SchemaWrite {
    Patch {
        schema_path: String,
        effect: MutationEffect,
    },
    Delete {
        schema_path: String,
    },
}

/// Result boundary for a JJ publication that runs a caller-supplied policy
/// against the exact final tree while holding the repository publication lock.
/// A policy rejection is known to happen before any commit or operation is
/// published; a JJ error can be operationally ambiguous and must retain its
/// durable retry/reconciliation marker.
#[derive(Debug)]
pub enum PublicationError<E> {
    Jj(JjError),
    Rejected(E),
}

impl<E> From<JjError> for PublicationError<E> {
    fn from(error: JjError) -> Self {
        Self::Jj(error)
    }
}

/// Read-only view of the exact merged tree proposed for publication.
///
/// The value is valid only during the publication-policy callback. It exposes
/// schema-shaped reads without leaking jj-lib tree types across the crate
/// boundary. `known_schema_names` is the union of schemas visible in the
/// publication inputs and final tree, which lets reference-integrity policy
/// identify an import whose provider disappeared during a concurrent merge.
pub struct PublicationSnapshot<'a> {
    jj: &'a Jj,
    repo: &'a Arc<ReadonlyRepo>,
    final_tree: &'a jj_lib::merged_tree::MergedTree,
    known_schema_names: BTreeSet<String>,
    bookmark_target_conflicted: bool,
}

impl PublicationSnapshot<'_> {
    pub fn conflicted_declarations(&self) -> JjResult<Vec<String>> {
        self.jj.conflicted_declaration_paths(self.final_tree)
    }

    pub fn list_schemas(&self) -> JjResult<Vec<String>> {
        self.jj.list_schemas_in_tree(self.final_tree)
    }

    pub fn load_schema(&self, schema_path: &str) -> JjResult<SchemaObjects> {
        self.jj
            .load_schema_from_tree(self.repo, self.final_tree, schema_path)
    }

    pub fn known_schema_names(&self) -> &BTreeSet<String> {
        &self.known_schema_names
    }

    pub fn bookmark_target_conflicted(&self) -> bool {
        self.bookmark_target_conflicted
    }
}

/// A commit/change graph node returned by [`Jj::commit_log`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    pub commit_id: String,
    pub change_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub message: String,
    pub timestamp: String,
}

/// One bounded lexicographical page from a repository-local named-ref
/// namespace. `next_cursor` is the last returned name and is intentionally
/// transport-neutral; callers bind it to the request scope before exposing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedRefPage {
    pub refs: Vec<(String, String)>,
    pub next_cursor: Option<String>,
}

/// One bounded lexicographical page of schema-file names from an immutable
/// repository tree. The cursor is the last returned schema path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaNamePage {
    pub schemas: Vec<String>,
    pub next_cursor: Option<String>,
}

/// A caller-selected set of schemas loaded from one immutable tree traversal.
///
/// `all_schema_names` is collected during the same traversal so higher layers
/// can normalize repository-local imports without scanning the tree again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaLoadBatch {
    pub schemas: BTreeMap<String, SchemaObjects>,
    pub all_schema_names: BTreeSet<String>,
}

/// Conflict counts computed without materializing every conflicted path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictStats {
    pub total: usize,
    pub by_schema: BTreeMap<String, usize>,
}

/// The schemahub-shaped JJ handle — the contract `schemahub-core` consumes.
///
/// All methods are synchronous; the jj async backend is bridged via a dedicated
/// runtime owned by the inner [`Store`] (see `repo.rs`). A `(project, repo)`
/// pair scopes the op-log and bookmarks; content objects dedup globally.
pub struct Jj {
    store: Store,
    maintenance: std::sync::RwLock<()>,
}

struct MutationGuard<'a> {
    _local: std::sync::RwLockReadGuard<'a, ()>,
    _backend: Box<dyn object_db::ObjectDbLockGuard + 'a>,
}

struct PublicationGuard<'a> {
    _mutation: MutationGuard<'a>,
    _publication: Box<dyn object_db::ObjectDbLockGuard + 'a>,
}

struct GcGuard<'a> {
    _local: std::sync::RwLockWriteGuard<'a, ()>,
    _backend: Box<dyn object_db::ObjectDbLockGuard + 'a>,
}

impl Jj {
    /// Construct the JJ layer over a concrete object store.
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self {
            store: Store::new(db),
            maintenance: std::sync::RwLock::new(()),
        }
    }

    /// Clone the durable object-store handle backing this JJ instance.
    ///
    /// Higher format-agnostic layers use the same database for control-plane
    /// records that must be committed alongside the repository's lifetime but
    /// are not themselves JJ objects (for example first-materialized serving
    /// artifacts). Callers receive only the narrow [`ObjectDb`] abstraction.
    pub fn object_db(&self) -> Arc<dyn ObjectDb> {
        self.store.db.clone()
    }

    fn mutation_guard(&self) -> JjResult<MutationGuard<'_>> {
        let local = self
            .maintenance
            .read()
            .map_err(|error| JjError::Other(format!("poisoned maintenance lock: {error}")))?;
        let backend = self.store.db.acquire_mutation_guard()?;
        Ok(MutationGuard {
            _local: local,
            _backend: backend,
        })
    }

    fn publication_guard(&self, repo_key: &str) -> JjResult<PublicationGuard<'_>> {
        let mutation = self.mutation_guard()?;
        let publication = self.store.db.acquire_publication_guard(repo_key)?;
        Ok(PublicationGuard {
            _mutation: mutation,
            _publication: publication,
        })
    }

    fn gc_guard(&self) -> JjResult<GcGuard<'_>> {
        let local = self
            .maintenance
            .write()
            .map_err(|error| JjError::Other(format!("poisoned maintenance lock: {error}")))?;
        let backend = self.store.db.acquire_gc_guard()?;
        Ok(GcGuard {
            _local: local,
            _backend: backend,
        })
    }

    /// The per-repo key used to scope the op-log and refs.
    fn repo_key(project: &str, repo: &str) -> String {
        format!("{project}/{repo}")
    }

    // ── Ref resolution ────────────────────────────────────────────────────────

    /// Resolve a [`RefSpec`] to a [`CommitId`] against the repo's current view.
    ///
    /// Raw commit ids require an ownership proof because backend objects are
    /// deduplicated across repositories. Bookmark and tag targets already come
    /// from this repository's scoped view.
    fn resolve_ref(
        &self,
        repo_key: &str,
        repo: &Arc<ReadonlyRepo>,
        at: &RefSpec,
    ) -> JjResult<CommitId> {
        let view = repo.view();
        match at {
            RefSpec::Bookmark(name) => {
                let target = view.get_local_bookmark(RefName::new(name));
                target
                    .added_ids()
                    .next()
                    .cloned()
                    .ok_or_else(|| JjError::BookmarkNotFound(name.clone()))
            }
            RefSpec::Tag(name) => {
                let target = view.get_local_tag(RefName::new(name));
                target
                    .added_ids()
                    .next()
                    .cloned()
                    .ok_or_else(|| JjError::TagNotFound(name.clone()))
            }
            RefSpec::Commit(id_hex) => {
                let commit = CommitId::try_from_hex(id_hex)
                    .ok_or_else(|| JjError::BadRef(id_hex.clone()))?;
                self.validate_revision_in_repo(repo_key, repo, &commit)?;
                Ok(commit)
            }
        }
    }

    /// Resolve a ref for a write, tolerating a missing bookmark (returns None so
    /// a fresh bookmark can be created).
    fn try_resolve(
        &self,
        repo_key: &str,
        repo: &Arc<ReadonlyRepo>,
        at: &RefSpec,
    ) -> JjResult<Option<CommitId>> {
        match self.resolve_ref(repo_key, repo, at) {
            Ok(id) => Ok(Some(id)),
            Err(JjError::BookmarkNotFound(_)) | Err(JjError::TagNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Resolve a ref to an immutable commit id for planning a future write.
    /// A missing bookmark resolves to jj's root commit so callers can validate
    /// the first schema change in a new repository without inventing a mutable
    /// or empty base identifier. Raw commit ids are loaded before returning,
    /// which rejects syntactically valid ids that are not present in storage.
    pub fn resolve_ref_or_root(&self, project: &str, repo: &str, at: &RefSpec) -> JjResult<String> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self
            .try_resolve(&repo_key, &jj_repo, at)?
            .unwrap_or_else(|| jj_repo.store().root_commit_id().clone());
        match self
            .store
            .block_on(jj_repo.store().get_commit_async(&commit_id))
        {
            Ok(_) => {}
            Err(BackendError::ObjectNotFound { .. }) => return Err(JjError::ObjectNotFound),
            Err(error) => return Err(JjError::Other(error.to_string())),
        }
        Ok(commit_id.hex())
    }

    /// Resolve an existing bookmark/tag/commit to an immutable commit id.
    /// Unlike [`Jj::resolve_ref_or_root`], a missing mutable ref is an error.
    pub fn resolve_ref_id(&self, project: &str, repo: &str, at: &RefSpec) -> JjResult<String> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at)?;
        match self
            .store
            .block_on(jj_repo.store().get_commit_async(&commit_id))
        {
            Ok(_) => Ok(commit_id.hex()),
            Err(BackendError::ObjectNotFound { .. }) => Err(JjError::ObjectNotFound),
            Err(error) => Err(JjError::Other(error.to_string())),
        }
    }

    /// List every repository represented by the shared ObjectDb's ref or
    /// operation namespaces. The returned `(project, repo)` pairs are sorted
    /// and deduplicated; malformed storage keys fail closed rather than making
    /// a global inventory silently incomplete.
    pub fn list_repository_keys(&self) -> JjResult<Vec<(String, String)>> {
        let mut repositories = Vec::new();
        for key in self.store.db.list_repo_keys()? {
            let Some((project, repo)) = key.split_once('/') else {
                return Err(JjError::Other(format!(
                    "malformed repository key in object database: {key:?}"
                )));
            };
            if project.is_empty() || repo.is_empty() || repo.contains('/') {
                return Err(JjError::Other(format!(
                    "malformed repository key in object database: {key:?}"
                )));
            }
            repositories.push((project.to_string(), repo.to_string()));
        }
        repositories.sort();
        repositories.dedup();
        Ok(repositories)
    }

    /// Verify that a commit belongs to the named repository's retained JJ
    /// history. Content objects deduplicate globally, so checking existence
    /// alone would let a caller present another repository's commit id. This
    /// walks heads retained by current and historical operation views, then
    /// their commit ancestors.
    pub fn validate_revision(&self, project: &str, repo: &str, commit_hex: &str) -> JjResult<()> {
        let target = CommitId::try_from_hex(commit_hex)
            .ok_or_else(|| JjError::BadRef(commit_hex.to_string()))?;
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        self.validate_revision_in_repo(&repo_key, &jj_repo, &target)
    }

    fn validate_revision_in_repo(
        &self,
        repo_key: &str,
        jj_repo: &Arc<ReadonlyRepo>,
        target: &CommitId,
    ) -> JjResult<()> {
        let loader = jj_repo.loader();
        let root_op_id = loader.op_store().root_operation_id().clone();
        let mut queue: Vec<CommitId> = jj_repo.view().heads().iter().cloned().collect();
        for op_id in self.store.db.list_ops(repo_key)? {
            let op_id = jj_lib::op_store::OperationId::new(op_id.0);
            if op_id == root_op_id {
                continue;
            }
            let operation = Self::map_jj(self.store.block_on(loader.load_operation(&op_id)))?;
            let view = Self::map_jj(self.store.block_on(operation.view()))?;
            queue.extend(view.heads().iter().cloned());
        }
        queue.push(jj_repo.store().root_commit_id().clone());

        let mut seen = std::collections::HashSet::new();
        while let Some(commit_id) = queue.pop() {
            if !seen.insert(commit_id.clone()) {
                continue;
            }
            if &commit_id == target {
                return Ok(());
            }
            let commit = match self
                .store
                .block_on(jj_repo.store().get_commit_async(&commit_id))
            {
                Ok(commit) => commit,
                Err(BackendError::ObjectNotFound { .. }) => continue,
                Err(error) => return Err(JjError::Other(error.to_string())),
            };
            queue.extend(commit.parent_ids().iter().cloned());
        }
        Err(JjError::BadRef(format!(
            "commit {} is not retained by repository {repo_key}",
            target.hex()
        )))
    }

    fn map_jj<T, E: std::fmt::Display>(r: Result<T, E>) -> JjResult<T> {
        r.map_err(|e| JjError::Other(e.to_string()))
    }

    // ── Reads ─────────────────────────────────────────────────────────────────

    /// Reassemble a schema file's objects (meta + clean decls) at a ref.
    /// Conflicted declarations are omitted; inspect them via [`Jj::read_conflict`].
    pub fn load_schema(
        &self,
        project: &str,
        repo: &str,
        schema_path: &str,
        at_ref: &RefSpec,
    ) -> JjResult<SchemaObjects> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        self.load_schema_from_tree(&jj_repo, &commit.tree(), schema_path)
    }

    /// Load a bounded caller-selected schema set and the repository schema-name
    /// inventory from one immutable tree traversal.
    pub fn load_schemas(
        &self,
        project: &str,
        repo: &str,
        schema_paths: &BTreeSet<String>,
        at_ref: &RefSpec,
    ) -> JjResult<SchemaLoadBatch> {
        if schema_paths.is_empty() {
            return Ok(SchemaLoadBatch {
                schemas: BTreeMap::new(),
                all_schema_names: BTreeSet::new(),
            });
        }
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        self.load_schemas_from_tree(&jj_repo, &commit.tree(), schema_paths)
    }

    fn load_schema_from_tree(
        &self,
        repo: &Arc<ReadonlyRepo>,
        tree: &jj_lib::merged_tree::MergedTree,
        schema_path: &str,
    ) -> JjResult<SchemaObjects> {
        let selected = BTreeSet::from([schema_path.to_string()]);
        self.load_schemas_from_tree(repo, tree, &selected)?
            .schemas
            .remove(schema_path)
            .ok_or_else(|| JjError::SchemaNotFound(schema_path.to_string()))
    }

    fn load_schemas_from_tree(
        &self,
        repo: &Arc<ReadonlyRepo>,
        tree: &jj_lib::merged_tree::MergedTree,
        schema_paths: &BTreeSet<String>,
    ) -> JjResult<SchemaLoadBatch> {
        let mut schemas: BTreeMap<String, SchemaObjects> = schema_paths
            .iter()
            .cloned()
            .map(|path| (path, SchemaObjects::default()))
            .collect();
        let mut found_schemas = BTreeSet::new();
        let mut all_schema_names = BTreeSet::new();

        for (path, value) in tree.entries() {
            let value = Self::map_jj(value)?;
            let internal = path.as_internal_file_string();
            let Some((schema_path, component)) = internal.rsplit_once('/') else {
                continue;
            };
            if component == META_NAME {
                all_schema_names.insert(schema_path.to_string());
            }
            let Some(schema) = schemas.get_mut(schema_path) else {
                continue;
            };
            found_schemas.insert(schema_path.to_string());
            // Conflicted entries are surfaced via read_conflict, not here.
            let Ok(Some(TreeValue::File { id, .. })) = value.into_resolved() else {
                continue;
            };
            let bytes = self.read_file(repo, &id)?;
            if component == META_NAME {
                schema.meta = MetaBlob::new(bytes);
            } else {
                // `component` is the single encoded decl-name component; decode
                // it back to the original name (which itself may contain `/`).
                schema
                    .decls
                    .insert(decode_decl_name(component), DeclBlob::new(bytes));
            }
        }
        if let Some(missing) = schema_paths.difference(&found_schemas).next() {
            return Err(JjError::SchemaNotFound(missing.clone()));
        }
        Ok(SchemaLoadBatch {
            schemas,
            all_schema_names,
        })
    }

    /// List conflicted declaration paths at an immutable ref. Paths retain the
    /// schema-file prefix and decode the declaration-name component, making the
    /// result suitable for validation and audit output without exposing JJ's
    /// internal path encoding.
    pub fn list_conflicted_declarations(
        &self,
        project: &str,
        repo: &str,
        at_ref: &RefSpec,
    ) -> JjResult<Vec<String>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        self.conflicted_declaration_paths(&commit.tree())
    }

    /// Count all conflicts and the conflicts belonging to a bounded caller
    /// selection of schema paths without collecting the repository-wide
    /// conflict namespace.
    pub fn conflict_stats(
        &self,
        project: &str,
        repo: &str,
        at_ref: &RefSpec,
        selected_schemas: &BTreeSet<String>,
    ) -> JjResult<ConflictStats> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        let mut total = 0usize;
        let mut by_schema = BTreeMap::new();
        for (path, value) in commit.tree().conflicts() {
            Self::map_jj(value)?;
            total = total
                .checked_add(1)
                .ok_or_else(|| JjError::Corrupt("conflict count overflow".to_string()))?;
            let internal = path.as_internal_file_string();
            let Some((schema, _)) = internal.rsplit_once('/') else {
                continue;
            };
            if selected_schemas.contains(schema) {
                let count = by_schema.entry(schema.to_string()).or_insert(0usize);
                *count = count.checked_add(1).ok_or_else(|| {
                    JjError::Corrupt("schema conflict count overflow".to_string())
                })?;
            }
        }
        Ok(ConflictStats { total, by_schema })
    }

    fn conflicted_declaration_paths(
        &self,
        tree: &jj_lib::merged_tree::MergedTree,
    ) -> JjResult<Vec<String>> {
        let mut conflicts = Vec::new();
        for (path, value) in tree.conflicts() {
            Self::map_jj(value)?;
            let internal = path.as_internal_file_string();
            let decoded = match internal.rsplit_once('/') {
                Some((schema, component)) => {
                    format!("{schema}/{}", decode_decl_name(component))
                }
                None => decode_decl_name(internal),
            };
            conflicts.push(decoded);
        }
        conflicts.sort();
        conflicts.dedup();
        Ok(conflicts)
    }

    /// List schema-file names present at a ref.
    pub fn list_schemas(
        &self,
        project: &str,
        repo: &str,
        at_ref: &RefSpec,
    ) -> JjResult<Vec<String>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        self.list_schemas_in_tree(&commit.tree())
    }

    /// List one bounded schema-name page in lexicographical path order.
    ///
    /// The tree iterator stops after one page plus lookahead and does not load
    /// declaration blobs. A later page resumes exclusively after the returned
    /// schema path.
    pub fn list_schemas_page(
        &self,
        project: &str,
        repo: &str,
        at_ref: &RefSpec,
        start_after: Option<&str>,
        limit: usize,
    ) -> JjResult<SchemaNamePage> {
        if limit == 0 {
            return Ok(SchemaNamePage {
                schemas: Vec::new(),
                next_cursor: None,
            });
        }
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        let mut schemas = Vec::with_capacity(limit);
        for (path, value) in commit.tree().entries() {
            Self::map_jj(value)?;
            let internal = path.as_internal_file_string();
            let Some(schema) = internal.strip_suffix(&format!("/{META_NAME}")) else {
                continue;
            };
            if start_after.is_some_and(|cursor| schema <= cursor) {
                continue;
            }
            schemas.push(schema.to_string());
            if schemas.len() > limit {
                break;
            }
        }
        let has_more = schemas.len() > limit;
        if has_more {
            schemas.truncate(limit);
        }
        let next_cursor = if has_more {
            schemas.last().cloned()
        } else {
            None
        };
        Ok(SchemaNamePage {
            schemas,
            next_cursor,
        })
    }

    fn list_schemas_in_tree(
        &self,
        tree: &jj_lib::merged_tree::MergedTree,
    ) -> JjResult<Vec<String>> {
        let mut schemas = std::collections::BTreeSet::new();
        for (path, value) in tree.entries() {
            Self::map_jj(value)?;
            let internal = path.as_internal_file_string();
            if let Some(schema) = internal.strip_suffix(&format!("/{META_NAME}")) {
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
    ) -> JjResult<Vec<String>> {
        let objs = self.load_schema(project, repo, schema_path, at_ref)?;
        Ok(objs.decls.keys().cloned().collect())
    }

    /// Fetch one declaration's blob at a ref. Errors if the declaration is
    /// conflicted (use [`Jj::read_conflict`]).
    pub fn get_declaration(
        &self,
        project: &str,
        repo: &str,
        schema_path: &str,
        decl: &str,
        at_ref: &RefSpec,
    ) -> JjResult<DeclBlob> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        let tree = commit.tree();
        let path = decl_path(schema_path, decl)?;
        let value = Self::map_jj(self.store.block_on(tree.path_value(&path)))?;
        match value.into_resolved() {
            Ok(Some(TreeValue::File { id, .. })) => {
                Ok(DeclBlob::new(self.read_file(&jj_repo, &id)?))
            }
            Ok(Some(_)) => Err(JjError::Corrupt(format!(
                "{schema_path}/{decl} is not a file"
            ))),
            Ok(None) => Err(JjError::DeclNotFound(decl.to_string())),
            Err(_) => Err(JjError::NotConflicted {
                decl: format!("{decl} is conflicted; use read_conflict"),
            }),
        }
    }

    /// Read a file blob by jj FileId.
    fn read_file(&self, repo: &Arc<ReadonlyRepo>, id: &FileId) -> JjResult<Vec<u8>> {
        use tokio::io::AsyncReadExt as _;
        let bytes = self.store.block_on(async {
            let mut reader = repo
                .store()
                .read_file(jj_lib::repo_path::RepoPath::root(), id)
                .await?;
            let mut buf = Vec::new();
            reader
                .read_to_end(&mut buf)
                .await
                .map_err(|e| jj_lib::backend::BackendError::Other(Box::new(e)))?;
            Ok::<_, jj_lib::backend::BackendError>(buf)
        });
        Self::map_jj(bytes)
    }

    /// Write a file blob, returning its jj FileId.
    fn write_file(&self, repo: &Arc<ReadonlyRepo>, bytes: &[u8]) -> JjResult<FileId> {
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
    ) -> JjResult<WriteResult> {
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

    /// Like [`Jj::commit_write`] but touches several schema files atomically in
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
    ) -> JjResult<WriteResult> {
        let writes = effects
            .into_iter()
            .map(|(schema_path, effect)| SchemaWrite::Patch {
                schema_path,
                effect,
            })
            .collect();
        self.commit_schema_changes(
            project,
            repo,
            bookmark,
            base_ref,
            writes,
            author,
            message,
            BTreeMap::new(),
        )
    }

    /// Commit final patch/delete writes across several schema files while
    /// stamping structured attributes on the publishing JJ operation. Change
    /// application uses those attributes as a durable crash-recovery marker.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_schema_changes(
        &self,
        project: &str,
        repo: &str,
        bookmark: &str,
        base_ref: &RefSpec,
        writes: Vec<SchemaWrite>,
        author: &str,
        message: &str,
        operation_attributes: BTreeMap<String, String>,
    ) -> JjResult<WriteResult> {
        match self.commit_schema_changes_validated(
            project,
            repo,
            bookmark,
            base_ref,
            writes,
            author,
            message,
            operation_attributes,
            |_| Ok::<(), std::convert::Infallible>(()),
        ) {
            Ok(write) => Ok(write),
            Err(PublicationError::Jj(error)) => Err(error),
            Err(PublicationError::Rejected(never)) => match never {},
        }
    }

    /// Commit schema changes only if `validate` accepts the exact final merged
    /// tree. The callback and operation publication run under one repository
    /// publication guard, so no other SchemaHub writer can invalidate the
    /// decision between validation and the operation-head update.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_schema_changes_validated<E>(
        &self,
        project: &str,
        repo: &str,
        bookmark: &str,
        base_ref: &RefSpec,
        writes: Vec<SchemaWrite>,
        author: &str,
        message: &str,
        operation_attributes: BTreeMap<String, String>,
        validate: impl FnOnce(&PublicationSnapshot<'_>) -> Result<(), E>,
    ) -> Result<WriteResult, PublicationError<E>> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;

        let base_id = self.try_resolve(&repo_key, &jj_repo, base_ref)?;
        let current_tip = self.try_resolve(&repo_key, &jj_repo, &RefSpec::bookmark(bookmark))?;

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
        let mut builder = jj_lib::merged_tree_builder::MergedTreeBuilder::new(base_tree.clone());
        for write in &writes {
            match write {
                SchemaWrite::Patch {
                    schema_path,
                    effect,
                } => self.apply_effect(&jj_repo, &mut builder, schema_path, effect)?,
                SchemaWrite::Delete { schema_path } => {
                    self.delete_schema_from_tree(&base_tree, &mut builder, schema_path)?
                }
            }
        }
        let writer_tree = Self::map_jj(self.store.block_on(builder.write_tree()))?;

        // 2. Determine parents: the current bookmark tip (if any). When the tip
        // moved under the writer relative to `base`, create a merge commit so jj
        // produces first-class conflicts at declaration granularity.
        let signature = author_signature(author);
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        for (key, value) in operation_attributes {
            if !key.is_empty() {
                tx.set_attribute(key, value);
            }
        }

        let (final_tree, parents) = match &current_tip {
            Some(tip) if Some(tip) == base_id.as_ref() => (writer_tree, vec![tip.clone()]),
            Some(tip) => {
                // Merge the writer's tree with the tip's tree over their base.
                let tip_commit =
                    Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(tip)))?;
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
        let mut known_schema_names: BTreeSet<_> =
            self.list_schemas_in_tree(&base_tree)?.into_iter().collect();
        known_schema_names.extend(self.list_schemas_in_tree(&final_tree)?);
        {
            let snapshot = PublicationSnapshot {
                jj: self,
                repo: &jj_repo,
                final_tree: &final_tree,
                known_schema_names,
                bookmark_target_conflicted: false,
            };
            validate(&snapshot).map_err(PublicationError::Rejected)?;
        }

        let commit = Self::map_jj(
            self.store.block_on(
                tx.repo_mut()
                    .new_commit(parents, final_tree)
                    .set_author(signature.clone())
                    .set_committer(signature)
                    .set_description(message)
                    .write(),
            ),
        )?;

        // 3. Move bookmark + heads.
        self.set_bookmark_in_tx(&mut tx, bookmark, commit.id().clone());
        Self::map_jj(self.store.block_on(tx.repo_mut().add_head(&commit)))?;
        let operation_id = self.commit_tx(tx, &format!("commit_write {bookmark}: {message}"))?;

        Ok(WriteResult {
            commit_id: commit.id().hex(),
            change_id: commit.change_id().reverse_hex(),
            operation_id,
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
    ) -> JjResult<()> {
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

    /// Remove every direct entry in a schema subtree, including `__meta__`.
    /// The write plan contains at most one final write per schema, so scanning
    /// the immutable writer base is sufficient and deterministic.
    fn delete_schema_from_tree(
        &self,
        base_tree: &jj_lib::merged_tree::MergedTree,
        builder: &mut jj_lib::merged_tree_builder::MergedTreeBuilder,
        schema_path: &str,
    ) -> JjResult<()> {
        let prefix = format!("{schema_path}/");
        let mut found = false;
        for (path, value) in base_tree.entries() {
            Self::map_jj(value)?;
            let internal = path.as_internal_file_string();
            let Some(rest) = internal.strip_prefix(&prefix) else {
                continue;
            };
            if rest.contains('/') {
                continue;
            }
            found = true;
            builder.set_or_remove(path, Merge::absent());
        }
        if !found {
            return Err(JjError::SchemaNotFound(schema_path.to_string()));
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
    ) -> JjResult<jj_lib::merged_tree::MergedTree> {
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
        let merged = self
            .store
            .block_on(jj_lib::merged_tree::MergedTree::merge(merge));
        Self::map_jj(merged)
    }

    /// The conflicted declaration names in a tree. Each conflict is a
    /// `<schema>/<decl>` path; we return the bare `<decl>` basename (matching the
    /// `commit_write` contract — the touched schema file is known to the caller).
    fn conflicted_decls(&self, tree: &jj_lib::merged_tree::MergedTree) -> JjResult<Vec<String>> {
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
    ) -> JjResult<String> {
        let repo = Self::map_jj(self.store.block_on(tx.commit(description.to_string())))?;
        Ok(repo.operation().id().hex())
    }

    /// Stamp the schemahub-resolved audit author onto a transaction's op-log
    /// metadata. jj's `UserSettings::operation_username` would otherwise
    /// supply a stable but anonymous \"jj\" / hostname value; the
    /// `AUTHOR_ATTRIBUTE` is preferred by [`Jj::list_operations`] over
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
    ) -> JjResult<String> {
        match self.create_bookmark_validated(project, repo, name, from, author, |_| {
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(commit) => Ok(commit),
            Err(PublicationError::Jj(error)) => Err(error),
            Err(PublicationError::Rejected(never)) => match never {},
        }
    }

    /// Create a bookmark only after policy accepts its immutable target tree.
    pub fn create_bookmark_validated<E>(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        from: &RefSpec,
        author: &str,
        validate: impl FnOnce(&PublicationSnapshot<'_>) -> Result<(), E>,
    ) -> Result<String, PublicationError<E>> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo
            .view()
            .get_local_bookmark(RefName::new(name))
            .is_present()
        {
            return Err(JjError::BookmarkExists(name.to_string()).into());
        }
        let commit = self.resolve_ref(&repo_key, &jj_repo, from)?;
        let target = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit)),
        )?;
        let final_tree = target.tree();
        let known_schema_names = self
            .list_schemas_in_tree(&final_tree)?
            .into_iter()
            .collect();
        {
            let snapshot = PublicationSnapshot {
                jj: self,
                repo: &jj_repo,
                final_tree: &final_tree,
                known_schema_names,
                bookmark_target_conflicted: false,
            };
            validate(&snapshot).map_err(PublicationError::Rejected)?;
        }
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
    ) -> JjResult<String> {
        match self.move_bookmark_validated(project, repo, name, to, author, |_| {
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(commit) => Ok(commit),
            Err(PublicationError::Jj(error)) => Err(error),
            Err(PublicationError::Rejected(never)) => match never {},
        }
    }

    /// Move a bookmark only after policy accepts the exact target tree while
    /// the repository publication guard still protects the ref update.
    pub fn move_bookmark_validated<E>(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        to: &RefSpec,
        author: &str,
        validate: impl FnOnce(&PublicationSnapshot<'_>) -> Result<(), E>,
    ) -> Result<String, PublicationError<E>> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo
            .view()
            .get_local_bookmark(RefName::new(name))
            .is_absent()
        {
            return Err(JjError::BookmarkNotFound(name.to_string()).into());
        }
        let current = self.resolve_ref(&repo_key, &jj_repo, &RefSpec::bookmark(name))?;
        let commit = self.resolve_ref(&repo_key, &jj_repo, to)?;
        let current = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&current)),
        )?;
        let target = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit)),
        )?;
        let final_tree = target.tree();
        let mut known_schema_names: BTreeSet<_> = self
            .list_schemas_in_tree(&current.tree())?
            .into_iter()
            .collect();
        known_schema_names.extend(self.list_schemas_in_tree(&final_tree)?);
        {
            let snapshot = PublicationSnapshot {
                jj: self,
                repo: &jj_repo,
                final_tree: &final_tree,
                known_schema_names,
                bookmark_target_conflicted: false,
            };
            validate(&snapshot).map_err(PublicationError::Rejected)?;
        }
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
    ) -> JjResult<()> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo
            .view()
            .get_local_bookmark(RefName::new(name))
            .is_absent()
        {
            return Err(JjError::BookmarkNotFound(name.to_string()));
        }
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_bookmark_target(RefName::new(name), RefTarget::absent());
        self.commit_tx(tx, &format!("delete_bookmark {name}"))
            .map(|_| ())
    }

    /// List bookmarks (name → target commit ids) at the current view.
    pub fn list_bookmarks(
        &self,
        project: &str,
        repo: &str,
    ) -> JjResult<Vec<(String, Vec<String>)>> {
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

    /// Look up one bookmark without materializing the repository's complete
    /// bookmark namespace.
    pub fn get_bookmark(&self, project: &str, repo: &str, name: &str) -> JjResult<Option<String>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let target = jj_repo.view().get_local_bookmark(RefName::new(name));
        if target.is_absent() {
            return Ok(None);
        }
        let head = target
            .added_ids()
            .next()
            .map(|id| Some(id.hex()))
            .ok_or_else(|| {
                JjError::Corrupt(format!(
                    "bookmark {name:?} is present without an added target"
                ))
            })?;
        Ok(head)
    }

    /// List one bounded bookmark page in lexicographical name order.
    ///
    /// The JJ operation view stores refs in a `BTreeMap`; this method walks that
    /// immutable view lazily and materializes at most `limit + 1` matching
    /// entries. Loading the JJ view itself remains one repository-scoped object
    /// read.
    pub fn list_bookmarks_page(
        &self,
        project: &str,
        repo: &str,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> JjResult<NamedRefPage> {
        if limit == 0 {
            return Ok(NamedRefPage {
                refs: Vec::new(),
                next_cursor: None,
            });
        }
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let mut refs = jj_repo
            .view()
            .local_bookmarks()
            .skip_while(|(name, _)| {
                let name = name.as_str();
                start_after.map_or(name < name_prefix, |cursor| name <= cursor)
            })
            .take_while(|(name, _)| {
                name_prefix.is_empty() || name.as_str().starts_with(name_prefix)
            })
            .take(limit.saturating_add(1))
            .map(|(name, target)| {
                let head = target.added_ids().next().ok_or_else(|| {
                    JjError::Corrupt(format!(
                        "bookmark {:?} is present without an added target",
                        name.as_str()
                    ))
                })?;
                Ok((name.as_str().to_string(), head.hex()))
            })
            .collect::<JjResult<Vec<_>>>()?;
        let has_more = refs.len() > limit;
        if has_more {
            refs.truncate(limit);
        }
        let next_cursor = if has_more {
            Some(
                refs.last()
                    .ok_or_else(|| {
                        JjError::Corrupt(
                            "bookmark page lookahead had no returned predecessor".to_string(),
                        )
                    })?
                    .0
                    .clone(),
            )
        } else {
            None
        };
        Ok(NamedRefPage { refs, next_cursor })
    }

    /// Create a tag (name → commit pin) at the commit `at` resolves to.
    pub fn create_tag(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        at: &RefSpec,
        author: &str,
    ) -> JjResult<String> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;
        if !jj_repo.view().get_local_tag(RefName::new(name)).is_absent() {
            return Err(JjError::TagExists(name.to_string()));
        }
        let commit = self.resolve_ref(&repo_key, &jj_repo, at)?;
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_tag_target(RefName::new(name), RefTarget::normal(commit.clone()));
        self.commit_tx(tx, &format!("create_tag {name}"))?;
        Ok(commit.hex())
    }

    /// Delete a tag.
    pub fn delete_tag(&self, project: &str, repo: &str, name: &str, author: &str) -> JjResult<()> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;
        if jj_repo.view().get_local_tag(RefName::new(name)).is_absent() {
            return Err(JjError::TagNotFound(name.to_string()));
        }
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        tx.repo_mut()
            .set_local_tag_target(RefName::new(name), RefTarget::absent());
        self.commit_tx(tx, &format!("delete_tag {name}"))
            .map(|_| ())
    }

    /// List tags (name → commit id) at the current view.
    pub fn list_tags(&self, project: &str, repo: &str) -> JjResult<Vec<(String, String)>> {
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

    /// List one bounded tag page in lexicographical name order.
    pub fn list_tags_page(
        &self,
        project: &str,
        repo: &str,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> JjResult<NamedRefPage> {
        if limit == 0 {
            return Ok(NamedRefPage {
                refs: Vec::new(),
                next_cursor: None,
            });
        }
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let mut refs = jj_repo
            .view()
            .local_tags()
            .skip_while(|(name, _)| {
                let name = name.as_str();
                start_after.map_or(name < name_prefix, |cursor| name <= cursor)
            })
            .take_while(|(name, _)| {
                name_prefix.is_empty() || name.as_str().starts_with(name_prefix)
            })
            .take(limit.saturating_add(1))
            .map(|(name, target)| {
                let commit = target.added_ids().next().ok_or_else(|| {
                    JjError::Corrupt(format!(
                        "tag {:?} is present without an added target",
                        name.as_str()
                    ))
                })?;
                Ok((name.as_str().to_string(), commit.hex()))
            })
            .collect::<JjResult<Vec<_>>>()?;
        let has_more = refs.len() > limit;
        if has_more {
            refs.truncate(limit);
        }
        let next_cursor = if has_more {
            Some(
                refs.last()
                    .ok_or_else(|| {
                        JjError::Corrupt(
                            "tag page lookahead had no returned predecessor".to_string(),
                        )
                    })?
                    .0
                    .clone(),
            )
        } else {
            None
        };
        Ok(NamedRefPage { refs, next_cursor })
    }

    // ── Operation log & undo ──────────────────────────────────────────────────

    /// List the operation log for a repo, ordered oldest→newest along the
    /// current head's parent chain (design.md §4.4).
    pub fn list_operations(&self, project: &str, repo: &str) -> JjResult<Vec<OpRecord>> {
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
            chain.push(Self::operation_record(&op));
            for parent_id in op.parent_ids() {
                if parent_id != &root_op_id {
                    let parent =
                        Self::map_jj(self.store.block_on(loader.load_operation(parent_id)))?;
                    cursor.push(parent);
                }
            }
        }
        // The chain is in newest→oldest discovery order; reverse to oldest→newest.
        chain.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(chain)
    }

    /// Return at most the newest `limit` operations, ordered oldest→newest.
    ///
    /// Normal SchemaHub publication produces a linear operation chain, so this
    /// reads only the requested tail. If a concurrent JJ history introduces a
    /// branch inside that tail, it falls back to the complete graph traversal
    /// to preserve [`Self::list_operations`]'s ordering and deduplication
    /// semantics.
    pub fn list_operations_tail(
        &self,
        project: &str,
        repo: &str,
        limit: usize,
    ) -> JjResult<Vec<OpRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let loader = jj_repo.loader();
        let root_op_id = loader.op_store().root_operation_id().clone();
        let mut cursor = jj_repo.operation().clone();
        // `limit` can originate from an untrusted uint32 request. Avoid
        // reserving caller-sized memory up front; the graph walk grows only as
        // operations are actually discovered.
        let mut newest_first = Vec::new();
        let mut seen = std::collections::HashSet::new();

        while cursor.id() != &root_op_id && newest_first.len() < limit {
            if !seen.insert(cursor.id().clone()) {
                return Err(JjError::Corrupt(format!(
                    "operation graph contains a cycle at {}",
                    cursor.id().hex()
                )));
            }
            newest_first.push(Self::operation_record(&cursor));
            if newest_first.len() == limit {
                break;
            }

            let parents: Vec<_> = cursor
                .parent_ids()
                .iter()
                .filter(|parent| *parent != &root_op_id)
                .collect();
            match parents.as_slice() {
                [] => break,
                [parent] => {
                    cursor = Self::map_jj(self.store.block_on(loader.load_operation(parent)))?;
                }
                _ => {
                    let mut all = self.list_operations(project, repo)?;
                    if all.len() > limit {
                        all.drain(..all.len() - limit);
                    }
                    return Ok(all);
                }
            }
        }

        newest_first.reverse();
        Ok(newest_first)
    }

    fn operation_record(operation: &jj_lib::operation::Operation) -> OpRecord {
        let metadata = operation.metadata();
        // Prefer the schemahub-stamped author attribute (the authenticated
        // identity that drove the change) over jj's default username.
        let author = metadata
            .attributes
            .get(AUTHOR_ATTRIBUTE)
            .cloned()
            .unwrap_or_else(|| metadata.username.clone());
        OpRecord {
            op_id: operation.id().hex(),
            parents: operation
                .parent_ids()
                .iter()
                .map(|parent| parent.hex())
                .collect(),
            description: metadata.description.clone(),
            author,
            timestamp: metadata.time.end.timestamp.0.to_string(),
            attributes: metadata.attributes.clone(),
        }
    }

    /// Find the newest operation whose metadata contains every requested
    /// key/value pair. Correlation attributes are intended to be unique; the
    /// newest match is returned defensively if a repository contains legacy
    /// duplicates.
    pub fn find_operation_by_attributes(
        &self,
        project: &str,
        repo: &str,
        required: &BTreeMap<String, String>,
    ) -> JjResult<Option<OpRecord>> {
        let operations = self.list_operations(project, repo)?;
        Ok(operations.into_iter().rev().find(|operation| {
            required
                .iter()
                .all(|(key, value)| operation.attributes.get(key) == Some(value))
        }))
    }

    /// Recover the commit receipt published by a correlated operation. The
    /// bookmark is read from that operation's historical view, so recovery is
    /// stable even if later operations have moved the live bookmark again.
    pub fn find_correlated_write(
        &self,
        project: &str,
        repo: &str,
        bookmark: &str,
        required: &BTreeMap<String, String>,
    ) -> JjResult<Option<WriteResult>> {
        let Some(record) = self.find_operation_by_attributes(project, repo, required)? else {
            return Ok(None);
        };
        let operation_id = jj_lib::op_store::OperationId::try_from_hex(&record.op_id)
            .ok_or_else(|| JjError::Corrupt(format!("invalid operation id {}", record.op_id)))?;
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let loader = jj_repo.loader();
        let operation = Self::map_jj(self.store.block_on(loader.load_operation(&operation_id)))?;
        let historical_repo = Self::map_jj(self.store.block_on(loader.load_at(&operation)))?;
        let commit_id = historical_repo
            .view()
            .get_local_bookmark(RefName::new(bookmark))
            .added_ids()
            .next()
            .cloned()
            .ok_or_else(|| {
                JjError::Corrupt(format!(
                    "correlated operation {} has no target for bookmark {bookmark}",
                    record.op_id
                ))
            })?;
        let commit = Self::map_jj(
            self.store
                .block_on(historical_repo.store().get_commit_async(&commit_id)),
        )?;
        Ok(Some(WriteResult {
            commit_id: commit.id().hex(),
            change_id: commit.change_id().reverse_hex(),
            operation_id: record.op_id,
            conflicted_decls: self.conflicted_decls(&commit.tree())?,
        }))
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
    pub fn undo(&self, project: &str, repo: &str, author: &str) -> JjResult<String> {
        match self.undo_validated(project, repo, author, |_, _| {
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(operation) => Ok(operation),
            Err(PublicationError::Jj(error)) => Err(error),
            Err(PublicationError::Rejected(never)) => match never {},
        }
    }

    /// Undo only after policy accepts every bookmark target in the historical
    /// view that is about to become current.
    pub fn undo_validated<E>(
        &self,
        project: &str,
        repo: &str,
        author: &str,
        mut validate: impl FnMut(&str, &PublicationSnapshot<'_>) -> Result<(), E>,
    ) -> Result<String, PublicationError<E>> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
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
                Some(parent_id) => Some(Self::map_jj(
                    self.store.block_on(loader.load_operation(parent_id)),
                )?),
                None => None,
            };
        }

        // No content to roll back at all.
        if content_ops.is_empty() {
            return Err(JjError::NothingToUndo.into());
        }

        // Target the change `leading_undos + 1` steps back from the newest write.
        // depth in [0, len): restore that content op's own view (state after it).
        // depth == len: restore the empty/initial state (parent of the oldest op).
        // depth  > len: already at empty — nothing left to undo.
        let depth = leading_undos + 1;
        if depth > content_ops.len() {
            return Err(JjError::NothingToUndo.into());
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
                    let parent_op = Self::map_jj(self.store.block_on(loader.load_operation(&pid)))?;
                    let parent_repo =
                        Self::map_jj(self.store.block_on(loader.load_at(&parent_op)))?;
                    parent_repo.view().clone()
                }
                None => {
                    // The oldest write's only parent is the op-store root: restore
                    // the root operation's (empty) view.
                    let root_op =
                        Self::map_jj(self.store.block_on(loader.load_operation(&root_op_id)))?;
                    let root_repo = Self::map_jj(self.store.block_on(loader.load_at(&root_op)))?;
                    root_repo.view().clone()
                }
            }
        };

        for (name, target) in target_view.local_bookmarks() {
            let target_ids: Vec<_> = target.added_ids().cloned().collect();
            let Some(target_id) = target_ids.first() else {
                continue;
            };
            let target_commit = Self::map_jj(
                self.store
                    .block_on(jj_repo.store().get_commit_async(target_id)),
            )?;
            let final_tree = target_commit.tree();
            let mut known_schema_names: BTreeSet<_> = self
                .list_schemas_in_tree(&final_tree)?
                .into_iter()
                .collect();
            if target_ids.len() == 1 {
                let current = jj_repo
                    .view()
                    .get_local_bookmark(RefName::new(name.as_str()));
                for current_id in current.added_ids() {
                    let current_commit = Self::map_jj(
                        self.store
                            .block_on(jj_repo.store().get_commit_async(current_id)),
                    )?;
                    known_schema_names.extend(self.list_schemas_in_tree(&current_commit.tree())?);
                }
            }
            let snapshot = PublicationSnapshot {
                jj: self,
                repo: &jj_repo,
                final_tree: &final_tree,
                known_schema_names,
                bookmark_target_conflicted: target_ids.len() != 1,
            };
            validate(name.as_str(), &snapshot).map_err(PublicationError::Rejected)?;
        }

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
    ) -> JjResult<Vec<CommitRecord>> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let start = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
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

    /// Whether one commit changed a schema relative to its first parent. The
    /// commit id is repository-scoped before any globally deduplicated object
    /// is loaded. Metadata-only changes count, as do file creation/deletion.
    pub fn commit_touches_schema(
        &self,
        project: &str,
        repo: &str,
        commit_hex: &str,
        schema_path: &str,
    ) -> JjResult<bool> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(
            &repo_key,
            &jj_repo,
            &RefSpec::commit(commit_hex.to_string()),
        )?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        let current = self.schema_tree_state(&commit.tree(), schema_path)?;
        let parent = match commit.parent_ids().first() {
            Some(parent_id) if parent_id != jj_repo.store().root_commit_id() => {
                let parent = Self::map_jj(
                    self.store
                        .block_on(jj_repo.store().get_commit_async(parent_id)),
                )?;
                self.schema_tree_state(&parent.tree(), schema_path)?
            }
            _ => BTreeMap::new(),
        };
        Ok(current != parent)
    }

    /// Capture the raw merged values below one schema subtree. Comparing
    /// parsed [`SchemaObjects`] would omit unresolved declarations and could
    /// incorrectly hide a conflict-to-conflict change from history filters.
    fn schema_tree_state(
        &self,
        tree: &jj_lib::merged_tree::MergedTree,
        schema_path: &str,
    ) -> JjResult<BTreeMap<String, MergedTreeValue>> {
        let prefix = format!("{schema_path}/");
        let mut state = BTreeMap::new();
        for (path, value) in tree.entries() {
            let internal = path.as_internal_file_string();
            if internal.starts_with(&prefix) {
                state.insert(internal.to_string(), Self::map_jj(value)?);
            }
        }
        Ok(state)
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
    ) -> JjResult<ConflictSides> {
        let repo_key = Self::repo_key(project, repo);
        let jj_repo = self.store.load_repo(&repo_key)?;
        let commit_id = self.resolve_ref(&repo_key, &jj_repo, at_ref)?;
        let commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&commit_id)),
        )?;
        let tree = commit.tree();
        let path = decl_path(schema_path, decl)?;
        let value = Self::map_jj(self.store.block_on(tree.path_value(&path)))?;
        if value.is_resolved() {
            return Err(JjError::NotConflicted {
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
        for v in value.adds().flatten() {
            if let Some(blob) = self.tree_value_blob(&jj_repo, v)? {
                sides.push(blob);
            }
        }
        Ok(ConflictSides { base, sides })
    }

    fn tree_value_blob(
        &self,
        repo: &Arc<ReadonlyRepo>,
        value: &TreeValue,
    ) -> JjResult<Option<DeclBlob>> {
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
    ) -> JjResult<WriteResult> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;
        let tip = self
            .resolve_ref(&repo_key, &jj_repo, &RefSpec::bookmark(bookmark))
            .map_err(|_| JjError::BookmarkNotFound(bookmark.to_string()))?;
        let tip_commit = Self::map_jj(self.store.block_on(jj_repo.store().get_commit_async(&tip)))?;

        let file_id = self.write_file(&jj_repo, resolved.as_bytes())?;
        let mut builder = jj_lib::merged_tree_builder::MergedTreeBuilder::new(tip_commit.tree());
        builder.set_or_remove(decl_path(schema_path, decl)?, resolved_file(file_id));
        let new_tree = Self::map_jj(self.store.block_on(builder.write_tree()))?;

        let signature = author_signature(author);
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        let commit = Self::map_jj(
            self.store.block_on(
                tx.repo_mut()
                    .new_commit(vec![tip.clone()], new_tree)
                    .set_author(signature.clone())
                    .set_committer(signature)
                    .set_description(message)
                    .write(),
            ),
        )?;
        self.set_bookmark_in_tx(&mut tx, bookmark, commit.id().clone());
        Self::map_jj(self.store.block_on(tx.repo_mut().add_head(&commit)))?;
        let operation_id = self.commit_tx(tx, &format!("resolve_conflict {schema_path}/{decl}"))?;

        Ok(WriteResult {
            commit_id: commit.id().hex(),
            change_id: commit.change_id().reverse_hex(),
            operation_id,
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
    ) -> JjResult<WriteResult> {
        self.merge_with_attributes(
            project,
            repo,
            src,
            dst,
            author,
            &format!("merge {src} into {dst}"),
            BTreeMap::new(),
        )
    }

    /// Merge with an explicit commit message and structured publishing-operation
    /// attributes. Durable workflows use the attributes as recovery markers.
    #[allow(clippy::too_many_arguments)]
    pub fn merge_with_attributes(
        &self,
        project: &str,
        repo: &str,
        src: &str,
        dst: &str,
        author: &str,
        message: &str,
        operation_attributes: BTreeMap<String, String>,
    ) -> JjResult<WriteResult> {
        match self.merge_with_attributes_validated(
            project,
            repo,
            src,
            dst,
            author,
            message,
            operation_attributes,
            |_| Ok::<(), std::convert::Infallible>(()),
        ) {
            Ok(write) => Ok(write),
            Err(PublicationError::Jj(error)) => Err(error),
            Err(PublicationError::Rejected(never)) => match never {},
        }
    }

    /// Merge and atomically validate the exact merged tree before publishing
    /// the destination bookmark operation.
    #[allow(clippy::too_many_arguments)]
    pub fn merge_with_attributes_validated<E>(
        &self,
        project: &str,
        repo: &str,
        src: &str,
        dst: &str,
        author: &str,
        message: &str,
        operation_attributes: BTreeMap<String, String>,
        validate: impl FnOnce(&PublicationSnapshot<'_>) -> Result<(), E>,
    ) -> Result<WriteResult, PublicationError<E>> {
        let repo_key = Self::repo_key(project, repo);
        let _guard = self.publication_guard(&repo_key)?;
        let jj_repo = self.store.load_repo(&repo_key)?;
        let src_id = self
            .resolve_ref(&repo_key, &jj_repo, &RefSpec::bookmark(src))
            .map_err(|_| JjError::BookmarkNotFound(src.to_string()))?;
        let dst_id = self
            .resolve_ref(&repo_key, &jj_repo, &RefSpec::bookmark(dst))
            .map_err(|_| JjError::BookmarkNotFound(dst.to_string()))?;

        let dst_commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&dst_id)),
        )?;
        let src_commit = Self::map_jj(
            self.store
                .block_on(jj_repo.store().get_commit_async(&src_id)),
        )?;

        // jj's merge_commit_trees does the N-way merge over the index-derived
        // base, yielding first-class conflicts inline.
        let merged_tree = Self::map_jj(self.store.block_on(jj_lib::rewrite::merge_commit_trees(
            jj_repo.as_ref(),
            &[dst_commit.clone(), src_commit.clone()],
        )))?;
        let conflicted = self.conflicted_decls(&merged_tree)?;
        let mut known_schema_names: BTreeSet<_> = self
            .list_schemas_in_tree(&dst_commit.tree())?
            .into_iter()
            .collect();
        known_schema_names.extend(self.list_schemas_in_tree(&src_commit.tree())?);
        known_schema_names.extend(self.list_schemas_in_tree(&merged_tree)?);
        {
            let snapshot = PublicationSnapshot {
                jj: self,
                repo: &jj_repo,
                final_tree: &merged_tree,
                known_schema_names,
                bookmark_target_conflicted: false,
            };
            validate(&snapshot).map_err(PublicationError::Rejected)?;
        }

        let signature = author_signature(author);
        let mut tx = jj_repo.start_transaction();
        Self::record_author(&mut tx, author);
        for (key, value) in operation_attributes {
            if !key.is_empty() {
                tx.set_attribute(key, value);
            }
        }
        let commit = Self::map_jj(
            self.store.block_on(
                tx.repo_mut()
                    .new_commit(vec![dst_id.clone(), src_id.clone()], merged_tree)
                    .set_author(signature.clone())
                    .set_committer(signature)
                    .set_description(message)
                    .write(),
            ),
        )?;
        self.set_bookmark_in_tx(&mut tx, dst, commit.id().clone());
        Self::map_jj(self.store.block_on(tx.repo_mut().add_head(&commit)))?;
        let operation_id = self.commit_tx(tx, message)?;

        Ok(WriteResult {
            commit_id: commit.id().hex(),
            change_id: commit.change_id().reverse_hex(),
            operation_id,
            conflicted_decls: conflicted,
        })
    }

    // ── GC ────────────────────────────────────────────────────────────────────

    /// Mark-and-sweep garbage collection over the [`ObjectDb`]: marks every
    /// object reachable from every repository present in the shared store plus
    /// the explicitly requested roots. The full op-log remains a root so undo
    /// keeps working. Only then does it sweep unreachable File/Tree/Commit/View
    /// objects. Returns the number of objects swept.
    pub fn gc(&self, repos: &[(String, String)]) -> JjResult<usize> {
        use std::collections::{BTreeSet, HashSet};

        let _guard = self.gc_guard()?;
        let mut reachable_commits: HashSet<String> = HashSet::new();
        let mut reachable_views: HashSet<String> = HashSet::new();
        let mut repo_keys: BTreeSet<String> = self.store.db.list_repo_keys()?.into_iter().collect();
        repo_keys.extend(
            repos
                .iter()
                .map(|(project, repo)| Self::repo_key(project, repo)),
        );
        let mut any_repo = None;

        for repo_key in repo_keys {
            let jj_repo = self.store.load_repo(&repo_key)?;
            any_repo.get_or_insert_with(|| jj_repo.clone());
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
        let any_repo = match any_repo {
            Some(repo) => repo,
            None => return Ok(0),
        };
        let root_id = any_repo.store().root_commit_id().clone();
        let mut queue: Vec<String> = reachable_commits.iter().cloned().collect();
        while let Some(c_hex) = queue.pop() {
            let cid = CommitId::try_from_hex(&c_hex).ok_or_else(|| {
                JjError::Corrupt(format!("malformed reachable commit hex: {c_hex}"))
            })?;
            if cid == root_id {
                continue;
            }
            // Fail-fast on read errors — silently skipping here would leave
            // this commit's trees + files unmarked, and the sweep would then
            // drop content the operation log still references.
            let commit =
                Self::map_jj(self.store.block_on(any_repo.store().get_commit_async(&cid)))?;
            for tree_id in commit.tree_ids().iter() {
                self.mark_tree(
                    &any_repo,
                    tree_id,
                    &mut reachable_trees,
                    &mut reachable_files,
                )?;
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
    ) -> JjResult<()> {
        if !trees.insert(tree_id.hex()) {
            return Ok(());
        }
        let tree = Self::map_jj(
            self.store.block_on(
                repo.store()
                    .get_tree(jj_lib::repo_path::RepoPathBuf::root(), tree_id),
            ),
        )?;
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

    fn sweep(&self, kind: ObjectKind, keep: &std::collections::HashSet<String>) -> JjResult<usize> {
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
fn decl_path(schema: &str, name: &str) -> JjResult<RepoPathBuf> {
    let encoded = encode_decl_name(name);
    RepoPathBuf::from_internal_string(format!("{schema}/{encoded}"))
        .map_err(|e| JjError::Corrupt(format!("bad path {schema}/{encoded}: {e}")))
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
