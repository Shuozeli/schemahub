//! `AdminService` — GC, rebuild index, server config (design.md §8).

use std::sync::Arc;

use schemahub_core::{
    Core, DEFAULT_TTL_HOURS, MAX_DEPENDENCY_SCAN_REPOSITORIES, MAX_DEPENDENCY_SCAN_SCHEMAS,
};
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
        let (swept, idempotency_cleaned) = if r.dry_run {
            (0, 0)
        } else {
            let repos = vec![(r.project.clone(), r.repo.clone())];
            let swept = self.core.gc(&repos, token.as_deref()).map_err(to_status)? as u64;
            let idempotency_cleaned = self.core.prune_idempotency().map_err(to_status)? as u64;
            (swept, idempotency_cleaned)
        };
        Ok(Response::new(pb::RunGcResponse {
            objects_scanned: swept,
            objects_deleted: swept,
            bytes_reclaimed: 0,
            pending_entries_cleaned: 0,
            idempotency_entries_cleaned: idempotency_cleaned,
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
            transaction_timeout_secs: crate::TRANSACTION_TIMEOUT_SECS as u32,
            pending_cleanup_threshold_secs: 0,
            idempotency_ttl_hours: DEFAULT_TTL_HOURS,
            gc_age_threshold_hours: 0,
            storage_backend: self.storage_backend.clone(),
            server_version: crate::BUILD_VERSION.to_string(),
            max_dependency_scan_repositories: MAX_DEPENDENCY_SCAN_REPOSITORIES as u32,
            max_dependency_scan_schemas: MAX_DEPENDENCY_SCAN_SCHEMAS as u32,
        }))
    }

    async fn get_format_capabilities(
        &self,
        _request: Request<pb::GetFormatCapabilitiesRequest>,
    ) -> Result<Response<pb::GetFormatCapabilitiesResponse>, Status> {
        Ok(Response::new(format_capabilities()))
    }
}

pub(crate) fn format_capabilities() -> pb::GetFormatCapabilitiesResponse {
    use pb::CapabilityStatus::{Rejected, Supported};
    use pb::Language;

    let supported = |operation: &str| operation_capability(operation, Supported, true, true, "");
    let protobuf_operations = [
        "add_field",
        "remove_field",
        "rename_field",
        "change_field_type",
        "change_field_label",
        "reorder_fields",
        "add_message",
        "remove_message",
        "rename_message",
        "add_enum",
        "remove_enum",
        "add_enum_value",
        "remove_enum_value",
        "rename_enum_value",
        "add_service",
        "remove_service",
        "rename_service",
        "add_rpc",
        "remove_rpc",
        "rename_rpc",
        "change_rpc_type",
        "update_import",
    ]
    .into_iter()
    .map(supported)
    .collect();

    let mut flatbuffers_operations: Vec<_> = [
        "add_field",
        "deprecate_field",
        "rename_field",
        "change_field_type",
        "add_table",
        "remove_table",
        "rename_table",
        "add_enum",
        "remove_enum",
        "rename_enum",
        "add_enum_value",
        "remove_enum_value",
        "rename_enum_value",
        "add_union",
        "remove_union",
        "rename_union",
        "add_union_member",
        "remove_union_member",
        "update_import",
    ]
    .into_iter()
    .map(supported)
    .collect();
    flatbuffers_operations.extend([
        operation_capability(
            "remove_field",
            Rejected,
            false,
            false,
            "FlatBuffers field slots are wire identity; deprecate the field instead",
        ),
        operation_capability(
            "reorder_fields",
            Rejected,
            false,
            false,
            "FlatBuffers field order determines wire slots",
        ),
    ]);

    let openapi_operations = [
        "push_document",
        "add_path",
        "remove_path",
        "add_operation",
        "remove_operation",
        "add_component_schema",
        "remove_component_schema",
    ]
    .into_iter()
    .map(supported)
    .collect();

    pb::GetFormatCapabilitiesResponse {
        matrix_version: "1.0".to_string(),
        formats: vec![
            format_capability("protobuf", protobuf_operations, &[Language::Rust]),
            format_capability(
                "flatbuffers",
                flatbuffers_operations,
                &[Language::Rust, Language::Typescript],
            ),
            format_capability("openapi", openapi_operations, &[]),
        ],
    }
}

fn operation_capability(
    operation: &str,
    status: pb::CapabilityStatus,
    apply_mutation: bool,
    apply_transaction: bool,
    notes: &str,
) -> pb::FormatOperationCapability {
    pb::FormatOperationCapability {
        operation: operation.to_string(),
        status: status as i32,
        apply_mutation,
        apply_transaction,
        notes: notes.to_string(),
    }
}

fn format_capability(
    format_id: &str,
    operations: Vec<pb::FormatOperationCapability>,
    generated_code_languages: &[pb::Language],
) -> pb::FormatCapability {
    pb::FormatCapability {
        format_id: format_id.to_string(),
        operations,
        parse_and_print: true,
        compatibility: true,
        conflict_resolution: true,
        descriptor_artifact: true,
        generated_code_languages: generated_code_languages
            .iter()
            .map(|language| *language as i32)
            .collect(),
    }
}
