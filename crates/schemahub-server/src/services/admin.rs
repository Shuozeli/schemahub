use std::sync::Arc;

use schemahub_core::Core;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    GetServerConfigRequest, GetServerConfigResponse, RebuildIndexRequest, RebuildIndexResponse,
    RunGcRequest, RunGcResponse,
    admin_service_server::AdminService,
};

pub struct AdminServiceImpl {
    #[allow(dead_code)]
    core: Arc<Core>,
}

impl AdminServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl AdminService for AdminServiceImpl {
    async fn run_gc(
        &self,
        _request: Request<RunGcRequest>,
    ) -> Result<Response<RunGcResponse>, Status> {
        Err(Status::unimplemented("RunGC not yet implemented"))
    }

    async fn rebuild_index(
        &self,
        _request: Request<RebuildIndexRequest>,
    ) -> Result<Response<RebuildIndexResponse>, Status> {
        Err(Status::unimplemented("RebuildIndex not yet implemented"))
    }

    async fn get_server_config(
        &self,
        _request: Request<GetServerConfigRequest>,
    ) -> Result<Response<GetServerConfigResponse>, Status> {
        Ok(Response::new(GetServerConfigResponse {
            max_ops_per_transaction: 500,
            max_schemas_per_transaction: 20,
            transaction_timeout_secs: 30,
            pending_cleanup_threshold_secs: 300,
            idempotency_ttl_hours: 24,
            gc_age_threshold_hours: 168,
            storage_backend: "redb".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }
}
