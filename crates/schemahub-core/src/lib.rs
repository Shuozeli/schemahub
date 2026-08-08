//! `schemahub-core` — orchestration over the JJ layer and the compiler
//! registry (crate-structure.md §3.3, design.md §5–§11).
//!
//! Core is the pass-through orchestrator: it never interprets mutation-op bytes
//! — it routes by `format_id` to a [`Compiler`](schemahub_types::Compiler) from
//! the [`CompilerRegistry`], runs the auth + compatibility policy, and delegates
//! storage to [`Jj`]. The mutation/transaction/exploration/codegen/conflict/
//! history/gc flows are split into focused modules; their public methods are all
//! `impl Core` blocks so the surface is a single type.
//!
//! ## Public surface (the server's contract)
//! - Mutation: [`Core::apply_mutation`], [`Core::apply_mutations`],
//!   [`Core::apply_mutations_with_limits`], [`Core::apply_mutations_with_deadline`]
//! - Exploration: [`Core::list_schemas`], [`Core::list_declarations`],
//!   [`Core::get_declaration`], [`Core::follow_type`],
//!   [`Core::list_dependencies`], [`Core::list_dependents`],
//!   [`Core::get_schema_source`], [`Core::search`], [`Core::search_detailed`]
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
pub mod auth_object_db;
pub mod auth_store;
pub mod change_record;
pub mod changes;
pub mod codegen;
pub mod config;
pub mod conflict;
pub mod control_plane_audit;
pub mod error;
pub mod exploration;
pub mod gc;
pub mod history;
pub mod lifecycle;
pub mod mutation;
pub mod projects;
mod reference_integrity;
pub mod refs;
pub mod registry;
pub mod repository;
pub mod request;
pub mod serving;

use std::sync::Arc;

pub use auth_files::{FileProjectStore, FileRoleStore};
pub use auth_impls::{BearerTokenAuthn, RoleBasedAuthz};
pub use auth_object_db::{ObjectDbProjectStore, ObjectDbRoleStore};
pub use auth_store::{
    AccessStoreError, AccessStoreResult, ProjectMeta, ProjectStore, ProjectStorePage, RoleStore,
    RoleStorePage,
};
pub use config::{RepoConfig, RepoConfigStore, ReviewPolicy, ServingPolicy};
pub use control_plane_audit::{
    audit_collection, audit_index_collection, is_valid_audit_cursor, ControlPlaneAuditAction,
    ControlPlaneAuditClock, ControlPlaneAuditContext, ControlPlaneAuditError,
    ControlPlaneAuditEvent, ControlPlaneAuditIdGenerator, ControlPlaneAuditPage,
    ControlPlaneAuditRuntime, ControlPlaneAuditSnapshot, ObjectDbControlPlaneAuditLog,
    SystemControlPlaneAuditClock, UuidControlPlaneAuditIdGenerator,
};
pub use error::{CoreError, CoreResult};
pub use exploration::{
    SchemaInventoryStats, MAX_DEPENDENCY_SCAN_REPOSITORIES, MAX_DEPENDENCY_SCAN_SCHEMAS,
};
pub use mutation::idempotency::{
    FingerprintBuilder, IdempotencyError, IdempotencyStore, DEFAULT_TTL_HOURS,
};
pub use mutation::load_base;
pub use projects::ProjectUpdate;
pub use registry::CompilerRegistry;
pub use repository::{
    CreateRepository, MemoryRepositoryStore, ObjectDbRepositoryStore, Repository, RepositoryError,
    RepositoryPage, RepositoryStore, RepositoryStoreError, RepositoryUpdate,
};
pub use request::{
    CodegenRequest, CreateSchemaRequest, DeclLocation, DeleteSchemaRequest, DependencyScanSnapshot,
    DependentsScan, FollowedType, LogEntry, MutationRequest, MutationResponse, OperationRecord,
    RepositoryDiff, SchemaDependency, SchemaDependent, SearchHit, TransactionDeadline,
    TransactionLimits, TransactionRequest, UpdateSchemaRequest,
};
pub use schemahub_jj::{ConflictStats, NamedRefPage, SchemaLoadBatch, SchemaNamePage};
pub use serving::{SchemaArtifact, SchemaArtifactKind, SchemaRevision};

use schemahub_jj::Jj;
use schemahub_types::{AuthnProvider, AuthzPolicy};

use crate::change_record::{
    ChangeLedger, MemoryChangeRecordStore, SystemChangeClock, UuidChangeIdGenerator,
};

