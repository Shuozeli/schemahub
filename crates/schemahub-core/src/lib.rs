pub mod error;
pub mod mutation;
pub mod objects;
pub mod plugin_registry;
pub mod repo_config;
pub mod version_control;

pub use error::CoreError;
pub use mutation::{apply_mutation, MutateRequest};
pub use plugin_registry::PluginRegistry;
pub use repo_config::RepoConfig;

use std::sync::Arc;

use schemahub_storage::StorageBackend;
use schemahub_types::{AuthnProvider, AuthzPolicy, Hash};

/// The Core struct owns all components and exposes the public API.
pub struct Core {
    pub storage: Arc<dyn StorageBackend>,
    pub plugins: PluginRegistry,
    pub authn: Arc<dyn AuthnProvider>,
    pub authz: Arc<dyn AuthzPolicy>,
}

impl Core {
    pub fn new(
        storage: Arc<dyn StorageBackend>,
        plugins: PluginRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
    ) -> Self {
        Self { storage, plugins, authn, authz }
    }

    // ── Repo config ───────────────────────────────────────────────────────────

    pub fn get_repo_config(&self, project: &str, repo: &str) -> Result<RepoConfig, CoreError> {
        repo_config::load_repo_config(self.storage.as_ref(), project, repo)
    }

    pub fn set_repo_config(&self, project: &str, repo: &str, config: &RepoConfig) -> Result<(), CoreError> {
        repo_config::save_repo_config(self.storage.as_ref(), project, repo, config)
    }

    // ── Version control ───────────────────────────────────────────────────────

    pub fn create_branch(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        from: &str,
    ) -> Result<(), CoreError> {
        let from_hash = Hash::from_hex(from)
            .map_err(|_| CoreError::InvalidArgument(format!("from is not a valid hash: {from}")))?;
        version_control::branch::create_branch(
            self.storage.as_ref(),
            project,
            repo,
            name,
            &from_hash,
        )
    }

    pub fn delete_branch(&self, project: &str, repo: &str, name: &str) -> Result<(), CoreError> {
        version_control::branch::delete_branch(self.storage.as_ref(), project, repo, name)
    }

    pub fn list_branches(
        &self,
        project: &str,
        repo: &str,
        prefix: &str,
    ) -> Result<Vec<(String, Hash)>, CoreError> {
        version_control::branch::list_branches(self.storage.as_ref(), project, repo, prefix)
    }

    pub fn get_branch_head(
        &self,
        project: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Hash, CoreError> {
        version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, branch)
    }

    // ── Mutations ─────────────────────────────────────────────────────────────

    pub fn apply_mutation(&self, req: MutateRequest) -> Result<String, CoreError> {
        let config = self.get_repo_config(&req.project, &req.repo)?;
        mutation::single::apply_mutation(
            self.storage.as_ref(),
            &self.plugins,
            self.authn.as_ref(),
            self.authz.as_ref(),
            &config,
            &req,
        )
    }
}
