//! Shared harness for the e2e integration tests: an in-process tonic server over
//! a `MemoryObjectDb`, connected gRPC clients, and small request helpers. Each
//! `tests/*.rs` file is its own binary, so unused helpers are expected per file.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use pb::change_service_client::ChangeServiceClient;
use pb::codegen_service_client::CodegenServiceClient;
use pb::exploration_service_client::ExplorationServiceClient;
use pb::history_service_client::HistoryServiceClient;
use pb::project_service_client::ProjectServiceClient;
use pb::ref_service_client::RefServiceClient;
use pb::schema_service_client::SchemaServiceClient;
use pb::serving_service_client::ServingServiceClient;
use schemahub_api::schemahub_v1 as pb;
use schemahub_jj::{MemoryObjectDb, ObjectDb};
use schemahub_server::{build_core, build_router, config::Config};
use tokio::net::TcpListener;
use tonic::transport::Channel;

/// Start an in-process server with the default config (every repo defaults to
/// `main` protected + FULL compatibility). Returns its base URL.
pub async fn start_server() -> String {
    start_server_with(Config::default()).await
}

/// Start an in-process server with a custom `Config` (e.g. to set protected
/// bookmarks or a compatibility direction per repo).
pub async fn start_server_with(config: Config) -> String {
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db, &config);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        build_router(core, "memory")
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    format!("http://{addr}")
}

pub async fn connect(url: &str) -> Channel {
    Channel::from_shared(url.to_string())
        .unwrap()
        .connect()
        .await
        .unwrap()
}

/// A bundle of every service client over one shared channel.
pub struct Clients {
    pub schema: SchemaServiceClient<Channel>,
    pub explore: ExplorationServiceClient<Channel>,
    pub history: HistoryServiceClient<Channel>,
    pub refs: RefServiceClient<Channel>,
    pub codegen: CodegenServiceClient<Channel>,
    pub change: ChangeServiceClient<Channel>,
    pub serving: ServingServiceClient<Channel>,
    pub project: ProjectServiceClient<Channel>,
}

/// Connect all service clients to a running server.
pub async fn clients(url: &str) -> Clients {
    let ch = connect(url).await;
    Clients {
        schema: SchemaServiceClient::new(ch.clone()),
        explore: ExplorationServiceClient::new(ch.clone()),
        history: HistoryServiceClient::new(ch.clone()),
        refs: RefServiceClient::new(ch.clone()),
        codegen: CodegenServiceClient::new(ch.clone()),
        change: ChangeServiceClient::new(ch.clone()),
        serving: ServingServiceClient::new(ch.clone()),
        project: ProjectServiceClient::new(ch),
    }
}

// ── VersionRef helpers ────────────────────────────────────────────────────────

pub fn vref_branch(name: &str) -> pb::VersionRef {
    pb::VersionRef {
        r#ref: Some(pb::version_ref::Ref::Branch(name.to_string())),
    }
}

pub fn vref_tag(name: &str) -> pb::VersionRef {
    pb::VersionRef {
        r#ref: Some(pb::version_ref::Ref::Tag(name.to_string())),
    }
}

pub fn vref_commit(id: &str) -> pb::VersionRef {
    pb::VersionRef {
        r#ref: Some(pb::version_ref::Ref::Commit(id.to_string())),
    }
}

// ── Request helpers ───────────────────────────────────────────────────────────

/// Create a schema; returns the response (panics on transport/status error).
#[allow(clippy::too_many_arguments)]
pub async fn create_schema(
    c: &mut SchemaServiceClient<Channel>,
    project: &str,
    repo: &str,
    branch: &str,
    name: &str,
    format: pb::SchemaFormat,
    source: &str,
    idempotency_key: &str,
) -> pb::CreateSchemaResponse {
    c.create_schema(pb::CreateSchemaRequest {
        project: project.into(),
        repo: repo.into(),
        branch: branch.into(),
        schema_name: name.into(),
        format: format as i32,
        source: source.into(),
        base_revision: String::new(),
        idempotency_key: idempotency_key.into(),
    })
    .await
    .expect("create_schema")
    .into_inner()
}

/// Pull reconstructed source text for a schema at a ref.
pub async fn pull_source(
    c: &mut ExplorationServiceClient<Channel>,
    project: &str,
    repo: &str,
    schema_path: &str,
    at: pb::VersionRef,
) -> String {
    let r = c
        .get_schema_source(pb::GetSchemaSourceRequest {
            project: project.into(),
            repo: repo.into(),
            schema_path: schema_path.into(),
            at: Some(at),
        })
        .await
        .expect("get_schema_source")
        .into_inner();
    String::from_utf8(r.source).expect("source is utf-8")
}

/// List top-level declaration names for a schema at a ref.
pub async fn list_decl_names(
    c: &mut ExplorationServiceClient<Channel>,
    project: &str,
    repo: &str,
    schema_path: &str,
    at: pb::VersionRef,
) -> Vec<String> {
    c.list_declarations(pb::ListDeclarationsRequest {
        project: project.into(),
        repo: repo.into(),
        schema_path: schema_path.into(),
        at: Some(at),
        kind_filter: pb::DeclKind::Unspecified as i32,
    })
    .await
    .expect("list_declarations")
    .into_inner()
    .declarations
    .into_iter()
    .map(|d| d.name)
    .collect()
}