/// The orchestration root. Holds the JJ handle, the compiler registry, the auth
/// providers, the per-repo compatibility config, the project + role registries,
/// and the idempotency edge cache. Constructed by `schemahub-server` (the
/// composition root). Cheap to share behind an `Arc` — all methods take `&self`.
pub struct Core {
    pub(crate) jj: Arc<Jj>,
    pub(crate) registry: CompilerRegistry,
    pub(crate) authn: Arc<dyn AuthnProvider>,
    pub(crate) authz: Arc<dyn AuthzPolicy>,
    pub(crate) repo_configs: RepoConfigStore,
    pub(crate) idempotency: IdempotencyStore,
    pub(crate) role_store: Arc<dyn RoleStore>,
    pub(crate) project_store: Arc<dyn ProjectStore>,
    pub(crate) change_ledger: ChangeLedger,
    pub(crate) repository_store: Arc<dyn RepositoryStore>,
    pub(crate) artifact_store: serving::ArtifactMaterializationStore,
    pub(crate) control_plane_audit: ControlPlaneAuditRuntime,
    pub(crate) control_plane_db: Arc<dyn schemahub_jj::ObjectDb>,
}

impl Core {
    /// Construct the core with default (empty) per-repo config + in-memory
    /// (non-persistent) role/project stores. Useful for tests and the
    /// getting-started default path where no `[auth]` is configured.
    pub fn new(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
    ) -> Self {
        Self::with_stores(
            jj,
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
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
    ) -> Self {
        Self::with_config_and_change_ledger(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            default_change_ledger(),
        )
    }

    /// Construct the core with an explicit change ledger while retaining the
    /// empty role/project-store defaults. The server uses this in Noop-auth
    /// deployments so change records still persist in the selected database.
    pub fn with_config_and_change_ledger(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        change_ledger: ChangeLedger,
    ) -> Self {
        Self::with_config_and_resource_stores(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            change_ledger,
            Arc::new(MemoryRepositoryStore::new()),
        )
    }

    /// Construct the Noop-auth shape with explicit durable control-plane
    /// stores. The server uses this so ChangeRecord and Repository resources
    /// persist even when RBAC is disabled.
    #[allow(clippy::too_many_arguments)]
    pub fn with_config_and_resource_stores(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        change_ledger: ChangeLedger,
        repository_store: Arc<dyn RepositoryStore>,
    ) -> Self {
        Self::with_config_and_all_stores(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            change_ledger,
            repository_store,
            IdempotencyStore::new(),
        )
    }

    /// Construct the Noop-auth shape with every durable control-plane store.
    #[allow(clippy::too_many_arguments)]
    pub fn with_config_and_all_stores(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        change_ledger: ChangeLedger,
        repository_store: Arc<dyn RepositoryStore>,
        idempotency: IdempotencyStore,
    ) -> Self {
        Self::with_all_stores(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            Arc::new(EmptyRoleStore),
            Arc::new(EmptyProjectStore),
            change_ledger,
            repository_store,
            idempotency,
        )
    }

    /// Construct the core with explicit role + project stores. This is the
    /// full constructor the server uses when `[auth]` is configured.
    pub fn with_stores(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        role_store: Arc<dyn RoleStore>,
        project_store: Arc<dyn ProjectStore>,
    ) -> Self {
        Self::with_stores_and_change_ledger(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            role_store,
            project_store,
            default_change_ledger(),
        )
    }

    /// Full composition-root constructor with explicit durable stores.
    #[allow(clippy::too_many_arguments)]
    pub fn with_stores_and_change_ledger(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        role_store: Arc<dyn RoleStore>,
        project_store: Arc<dyn ProjectStore>,
        change_ledger: ChangeLedger,
    ) -> Self {
        Self::with_stores_and_resource_stores(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            role_store,
            project_store,
            change_ledger,
            Arc::new(MemoryRepositoryStore::new()),
        )
    }

    /// Full composition-root constructor with all mutable resource stores.
    #[allow(clippy::too_many_arguments)]
    pub fn with_stores_and_resource_stores(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        role_store: Arc<dyn RoleStore>,
        project_store: Arc<dyn ProjectStore>,
        change_ledger: ChangeLedger,
        repository_store: Arc<dyn RepositoryStore>,
    ) -> Self {
        Self::with_all_stores(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            role_store,
            project_store,
            change_ledger,
            repository_store,
            IdempotencyStore::new(),
        )
    }

    /// Full composition-root constructor with all mutable resource stores and
    /// the durable idempotency receipt ledger.
    #[allow(clippy::too_many_arguments)]
    pub fn with_all_stores(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        role_store: Arc<dyn RoleStore>,
        project_store: Arc<dyn ProjectStore>,
        change_ledger: ChangeLedger,
        repository_store: Arc<dyn RepositoryStore>,
        idempotency: IdempotencyStore,
    ) -> Self {
        Self::with_all_stores_and_audit_runtime(
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            role_store,
            project_store,
            change_ledger,
            repository_store,
            idempotency,
            ControlPlaneAuditRuntime::production(),
        )
    }

    /// Full composition-root constructor with an injected control-plane audit
    /// clock and ID generator. Tests use this to make administrative event
    /// identity and ordering deterministic.
    #[allow(clippy::too_many_arguments)]
    pub fn with_all_stores_and_audit_runtime(
        jj: Arc<Jj>,
        registry: CompilerRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
        repo_configs: RepoConfigStore,
        role_store: Arc<dyn RoleStore>,
        project_store: Arc<dyn ProjectStore>,
        change_ledger: ChangeLedger,
        repository_store: Arc<dyn RepositoryStore>,
        idempotency: IdempotencyStore,
        control_plane_audit: ControlPlaneAuditRuntime,
    ) -> Self {
        let control_plane_db = jj.object_db();
        let artifact_store = serving::ArtifactMaterializationStore::new(control_plane_db.clone());
        Self {
            jj,
            registry,
            authn,
            authz,
            repo_configs,
            idempotency,
            role_store,
            project_store,
            change_ledger,
            repository_store,
            artifact_store,
            control_plane_audit,
            control_plane_db,
        }
    }

    /// Access the compiler registry (read-only).
    pub fn registry(&self) -> &CompilerRegistry {
        &self.registry
    }

    /// Access the JJ handle (read-only). Lets the server perform low-level ops
    /// not yet wrapped by a Core method.
    pub fn jj(&self) -> &Arc<Jj> {
        &self.jj
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

fn default_change_ledger() -> ChangeLedger {
    ChangeLedger::new(
        Arc::new(MemoryChangeRecordStore::new()),
        Arc::new(SystemChangeClock),
        Arc::new(UuidChangeIdGenerator),
    )
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
/// explicit one (the Noop default deployment).
///
/// Reads return `None` (no role configured); writes **fail** with
/// `Unsupported` rather than silently dropping data. The previous behaviour
/// — returning `Ok(())` from `set`/`remove` — let a Noop-auth deployment
/// answer `CreateProject` / `AddMember` with success while persisting
/// nothing, so subsequent reads disagreed with the write that just
/// "succeeded". Per user rules: fail-fast over fail-safe.
struct EmptyRoleStore;

fn empty_store_err(op: &str) -> AccessStoreError {
    AccessStoreError::Backend(format!(
        "{op}: no role/project store is configured \
             (this server was built with Noop auth — populate \
             `[auth]` in schemahub.toml to enable project/member management)"
    ))
}

impl RoleStore for EmptyRoleStore {
    fn get(
        &self,
        _project: &str,
        _identity: &schemahub_types::Identity,
    ) -> AccessStoreResult<Option<schemahub_types::Role>> {
        Ok(None)
    }
    fn set(
        &self,
        _project: &str,
        _identity_id: &str,
        _role: schemahub_types::Role,
    ) -> AccessStoreResult<()> {
        Err(empty_store_err("RoleStore::set"))
    }
    fn remove(&self, _project: &str, _identity_id: &str) -> AccessStoreResult<()> {
        Err(empty_store_err("RoleStore::remove"))
    }
    fn list_project(
        &self,
        _project: &str,
    ) -> AccessStoreResult<Vec<(String, schemahub_types::Role)>> {
        Ok(Vec::new())
    }
}

/// Empty fallback `ProjectStore`. Same fail-fast semantics as
/// [`EmptyRoleStore`].
struct EmptyProjectStore;

impl ProjectStore for EmptyProjectStore {
    fn get(&self, _project: &str) -> AccessStoreResult<Option<ProjectMeta>> {
        Ok(None)
    }
    fn create_with_owner(
        &self,
        _meta: ProjectMeta,
        _owner_id: &str,
    ) -> AccessStoreResult<ProjectMeta> {
        Err(empty_store_err("ProjectStore::create_with_owner"))
    }
    fn set(&self, _meta: ProjectMeta) -> AccessStoreResult<()> {
        Err(empty_store_err("ProjectStore::set"))
    }
    fn replace(&self, _expected_etag: &str, _meta: ProjectMeta) -> AccessStoreResult<ProjectMeta> {
        Err(empty_store_err("ProjectStore::replace"))
    }
    fn list(&self) -> AccessStoreResult<Vec<ProjectMeta>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests;
