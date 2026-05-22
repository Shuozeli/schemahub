use std::collections::HashSet;
use std::sync::Arc;

use schemahub_core::{Core, objects};
use schemahub_core::objects::{KIND_BLOB, KIND_SUBTREE};
use schemahub_storage::keys;
use schemahub_types::Hash;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    GetServerConfigRequest, GetServerConfigResponse, RebuildIndexRequest, RebuildIndexResponse,
    RunGcRequest, RunGcResponse,
    admin_service_server::AdminService,
};

use crate::error::core_to_status;

pub struct AdminServiceImpl {
    core: Arc<Core>,
}

impl AdminServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

/// Walk the object graph reachable from a commit hash, collecting all live hashes.
fn collect_live_objects(
    core: &Core,
    root_hash: &Hash,
    live: &mut HashSet<String>,
) {
    let hex = root_hash.to_hex();
    if live.contains(&hex) {
        return; // already visited
    }
    live.insert(hex.clone());

    // Read the commit.
    let commit = match core.storage.read_object(root_hash) {
        Ok(Some(data)) => match objects::decode_commit(&data) {
            Ok(c) => c,
            Err(_) => return,
        },
        _ => return,
    };

    // Recurse into parent commits.
    for parent_hex in &commit.parent_hashes {
        if let Ok(parent_hash) = Hash::from_hex(parent_hex) {
            collect_live_objects(core, &parent_hash, live);
        }
    }

    // Walk root tree.
    collect_tree_objects(core, &commit.tree_hash, live);
}

fn collect_tree_objects(core: &Core, tree_hex: &str, live: &mut HashSet<String>) {
    if live.contains(tree_hex) {
        return;
    }
    live.insert(tree_hex.to_string());

    let tree_hash = match Hash::from_hex(tree_hex) {
        Ok(h) => h,
        Err(_) => return,
    };
    let tree_data = match core.storage.read_object(&tree_hash) {
        Ok(Some(d)) => d,
        _ => return,
    };
    let tree = match objects::decode_tree(&tree_data) {
        Ok(t) => t,
        Err(_) => return,
    };

    for entry in &tree.entries {
        if live.contains(&entry.hash) {
            continue;
        }
        live.insert(entry.hash.clone());
        if entry.kind == KIND_SUBTREE {
            collect_tree_objects(core, &entry.hash, live);
        }
        // Blobs: already inserted above, nothing more to recurse into.
    }
}

#[tonic::async_trait]
impl AdminService for AdminServiceImpl {
    async fn run_gc(
        &self,
        request: Request<RunGcRequest>,
    ) -> Result<Response<RunGcResponse>, Status> {
        let req = request.into_inner();
        let storage = self.core.storage.as_ref();

        // ── Collect live objects ───────────────────────────────────────────────
        let mut live: HashSet<String> = HashSet::new();

        // Collect from all pending/ entries.
        let pending_entries = storage
            .scan_prefix(keys::PENDING_PREFIX)
            .map_err(|e| Status::internal(e.to_string()))?;
        for (_, value) in &pending_entries {
            if let Ok(hex) = std::str::from_utf8(value) {
                live.insert(hex.trim().to_string());
            }
        }

        // Walk all branch refs.
        let branch_scan_prefix = if !req.project.is_empty() && !req.repo.is_empty() {
            keys::branch_refs_prefix(&req.project, &req.repo)
        } else {
            "refs/".to_string()
        };

        let ref_entries = storage
            .scan_prefix(&branch_scan_prefix)
            .map_err(|e| Status::internal(e.to_string()))?;

        for (_, value) in &ref_entries {
            if let Ok(hex) = std::str::from_utf8(value) {
                if let Ok(hash) = Hash::from_hex(hex.trim()) {
                    collect_live_objects(&self.core, &hash, &mut live);
                }
            }
        }

        // ── Scan all stored objects ────────────────────────────────────────────
        let all_objects = storage
            .list_all_objects()
            .map_err(|e| Status::internal(e.to_string()))?;

        let objects_scanned = all_objects.len() as u64;
        let mut objects_deleted = 0u64;
        let mut bytes_reclaimed = 0u64;

        for hash in &all_objects {
            if !live.contains(&hash.to_hex()) {
                if !req.dry_run {
                    if let Ok(Some(data)) = storage.read_object(hash) {
                        bytes_reclaimed += data.len() as u64;
                        // Redb doesn't have a delete-object API, but we can
                        // use the generic delete on the object key.
                        let _ = storage.delete(&keys::object_key(hash));
                    }
                } else {
                    if let Ok(Some(data)) = storage.read_object(hash) {
                        bytes_reclaimed += data.len() as u64;
                    }
                }
                objects_deleted += 1;
            }
        }

        // Clean up expired pending/ entries (already completed mutations).
        let pending_entries_cleaned = if !req.dry_run {
            storage
                .delete_prefix(keys::PENDING_PREFIX)
                .map_err(|e| Status::internal(e.to_string()))?
        } else {
            pending_entries.len() as u64
        };

        // Clean up expired idempotency entries.
        let idempotency_entries_cleaned = schemahub_core::mutation::idempotency::gc_idempotency_entries(
            storage,
            req.dry_run,
        )
        .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(RunGcResponse {
            objects_scanned,
            objects_deleted,
            bytes_reclaimed,
            pending_entries_cleaned,
            idempotency_entries_cleaned,
        }))
    }

    async fn rebuild_index(
        &self,
        request: Request<RebuildIndexRequest>,
    ) -> Result<Response<RebuildIndexResponse>, Status> {
        let req = request.into_inner();

        if req.project.is_empty() || req.repo.is_empty() {
            return Err(Status::invalid_argument(
                "RebuildIndex requires both project and repo to be specified",
            ));
        }

        // Resolve HEAD of default branch.
        let config = self.core
            .get_repo_config(&req.project, &req.repo)
            .map_err(core_to_status)?;
        let head = match self.core.get_branch_head(&req.project, &req.repo, &config.default_branch) {
            Ok(h) => h,
            Err(_) => return Ok(Response::new(RebuildIndexResponse {
                blobs_scanned: 0,
                index_entries_written: 0,
                deps_entries_written: 0,
            })),
        };

        // List all schemas.
        let schemas = self.core
            .list_schemas(&req.project, &req.repo, &head.to_hex())
            .map_err(core_to_status)?;

        let mut blobs_scanned = 0u64;
        let mut index_entries_written = 0u64;

        for (schema_name, _) in schemas {
            // Detect format.
            let format_id = match schemahub_core::detect_format_from_name(&schema_name) {
                Some(f) => f,
                None => continue,
            };
            let plugin = match self.core.plugins.get(&format_id) {
                Some(p) => p,
                None => continue,
            };

            // List declarations in this schema.
            let decls = match self.core.list_declarations(
                &req.project, &req.repo, &schema_name, &head.to_hex(),
            ) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let _ = plugin; // plugin used to determine format above

            // Write search index entries: search/{decl_name}/{project}/{repo}/{schema}
            for decl in &decls {
                blobs_scanned += 1;
                let search_key = keys::search_key(&decl.name, &req.project, &req.repo, &schema_name);
                let _ = self.core.storage.put(&search_key, schema_name.as_bytes());
                index_entries_written += 1;
            }
        }

        Ok(Response::new(RebuildIndexResponse {
            blobs_scanned,
            index_entries_written,
            deps_entries_written: 0,
        }))
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
