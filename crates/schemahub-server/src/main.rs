mod error;
mod services;

use std::sync::Arc;

use clap::Parser;
use schemahub_api::schemahub_v1::{
    admin_service_server::AdminServiceServer,
    codegen_service_server::CodegenServiceServer,
    exploration_service_server::ExplorationServiceServer,
    project_service_server::ProjectServiceServer,
    ref_service_server::RefServiceServer,
    schema_service_server::SchemaServiceServer,
};
use schemahub_core::{Core, PluginRegistry};
use schemahub_plugin_flatbuffers::FlatBuffersPlugin;
use schemahub_plugin_openapi::OpenApiPlugin;
use schemahub_plugin_protobuf::ProtobufPlugin;
use schemahub_storage::RedbBackend;
use schemahub_types::{NoopAuthn, NoopAuthz};
use tonic::transport::Server;

use services::{
    admin::AdminServiceImpl, codegen::CodegenServiceImpl, exploration::ExplorationServiceImpl,
    project::ProjectServiceImpl, refs::RefServiceImpl, schema::SchemaServiceImpl,
};

#[derive(Parser)]
#[command(name = "schemahub-server", about = "SchemaHub gRPC server")]
struct Args {
    /// The address to listen on
    #[arg(long, default_value = "[::1]:50051")]
    listen: String,

    /// Path to the redb database file
    #[arg(long, default_value = "schemahub.db")]
    db: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // ── Storage ───────────────────────────────────────────────────────────────
    let storage = RedbBackend::open(&args.db)?;
    let storage: Arc<dyn schemahub_storage::StorageBackend> = Arc::new(storage);

    // ── Plugin registry ───────────────────────────────────────────────────────
    let mut plugins = PluginRegistry::new();
    plugins.register(Arc::new(ProtobufPlugin));
    plugins.register(Arc::new(FlatBuffersPlugin));
    plugins.register(Arc::new(OpenApiPlugin));

    // ── Core ──────────────────────────────────────────────────────────────────
    let core = Arc::new(Core::new(
        storage,
        plugins,
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
    ));

    // ── gRPC services ─────────────────────────────────────────────────────────
    let schema_svc = SchemaServiceServer::new(SchemaServiceImpl::new(Arc::clone(&core)));
    let ref_svc = RefServiceServer::new(RefServiceImpl::new(Arc::clone(&core)));
    let exploration_svc =
        ExplorationServiceServer::new(ExplorationServiceImpl::new(Arc::clone(&core)));
    let codegen_svc = CodegenServiceServer::new(CodegenServiceImpl::new(Arc::clone(&core)));
    let project_svc = ProjectServiceServer::new(ProjectServiceImpl::new(Arc::clone(&core)));
    let admin_svc = AdminServiceServer::new(AdminServiceImpl::new(Arc::clone(&core)));

    // ── Listen ────────────────────────────────────────────────────────────────
    let addr = args.listen.parse()?;
    eprintln!("schemahub-server listening on {addr}");

    Server::builder()
        .add_service(schema_svc)
        .add_service(ref_svc)
        .add_service(exploration_svc)
        .add_service(codegen_svc)
        .add_service(project_svc)
        .add_service(admin_svc)
        .serve(addr)
        .await?;

    Ok(())
}
