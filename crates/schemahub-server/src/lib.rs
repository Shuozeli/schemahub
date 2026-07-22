//! `schemahub-server` library surface — the composition root pieces, exposed so
//! integration tests can build the same `Core` + tonic service stack in-process
//! (crate-structure.md §3.6, §6 testing strategy).

#![allow(clippy::result_large_err)]

pub mod config;
pub mod error;
pub mod http;
pub mod jwt_auth;
pub mod observability;
pub mod services;
pub mod wire;

/// Version embedded into every public server surface. Release builds set
/// `SCHEMAHUB_VERSION` from the immutable tag; developer builds fall back to
/// the Cargo package version.
pub const BUILD_VERSION: &str = match option_env!("SCHEMAHUB_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Hard server execution deadline advertised by `GetServerConfig` and enforced
/// by the transaction handler.
pub const TRANSACTION_TIMEOUT_SECS: u64 = 30;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use schemahub_compiler_flatbuffers::FlatBuffersCompiler;
use schemahub_compiler_openapi::OpenApiCompiler;
use schemahub_compiler_protobuf::ProtobufCompiler;
use schemahub_core::{
    change_record::{
        ChangeLedger, ObjectDbChangeRecordStore, SystemChangeClock, UuidChangeIdGenerator,
    },
    repository::{now_unix_millis, validate_config},
    BearerTokenAuthn, CompilerRegistry, Core, FileProjectStore, FileRoleStore, IdempotencyStore,
    ObjectDbProjectStore, ObjectDbRepositoryStore, ObjectDbRoleStore, ProjectMeta, ProjectStore,
    RepoConfigStore, Repository, RepositoryStore, RepositoryStoreError, RoleBasedAuthz, RoleStore,
};
use schemahub_jj::{Jj, ObjectDb};
use schemahub_types::{AuthnProvider, AuthzPolicy, Identity, NoopAuthn, NoopAuthz, Role};

use tonic::transport::server::Router;
use tonic::transport::Server;
use tonic_health::server::{health_reporter, HealthReporter};

use schemahub_api::schemahub_v1::admin_service_server::AdminServiceServer;
use schemahub_api::schemahub_v1::change_service_server::ChangeServiceServer;
use schemahub_api::schemahub_v1::codegen_service_server::CodegenServiceServer;
use schemahub_api::schemahub_v1::exploration_service_server::ExplorationServiceServer;
use schemahub_api::schemahub_v1::history_service_server::HistoryServiceServer;
use schemahub_api::schemahub_v1::project_service_server::ProjectServiceServer;
use schemahub_api::schemahub_v1::ref_service_server::RefServiceServer;
use schemahub_api::schemahub_v1::schema_service_server::SchemaServiceServer;
use schemahub_api::schemahub_v1::serving_service_server::ServingServiceServer;

use crate::config::Config;
use crate::observability::ServerMetrics;
use crate::services::admin::AdminHandler;
use crate::services::bookmark::BookmarkHandler;
use crate::services::change::ChangeHandler;
use crate::services::codegen::CodegenHandler;
use crate::services::exploration::ExplorationHandler;
use crate::services::history::HistoryHandler;
use crate::services::project::ProjectHandler;
use crate::services::schema::SchemaHandler;
use crate::services::serving::ServingHandler;

/// Build the [`Core`] for noop or static-token deployments.
///
/// JWT deployments initialize remote/file key material asynchronously and use
/// [`build_core_with_authn`] instead. Keeping this convenience constructor
/// synchronous preserves the in-process test and embedded-development API.
pub fn build_core(db: Arc<dyn ObjectDb>, config: &Config) -> Arc<Core> {
    assert!(
        config.auth.jwt.is_none(),
        "JWT auth requires JwtAuthRuntime::initialize plus build_core_with_authn"
    );
    let authn: Arc<dyn AuthnProvider> = if config.auth_enabled() {
        let tokens: HashMap<String, Identity> = config
            .auth
            .tokens
            .iter()
            .map(|(token, identity)| (token.clone(), identity.to_identity()))
            .collect();
        Arc::new(BearerTokenAuthn::new(tokens))
    } else {
        Arc::new(NoopAuthn)
    };
    build_core_with_authn(db, config, authn)
}

/// Build the [`Core`] with a caller-initialized authentication provider.
///
/// The composition root still owns durable RBAC stores, legacy access-store
/// migration, project/repository bootstrap, compilers, and change/idempotency
/// stores. This seam lets production JWT key loading remain asynchronous and
/// outside core business logic.
pub fn build_core_with_authn(
    db: Arc<dyn ObjectDb>,
    config: &Config,
    authn: Arc<dyn AuthnProvider>,
) -> Arc<Core> {
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    registry.register(Arc::new(FlatBuffersCompiler::new()));
    registry.register(Arc::new(OpenApiCompiler::new()));

    let change_ledger = ChangeLedger::new(
        Arc::new(ObjectDbChangeRecordStore::new(db.clone())),
        Arc::new(SystemChangeClock),
        Arc::new(UuidChangeIdGenerator),
    );
    let repo_configs = config.repo_config_store();
    let repository_store = Arc::new(ObjectDbRepositoryStore::new(db.clone()));
    let idempotency = IdempotencyStore::over_object_db(db.clone());
    bootstrap_repositories(repository_store.as_ref(), &repo_configs)
        .expect("bootstrapping [repos.*] into the repository registry");
    let jj = Arc::new(Jj::new(db.clone()));

    if !config.auth_enabled() {
        return Arc::new(Core::with_config_and_all_stores(
            jj,
            registry,
            authn,
            Arc::new(NoopAuthz),
            repo_configs,
            change_ledger,
            repository_store,
            idempotency,
        ));
    }

    let role_store = Arc::new(ObjectDbRoleStore::new(db.clone()));
    let project_store = Arc::new(ObjectDbProjectStore::new(db));
    migrate_legacy_access_files(project_store.as_ref(), config)
        .expect("migrating legacy JSON role/project registries");
    bootstrap_projects(role_store.as_ref(), project_store.as_ref(), config)
        .expect("bootstrapping [projects.*] into the role/project registries");

    let authz: Arc<dyn AuthzPolicy> = Arc::new(RoleBasedAuthz::new(
        role_store.clone() as Arc<dyn RoleStore>,
        project_store.clone() as Arc<dyn ProjectStore>,
    ));

    Arc::new(Core::with_all_stores(
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
    ))
}

fn bootstrap_repositories(
    repositories: &dyn RepositoryStore,
    configs: &RepoConfigStore,
) -> anyhow::Result<()> {
    let now = now_unix_millis()?;
    for (project, repo, config) in configs.entries() {
        validate_config(&config)?;
        let record = Repository::new(project, repo, config, "schemahub-config", now);
        match repositories.create(record) {
            Ok(_) | Err(RepositoryStoreError::AlreadyExists(_)) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Import the former JSON project + role registries into ObjectDb once. Each
/// project and all its memberships are one database transaction. Existing
/// database projects win, which also makes repeated starts idempotent.
fn migrate_legacy_access_files(
    projects: &ObjectDbProjectStore,
    config: &Config,
) -> anyhow::Result<()> {
    let data_dir = Path::new(&config.auth.data_dir);
    let projects_path = data_dir.join("projects.json");
    if !projects_path.exists() {
        return Ok(());
    }
    let legacy_projects = FileProjectStore::open(&projects_path)?;
    let legacy_roles = FileRoleStore::open(data_dir.join("roles.json"))?;
    for mut project in legacy_projects.list()? {
        if projects.get(&project.name)?.is_some() {
            continue;
        }
        let mut members = legacy_roles.list_project(&project.name)?;
        members.sort_by(|left, right| left.0.cmp(&right.0));
        let owner = members
            .iter()
            .find(|(identity, role)| *role == Role::Owner && identity.as_str() == project.creator)
            .or_else(|| members.iter().find(|(_, role)| *role == Role::Owner))
            .map(|(identity, _)| identity.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "legacy project '{}' has no Owner in roles.json",
                    project.name
                )
            })?;
        if project.creator.is_empty() {
            project.creator = owner;
        }
        let now = now_unix_millis()?;
        if project.create_time_unix_ms == 0 {
            project.create_time_unix_ms = now;
        }
        if project.update_time_unix_ms == 0 {
            project.update_time_unix_ms = project.create_time_unix_ms;
        }
        projects.create_with_members(project, &members)?;
    }
    Ok(())
}

/// Seed the database-backed role + project registries from
/// `[projects.<name>]`. Existing projects are left untouched; configured role
/// assignments are reconciled on every start.
fn bootstrap_projects(
    roles: &dyn RoleStore,
    projects: &dyn ProjectStore,
    config: &Config,
) -> anyhow::Result<()> {
    for (name, section) in &config.projects {
        let visibility = section.parsed_visibility();
        if projects.get(name)?.is_none() {
            let creator = section
                .owners
                .first()
                .expect("validated bootstrap project has an owner");
            projects.create_with_owner(
                ProjectMeta::new(name, visibility, creator, now_unix_millis()?),
                creator,
            )?;
        }
        for owner_id in &section.owners {
            // Always set Owner — bootstrap fixes drift from the toml.
            roles.set(name, owner_id, Role::Owner)?;
        }
        for (id, role) in section.parsed_members()? {
            // Don't downgrade a configured Owner.
            if matches!(
                roles.get(name, &Identity::user(id.clone()))?,
                Some(Role::Owner)
            ) && role != Role::Owner
            {
                continue;
            }
            roles.set(name, &id, role)?;
        }
    }
    Ok(())
}

/// Assemble the full tonic [`Router`] with every service registered over `core`.
///
/// `storage_backend` is the resolved backend id (`"redb"` or `"postgres"`)
/// surfaced by `AdminService.GetServerConfig` so clients see the real
/// deployment, not a hard-coded guess.
pub fn build_router(core: Arc<Core>, storage_backend: impl Into<String>) -> Router {
    build_router_with_health(core, storage_backend).0
}

/// Build the complete gRPC router plus its standard gRPC health reporter.
/// The overall empty-service status starts as `Serving`; the process
/// composition root keeps the reporter so it can publish `NotServing` before
/// graceful shutdown begins.
pub fn build_router_with_health(
    core: Arc<Core>,
    storage_backend: impl Into<String>,
) -> (Router, HealthReporter) {
    build_router_with_health_and_metrics(core, storage_backend, ServerMetrics::default())
}

/// Build the gRPC router with a caller-owned metrics registry shared with the
/// HTTP operations surface.
pub fn build_router_with_health_and_metrics(
    core: Arc<Core>,
    storage_backend: impl Into<String>,
    metrics: ServerMetrics,
) -> (Router, HealthReporter) {
    let (health_reporter, health_service) = health_reporter();
    let grpc_metrics = metrics;
    let router = Server::builder()
        .trace_fn(move |request| {
            grpc_metrics.record_grpc_request();
            let request_id = request
                .headers()
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            tracing::info_span!(
                "grpc_request",
                event = "schemahub.grpc.request",
                rpc = request.uri().path(),
                request_id,
            )
        })
        .add_service(health_service)
        .add_service(SchemaServiceServer::new(SchemaHandler::new(core.clone())))
        .add_service(ExplorationServiceServer::new(ExplorationHandler::new(
            core.clone(),
        )))
        .add_service(CodegenServiceServer::new(CodegenHandler::new(core.clone())))
        .add_service(ChangeServiceServer::new(ChangeHandler::new(core.clone())))
        .add_service(ServingServiceServer::new(ServingHandler::new(core.clone())))
        .add_service(RefServiceServer::new(BookmarkHandler::new(core.clone())))
        .add_service(HistoryServiceServer::new(HistoryHandler::new(core.clone())))
        .add_service(AdminServiceServer::new(AdminHandler::new(
            core.clone(),
            storage_backend,
        )))
        .add_service(ProjectServiceServer::new(ProjectHandler::new(core)));
    (router, health_reporter)
}
