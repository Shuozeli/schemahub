//! `schemahub-core` — orchestration over the VCS layer and the compiler
//! registry (crate-structure.md §3.3, design.md §5–§11).
//!
//! Core is the pass-through orchestrator: it never interprets mutation-op bytes
//! — it routes by `format_id` to a [`Compiler`](schemahub_types::Compiler) from
//! the [`CompilerRegistry`], runs the auth + compatibility policy, and delegates
//! storage to [`Vcs`]. The mutation/transaction/exploration/codegen/conflict/
//! history/gc flows are split into focused modules; their public methods are all
//! `impl Core` blocks so the surface is a single type.
//!
//! ## Public surface (the server's contract)
//! - Mutation: [`Core::apply_mutation`], [`Core::apply_mutations`],
//!   [`Core::apply_mutations_with_limits`]
//! - Exploration: [`Core::list_schemas`], [`Core::list_declarations`],
//!   [`Core::get_declaration`], [`Core::follow_type`],
//!   [`Core::list_dependencies`], [`Core::get_schema_source`], [`Core::search`],
//!   [`Core::search_detailed`]
//! - Codegen: [`Core::generate_descriptors`], [`Core::generate_code`],
//!   [`Core::preview_codegen`]
//! - Conflicts: [`Core::render_conflict`], [`Core::resolve_conflict`]
//! - History: [`Core::log`], [`Core::op_log`], [`Core::undo`], [`Core::diff`],
//!   [`Core::diff_bookmarks`]
//! - Refs: [`Core::create_bookmark`], [`Core::move_bookmark`],
//!   [`Core::delete_bookmark`], [`Core::list_bookmarks`], [`Core::create_tag`],
//!   [`Core::delete_tag`], [`Core::list_tags`], [`Core::merge`]
//! - Admin: [`Core::gc`], [`Core::rebuild_index`]

pub mod auth;
pub mod auth_files;
pub mod auth_impls;
pub mod auth_store;
pub mod codegen;
pub mod config;
pub mod conflict;
pub mod error;
pub mod exploration;
pub mod gc;
pub mod history;
pub mod mutation;
pub mod projects;
pub mod refs;
pub mod registry;
pub mod request;

use std::sync::Arc;

pub use auth_files::{FileProjectStore, FileRoleStore};
pub use auth_impls::{BearerTokenAuthn, RoleBasedAuthz};
pub use auth_store::{ProjectMeta, ProjectStore, RoleStore};
pub use config::{RepoConfig, RepoConfigStore};
pub use error::{CoreError, CoreResult};
pub use mutation::idempotency::IdempotencyStore;
pub use mutation::load_base;
pub use registry::CompilerRegistry;
pub use request::{
    CodegenRequest, DeclLocation, LogEntry, MutationRequest, MutationResponse, OperationRecord,
    SearchHit, TransactionLimits, TransactionRequest,
};

use schemahub_types::{AuthnProvider, AuthzPolicy};
use schemahub_vcs::Vcs;

/// The orchestration root. Holds the VCS handle, the compiler registry, the auth
/// providers, the per-repo compatibility config, the project + role registries,
/// and the idempotency edge cache. Constructed by `schemahub-server` (the
/// composition root). Cheap to share behind an `Arc` — all methods take `&self`.
pub struct Core {
    pub(crate) vcs: Arc<Vcs>,
    pub(crate) registry: CompilerRegistry,
    pub(crate) authn: Arc<dyn AuthnProvider>,
    pub(crate) authz: Arc<dyn AuthzPolicy>,
    pub(crate) repo_configs: RepoConfigStore,
    pub(crate) idempotency: IdempotencyStore,
    pub(crate) role_store: Arc<dyn RoleStore>,
    pub(crate) project_store: Arc<dyn ProjectStore>,
}

impl Core {
    /// Construct the core with default (empty) per-repo config + in-memory
    /// (non-persistent) role/project stores. Useful for tests and the
    /// getting-started default path where no `[auth]` is configured.
    pub fn new(
        vcs: Arc<Vcs>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
    ) -> Self {
        Self::with_stores(
            vcs,
            registry,
            authn,
            authz,
            RepoConfigStore::new(),
            Arc::new(EmptyRoleStore),
            Arc::new(EmptyProjectStore),
        )
    }

    /// Construct the core with an explicit per-repo config store, defaulting
    /// the role/project stores to empty in-memory stubs.
    pub fn with_config(
        vcs: Arc<Vcs>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
    ) -> Self {
        Self::with_stores(
            vcs,
            registry,
            authn,
            authz,
            repo_configs,
            Arc::new(EmptyRoleStore),
            Arc::new(EmptyProjectStore),
        )
    }

    /// Construct the core with explicit role + project stores. This is the
    /// full constructor the server uses when `[auth]` is configured.
    pub fn with_stores(
        vcs: Arc<Vcs>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        role_store: Arc<dyn RoleStore>,
        project_store: Arc<dyn ProjectStore>,
    ) -> Self {
        Self {
            vcs,
            registry,
            authn,
            authz,
            repo_configs,
            idempotency: IdempotencyStore::new(),
            role_store,
            project_store,
        }
    }

    /// Access the compiler registry (read-only).
    pub fn registry(&self) -> &CompilerRegistry {
        &self.registry
    }

    /// Access the VCS handle (read-only). Lets the server perform low-level ops
    /// not yet wrapped by a Core method.
    pub fn vcs(&self) -> &Arc<Vcs> {
        &self.vcs
    }

    /// Register / replace a repo's compatibility config at runtime.
    pub fn set_repo_config(
        &mut self,
        project: impl Into<String>,
        repo: impl Into<String>,
        config: RepoConfig,
    ) {
        self.repo_configs.set(project, repo, config);
    }
}

/// Detect a format id from a schema file name extension.
/// `.proto` → "protobuf", `.fbs` → "flatbuffers", `.yaml`/`.yml`/`.json` → "openapi".
pub fn detect_format_from_name(schema_name: &str) -> Option<&'static str> {
    if schema_name.ends_with(".proto") {
        Some("protobuf")
    } else if schema_name.ends_with(".fbs") {
        Some("flatbuffers")
    } else if schema_name.ends_with(".yaml")
        || schema_name.ends_with(".yml")
        || schema_name.ends_with(".json")
    {
        Some("openapi")
    } else {
        None
    }
}

/// Empty fallback `RoleStore` used when the server is built without an
/// explicit one (the Noop default deployment). Every lookup is `None`; writes
/// succeed silently (so a Noop-auth deployment can still call
/// `add_member` without surfacing a "no store" error to clients).
struct EmptyRoleStore;

impl RoleStore for EmptyRoleStore {
    fn get(&self, _project: &str, _identity: &schemahub_types::Identity) -> Option<schemahub_types::Role> {
        None
    }
    fn set(&self, _project: &str, _identity_id: &str, _role: schemahub_types::Role) -> std::io::Result<()> {
        Ok(())
    }
    fn remove(&self, _project: &str, _identity_id: &str) -> std::io::Result<()> {
        Ok(())
    }
    fn list_project(&self, _project: &str) -> Vec<(String, schemahub_types::Role)> {
        Vec::new()
    }
}

/// Empty fallback `ProjectStore`. See [`EmptyRoleStore`] for the rationale.
struct EmptyProjectStore;

impl ProjectStore for EmptyProjectStore {
    fn get(&self, _project: &str) -> Option<ProjectMeta> {
        None
    }
    fn set(&self, _meta: ProjectMeta) -> std::io::Result<()> {
        Ok(())
    }
    fn list(&self) -> Vec<ProjectMeta> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
