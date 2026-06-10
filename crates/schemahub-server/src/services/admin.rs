//! `AdminService` — GC, rebuild index, server config (design.md §8).

use std::sync::Arc;

use schemahub_core::Core;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::admin_service_server::AdminService;

use crate::error::to_status;
use crate::services::token_from;

pub struct AdminHandler {
    core: Arc<Core>,
    /// The configured storage backend id (`"redb"` or `"postgres"`), surfaced
    /// by `GetServerConfig`. Threaded in from the composition root so the
    /// response reflects what the binary actually opened, not a hard-coded
    /// guess.
    storage_backend: String,
}

impl AdminHandler {
    pub fn new(core: Arc<Core>, storage_backend: impl Into<String>) -> Self {
        Self {
            core,
            storage_backend: storage_backend.into(),
        }
    }
}

#[tonic::async_trait]
impl AdminService for AdminHandler {
    async fn run_gc(
        &self,
        request: Request<pb::RunGcRequest>,
    ) -> Result<Response<pb::RunGcResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        if r.project.is_empty() || r.repo.is_empty() {
            return Err(Status::invalid_argument(
                "GC requires project and repo (global GC is v2)",
            ));
        }
        // Dry-run is not separately modeled by the JJ layer GC; honor it by skipping.
        let swept = if r.dry_run {
            0
        } else {
            let repos = vec![(r.project.clone(), r.repo.clone())];
            self.core.gc(&repos, token.as_deref()).map_err(to_status)? as u64
        };
        Ok(Response::new(pb::RunGcResponse {
            objects_scanned: swept,
            objects_deleted: swept,
            bytes_reclaimed: 0,
            pending_entries_cleaned: 0,
            idempotency_entries_cleaned: 0,
        }))
    }

    async fn rebuild_index(
        &self,
        request: Request<pb::RebuildIndexRequest>,
    ) -> Result<Response<pb::RebuildIndexResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        self.core
            .rebuild_index(&r.project, &r.repo, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::RebuildIndexResponse {
            blobs_scanned: 0,
            index_entries_written: 0,
            deps_entries_written: 0,
        }))
    }

    async fn get_server_config(
        &self,
        _request: Request<pb::GetServerConfigRequest>,
    ) -> Result<Response<pb::GetServerConfigResponse>, Status> {
        Ok(Response::new(pb::GetServerConfigResponse {
            max_ops_per_transaction: 100,
            max_schemas_per_transaction: 20,
            transaction_timeout_secs: 30,
            pending_cleanup_threshold_secs: 0,
            idempotency_ttl_hours: 0,
            gc_age_threshold_hours: 0,
            storage_backend: self.storage_backend.clone(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }
}
