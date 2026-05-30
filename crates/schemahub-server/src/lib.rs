//! `schemahub-server` library surface — the composition root pieces, exposed so
//! integration tests can build the same `Core` + tonic service stack in-process
//! (crate-structure.md §3.6, §6 testing strategy).

pub mod config;
pub mod error;
pub mod services;
pub mod wire;

use std::sync::Arc;

use schemahub_compiler_flatbuffers::FlatBuffersCompiler;
use schemahub_compiler_openapi::OpenApiCompiler;
use schemahub_compiler_protobuf::ProtobufCompiler;
use schemahub_core::{CompilerRegistry, Core};
use schemahub_types::{NoopAuthn, NoopAuthz};
use schemahub_vcs::{ObjectDb, Vcs};

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
/// compilers and seeding the per-repo config from `config`.
pub fn build_core(db: Arc<dyn ObjectDb>, config: &Config) -> Arc<Core> {
    let mut registry = CompilerRegistry::new();
    registry.register(Arc::new(ProtobufCompiler::new()));
    registry.register(Arc::new(FlatBuffersCompiler::new()));
    registry.register(Arc::new(OpenApiCompiler::new()));

    let vcs = Arc::new(Vcs::new(db));
    Arc::new(Core::with_config(
        vcs,
        registry,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
        config.repo_config_store(),
    ))
}

/// Assemble the full tonic [`Router`] with every service registered over `core`.
pub fn build_router(core: Arc<Core>) -> Router {
    Server::builder()
        .add_service(SchemaServiceServer::new(SchemaHandler::new(core.clone())))
        .add_service(ExplorationServiceServer::new(ExplorationHandler::new(
            core.clone(),
        )))
        .add_service(CodegenServiceServer::new(CodegenHandler::new(core.clone())))
        .add_service(RefServiceServer::new(BookmarkHandler::new(core.clone())))
        .add_service(HistoryServiceServer::new(HistoryHandler::new(core.clone())))
        .add_service(AdminServiceServer::new(AdminHandler::new(core.clone())))
        .add_service(ProjectServiceServer::new(ProjectHandler::new(core)))
}
