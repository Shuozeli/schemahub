//! `schemahub-server` library surface — the composition root pieces, exposed so
//! integration tests can build the same `Core` + tonic service stack in-process
//! (crate-structure.md §3.6, §6 testing strategy).

#![allow(clippy::result_large_err)]

pub mod config;
pub mod error;
pub mod http;
pub mod services;
pub mod wire;

use std::collections::HashMap;
use std::sync::Arc;

use schemahub_compiler_flatbuffers::FlatBuffersCompiler;
use schemahub_compiler_openapi::OpenApiCompiler;
use schemahub_compiler_protobuf::ProtobufCompiler;
use schemahub_core::{
    BearerTokenAuthn, CompilerRegistry, Core, FileProjectStore, FileRoleStore, ProjectMeta,
    ProjectStore, RoleBasedAuthz, RoleStore,
};
use schemahub_jj::{Jj, ObjectDb};
use schemahub_types::{AuthnProvider, AuthzPolicy, Identity, NoopAuthn, NoopAuthz, Role};

use tonic::transport::server::Router;
use tonic::transport::Server;

use schemahub_api::schemahub_v1::admin_service_server::AdminServiceServer;
use schemahub_api::schemahub_v1::codegen_service_server::CodegenServiceServer;
use schemahub_api::schemahub_v1::exploration_service_server::ExplorationServiceServer;
use schemahub_api::schemahub_v1::history_service_server::HistoryServiceServer;
use schemahub_api::schemahub_v1::project_service_server::ProjectServiceServer;
use schemahub_api::schemahub_v1::ref_service_server::RefServiceServer;
use schemahub_api::schemahub_v1::schema_service_server::SchemaServiceServer;

use crate::config::Config;
use crate::services::admin::AdminHandler;
use crate::services::bookmark::BookmarkHandler;
use crate::services::codegen::CodegenHandler;
use crate::services::exploration::ExplorationHandler;
use crate::services::history::HistoryHandler;
use crate::services::project::ProjectHandler;
use crate::services::schema::SchemaHandler;

/// Build the [`Core`] over a concrete object store, registering all three
/// compilers and seeding the per-repo config + project/role registries from
/// `config`.
///
/// Auth wiring (design.md §6):
/// - **No `[auth]` section** → `NoopAuthn` + `NoopAuthz` + empty in-memory
///   stores. Today's behavior, every request is anonymous.
/// - **`[auth].tokens` set** → `BearerTokenAuthn` + `RoleBasedAuthz` backed
///   by `FileRoleStore` + `FileProjectStore` under `[auth].data_dir`. The
///   `[projects.<name>]` bootstrap seeds the registries (idempotent: skips
///   any project already present in the file).
///
/// Panics on store-file IO errors at startup — fail-fast (design.md §6 +
/// user infra-defaults: surface config errors at startup, not at first RPC).
pub fn build_core(db: Arc<dyn ObjectDb>, config: &Config) -> Arc<Core> {
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    registry.register(Arc::new(FlatBuffersCompiler::new()));
    registry.register(Arc::new(OpenApiCompiler::new()));

    let jj = Arc::new(Jj::new(db));
    let repo_configs = config.repo_config_store();

    if !config.auth_enabled() {
        return Arc::new(Core::with_config(
            jj,
            registry,
            Arc::new(NoopAuthn),
            Arc::new(NoopAuthz),
            repo_configs,
        ));
    }

    let data_dir = std::path::PathBuf::from(&config.auth.data_dir);
    let role_store = Arc::new(
        FileRoleStore::open(data_dir.join("roles.json"))
            .expect("opening file role store at [auth].data_dir/roles.json"),
    );
    let project_store = Arc::new(
        FileProjectStore::open(data_dir.join("projects.json"))
            .expect("opening file project store at [auth].data_dir/projects.json"),
    );
    bootstrap_projects(role_store.as_ref(), project_store.as_ref(), config)
        .expect("bootstrapping [projects.*] into the role/project registries");

    let tokens: HashMap<String, Identity> = config
        .auth
        .tokens
        .iter()
        .map(|(t, id)| (t.clone(), id.to_identity()))
        .collect();

    let authn: Arc<dyn AuthnProvider> = Arc::new(BearerTokenAuthn::new(tokens));
    let authz: Arc<dyn AuthzPolicy> = Arc::new(RoleBasedAuthz::new(
        role_store.clone() as Arc<dyn RoleStore>,
        project_store.clone() as Arc<dyn ProjectStore>,
    ));

    Arc::new(Core::with_stores(
        jj,
        registry,
        authn,
        authz,
        repo_configs,
        role_store,
        project_store,
    ))
}

/// Seed the file-backed role + project registries from `[projects.<name>]`.
/// Idempotent — entries that already exist on disk are left untouched (so a
/// production deployment can hand-edit `roles.json` without the config
/// resetting it on every restart).
fn bootstrap_projects(
    roles: &dyn RoleStore,
    projects: &dyn ProjectStore,
    config: &Config,
) -> anyhow::Result<()> {
    for (name, section) in &config.projects {
        let visibility = section.parsed_visibility();
        if projects.get(name).is_none() {
            // First-time: register the project. The creator field is just an
            // audit hint here — we set it to the first owner if any.
            let creator = section.owners.first().cloned().unwrap_or_default();
            projects.set(ProjectMeta {
                name: name.clone(),
                visibility,
                creator,
            })?;
        }
        for owner_id in &section.owners {
            // Always set Owner — bootstrap fixes drift from the toml.
            roles.set(name, owner_id, Role::Owner)?;
        }
        for (id, role) in section.parsed_members()? {
            // Don't downgrade a configured Owner.
            if matches!(
                roles.get(name, &Identity::user(id.clone())),
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
    Server::builder()
        .add_service(SchemaServiceServer::new(SchemaHandler::new(core.clone())))
        .add_service(ExplorationServiceServer::new(ExplorationHandler::new(
            core.clone(),
        )))
        .add_service(CodegenServiceServer::new(CodegenHandler::new(core.clone())))
        .add_service(RefServiceServer::new(BookmarkHandler::new(core.clone())))
        .add_service(HistoryServiceServer::new(HistoryHandler::new(core.clone())))
        .add_service(AdminServiceServer::new(AdminHandler::new(
            core.clone(),
            storage_backend,
        )))
        .add_service(ProjectServiceServer::new(ProjectHandler::new(core)))
}
