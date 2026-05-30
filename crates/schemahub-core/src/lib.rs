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
pub mod codegen;
pub mod config;
pub mod conflict;
pub mod error;
pub mod exploration;
pub mod gc;
pub mod history;
pub mod mutation;
pub mod refs;
pub mod registry;
pub mod request;

use std::sync::Arc;

pub use config::{RepoConfig, RepoConfigStore};
pub use error::{CoreError, CoreResult};
pub use mutation::idempotency::IdempotencyStore;
pub use registry::CompilerRegistry;
pub use request::{
    CodegenRequest, DeclLocation, LogEntry, MutationRequest, MutationResponse, OperationRecord,
    SearchHit, TransactionLimits, TransactionRequest,
};

use schemahub_types::{AuthnProvider, AuthzPolicy};
use schemahub_vcs::Vcs;

/// The orchestration root. Holds the VCS handle, the compiler registry, the auth
/// providers, the per-repo compatibility config, and the idempotency edge cache.
/// Constructed by `schemahub-server` (the composition root). Cheap to share
/// behind an `Arc` — all methods take `&self`.
pub struct Core {
    pub(crate) vcs: Arc<Vcs>,
    pub(crate) registry: CompilerRegistry,
    pub(crate) authn: Arc<dyn AuthnProvider>,
    pub(crate) authz: Arc<dyn AuthzPolicy>,
    pub(crate) repo_configs: RepoConfigStore,
    pub(crate) idempotency: IdempotencyStore,
}

impl Core {
    /// Construct the core with default (empty) per-repo config — every repo
    /// falls back to [`RepoConfig::default`] (protect `main`, FULL compatibility).
    pub fn new(
        vcs: Arc<Vcs>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
    ) -> Self {
        Self::with_config(vcs, registry, authn, authz, RepoConfigStore::new())
    }

    /// Construct the core with an explicit per-repo config store (seeded by the
    /// server from `schemahub.toml`).
    pub fn with_config(
        vcs: Arc<Vcs>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
    ) -> Self {
        Self {
            vcs,
            registry,
            authn,
            authz,
            repo_configs,
            idempotency: IdempotencyStore::new(),
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

#[cfg(test)]
mod tests;
