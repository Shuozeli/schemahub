//! Store layer — wires our DB-backed jj stores into a jj [`RepoLoader`] per
//! `(project, repo)` and bridges jj-lib's async API to schemahub's sync `Vcs`.
//!
//! - **Backend** ([`DbBackend`]) and **OpStore** ([`DbOpStore`]) persist all
//!   durable objects (files/trees/commits, operations/views) to the
//!   [`ObjectDb`].
//! - **OpHeadsStore** ([`DbOpHeadsStore`]) keeps the per-repo operation-head
//!   pointer in the `ObjectDb` ref table (durable, the substrate for `undo`).
//! - **IndexStore** / **SubmoduleStore**: jj's own `DefaultIndexStore` /
//!   `DefaultSubmoduleStore`, pointed at a per-`Vcs` temp directory. The index
//!   is a pure cache, reconstructable from the op-log; the submodule store is a
//!   stub (schemahub has no submodules). Neither holds durable schemahub state.
//!
//! ## Async bridge
//! jj-lib's `Backend`/`OpStore` are `#[async_trait]`, but our DB-backed stores
//! resolve every future synchronously (redb reads, in-memory tree merges — no
//! reactor IO or timers). The `Vcs` public API is synchronous, so we drive each
//! future to completion with [`pollster::block_on`], which polls on the calling
//! thread without entering a tokio runtime. This is deliberate: a tokio
//! `Runtime::block_on` panics ("cannot start a runtime from within a runtime")
//! when a `Vcs` method is called from inside the server's tokio runtime, whereas
//! `pollster` works whether or not an ambient runtime is present.

use std::path::PathBuf;
use std::sync::Arc;

use jj_lib::backend::Backend as _;
use jj_lib::config::StackedConfig;
use jj_lib::default_index::DefaultIndexStore;
use jj_lib::default_submodule_store::DefaultSubmoduleStore;
use jj_lib::op_store::RootOperationData;
use jj_lib::repo::{ReadonlyRepo, RepoLoader};
use jj_lib::settings::UserSettings;
use jj_lib::signing::Signer;
use jj_lib::store::Store as JjStore;
use jj_lib::tree_merge::MergeOptions;

use crate::jj_backend::DbBackend;
use crate::jj_op_heads::DbOpHeadsStore;
use crate::jj_op_store::DbOpStore;
use crate::object_db::{ObjectDb, ObjectDbError};
use crate::{VcsError, VcsResult};

/// The store layer shared by all `(project, repo)` operations.
pub(crate) struct Store {
    pub(crate) db: Arc<dyn ObjectDb>,
    settings: UserSettings,
    /// Per-`Vcs` scratch dir for jj's index/submodule caches (reconstructable).
    index_root: PathBuf,
}

impl Store {
    pub(crate) fn new(db: Arc<dyn ObjectDb>) -> Self {
        // jj's default config supplies empty user.name/email and operation
        // hostname/username, which is all `UserSettings` requires.
        let settings = UserSettings::from_config(StackedConfig::with_defaults())
            .expect("jj default settings");
        let index_root = std::env::temp_dir().join(format!(
            "schemahub-vcs-index-{}",
            uuid::Uuid::new_v4().simple()
        ));
        Self {
            db,
            settings,
            index_root,
        }
    }

    /// Drive a jj-lib future to completion on the calling thread (the sync↔async
    /// bridge). Uses `pollster` rather than a tokio runtime so it is safe to call
    /// from inside the server's tokio runtime — see the module-level docs.
    pub(crate) fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        pollster::block_on(fut)
    }

    /// Build a [`RepoLoader`] for one `(project, repo)` over the DB-backed jj
    /// stores. The index/submodule caches live under a per-repo subdir of the
    /// `Vcs`'s scratch dir.
    pub(crate) fn loader(&self, repo_key: &str) -> VcsResult<RepoLoader> {
        let backend = DbBackend::new(self.db.clone());
        let root_commit_id = backend.root_commit_id().clone();
        let merge_options =
            MergeOptions::from_settings(&self.settings).map_err(|e| VcsError::Other(e.to_string()))?;
        let jj_store = JjStore::new(Box::new(backend), Signer::from_settings(&self.settings).map_err(|e| VcsError::Other(e.to_string()))?, merge_options);

        let root_data = RootOperationData { root_commit_id };
        let op_store: Arc<dyn jj_lib::op_store::OpStore> =
            Arc::new(DbOpStore::new(self.db.clone(), repo_key.to_string(), root_data));
        let op_heads_store: Arc<dyn jj_lib::op_heads_store::OpHeadsStore> =
            Arc::new(DbOpHeadsStore::new(self.db.clone(), repo_key.to_string()));

        // Index + submodule caches are reconstructable; give them a per-repo dir.
        let index_dir = self.index_root.join(sanitize(repo_key));
        std::fs::create_dir_all(&index_dir).map_err(|e| VcsError::Other(e.to_string()))?;
        let index_store: Arc<dyn jj_lib::index::IndexStore> = Arc::new(
            DefaultIndexStore::init(&index_dir).map_err(|e| VcsError::Other(e.to_string()))?,
        );
        let submodule_store: Arc<dyn jj_lib::submodule_store::SubmoduleStore> =
            Arc::new(DefaultSubmoduleStore::init(&index_dir));

        Ok(RepoLoader::new(
            self.settings.clone(),
            jj_store,
            op_store,
            op_heads_store,
            index_store,
            submodule_store,
        ))
    }

    /// Load the repo at its current operation head, seeding the root operation
    /// head on first access (jj's `ReadonlyRepo::init` normally does this).
    pub(crate) fn load_repo(&self, repo_key: &str) -> VcsResult<Arc<ReadonlyRepo>> {
        let loader = self.loader(repo_key)?;
        // Seed the op-head with the root operation if the repo is brand new.
        let heads = self
            .block_on(loader.op_heads_store().get_op_heads())
            .map_err(|e| VcsError::Other(e.to_string()))?;
        if heads.is_empty() {
            self.block_on(
                loader
                    .op_heads_store()
                    .update_op_heads(&[], loader.op_store().root_operation_id()),
            )
            .map_err(|e| VcsError::Other(e.to_string()))?;
        }
        self.block_on(loader.load_at_head())
            .map_err(|e| VcsError::Other(e.to_string()))
    }
}

/// Make a repo key safe for use as a single path component.
fn sanitize(repo_key: &str) -> String {
    repo_key
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

impl From<ObjectDbError> for VcsError {
    fn from(e: ObjectDbError) -> Self {
        match e {
            ObjectDbError::NotFound => VcsError::ObjectNotFound,
            other => VcsError::ObjectDb(other),
        }
    }
}
