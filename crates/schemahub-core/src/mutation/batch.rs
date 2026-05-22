use std::collections::HashMap;

use schemahub_storage::{StorageBackend, StorageOp, keys};
use schemahub_types::{
    Action, AuthnProvider, AuthzPolicy, Blob, CompatibilityRules, Mutation, ResourcePath,
};
use uuid::Uuid;

use crate::error::CoreError;
use crate::mutation::idempotency::{check_idempotency, store_idempotency, IdempotencyResult};
use crate::objects::{
    CommitObject, TreeEntryProto, TreeObject, encode_commit, encode_tree, hash_of_bytes, unix_now,
    KIND_BLOB, KIND_SUBTREE,
};
use crate::plugin_registry::PluginRegistry;
use crate::repo_config::RepoConfig;
use crate::version_control::{
    branch::get_branch_head,
    commit::read_commit,
    tree::{root_tree_from_commit, schema_tree_from_root},
};

/// Request to apply a batch of mutations atomically to a branch.
pub struct BatchMutateRequest {
    pub project: String,
    pub repo: String,
    pub branch: String,
    /// Commit hash hex to use as the expected current HEAD.
    pub base_revision: String,
    pub idempotency_key: String,
    pub force: bool,
    pub mutations: Vec<Mutation>,
    pub token: Option<String>,
    pub author: String,
}

/// Apply a sequence of mutations atomically across multiple schemas on a branch.
/// All mutations must target the same format.
/// Returns the new commit hash as a hex string.
pub fn apply_mutations(
    storage: &dyn StorageBackend,
    plugins: &PluginRegistry,
    authn: &dyn AuthnProvider,
    authz: &dyn AuthzPolicy,
    config: &RepoConfig,
    req: &BatchMutateRequest,
) -> Result<String, CoreError> {
    // ── Step 1: Idempotency check ─────────────────────────────────────────────
    if let Some(existing) = check_idempotency(storage, &req.project, &req.repo, &req.idempotency_key)? {
        match existing {
            IdempotencyResult::Success { commit_hash } => return Ok(commit_hash),
            IdempotencyResult::Error { code: _, message } => {
                return Err(CoreError::InvalidArgument(format!(
                    "prior idempotent call failed: {message}"
                )));
            }
        }
    }

    // ── Step 2: AuthN ─────────────────────────────────────────────────────────
    let identity = authn
        .identify(req.token.as_deref())
        .map_err(|e| CoreError::Unauthenticated(e.to_string()))?;

    // ── Step 3: AuthZ ─────────────────────────────────────────────────────────
    let resource = ResourcePath::repo(&req.project, &req.repo);
    let action = if req.force { Action::Force } else { Action::Write };
    authz
        .check(&identity, action, &resource)
        .map_err(|e| CoreError::PermissionDenied(e.to_string()))?;

    // ── Step 4: Validate mutations share the same format ──────────────────────
    if req.mutations.is_empty() {
        return Err(CoreError::InvalidArgument("batch must contain at least one mutation".to_string()));
    }
    let format_id = &req.mutations[0].format_id;
    for m in &req.mutations {
        if &m.format_id != format_id {
            return Err(CoreError::InvalidArgument(format!(
                "all mutations must use the same format; expected '{}', got '{}'",
                format_id, m.format_id
            )));
        }
    }

    // ── Step 5: CAS check — get current HEAD ──────────────────────────────────
    let current_head = get_branch_head(storage, &req.project, &req.repo, &req.branch)?;
    if !req.base_revision.is_empty() {
        let provided_base = schemahub_types::Hash::from_hex(&req.base_revision)
            .map_err(|_| CoreError::InvalidArgument(format!("base_revision is not a valid hash: {}", req.base_revision)))?;
        if current_head != provided_base {
            return Err(CoreError::Conflict {
                current_head: current_head.to_hex(),
                provided_base: req.base_revision.clone(),
            });
        }
    }

    // ── Step 6: Load current root tree ────────────────────────────────────────
    let head_commit = read_commit(storage, &current_head)?;
    let (_, root_tree) = root_tree_from_commit(storage, &head_commit)?;

    // ── Step 7: Find the plugin ───────────────────────────────────────────────
    let plugin = plugins
        .get(format_id)
        .ok_or_else(|| CoreError::InvalidArgument(format!("unknown format_id: {format_id}")))?;

    // ── Step 8: Group mutations by schema_name ────────────────────────────────
    // Map: schema_name → Vec<Mutation>
    let mut by_schema: HashMap<String, Vec<Mutation>> = HashMap::new();
    for m in &req.mutations {
        by_schema
            .entry(m.schema_path.schema_name.clone())
            .or_default()
            .push(m.clone());
    }

    // ── Step 9: For each schema, load __schema__ blob and apply mutations ───────
    let mut write_ops: Vec<StorageOp> = Vec::new();

    // Build new root entries, start from existing.
    let mut new_root_entries: Vec<TreeEntryProto> = root_tree.entries.clone();

    // Load whole-schema blobs per schema.
    let plugin_blobs: HashMap<schemahub_types::SchemaPath, Blob> = {
        let mut map = HashMap::new();
        for (schema_name, _) in &by_schema {
            let schema_blob = match schema_tree_from_root(storage, &root_tree, schema_name) {
                Ok((_, schema_tree)) => {
                    // Find __schema__ entry
                    let entry = schema_tree.entries.iter()
                        .find(|e| e.name == "__schema__" && e.kind == KIND_BLOB);
                    match entry {
                        Some(e) => {
                            let hash = schemahub_types::Hash::from_hex(&e.hash).map_err(|_| {
                                CoreError::ObjectCorrupted(format!("__schema__ hash invalid in {schema_name}"))
                            })?;
                            let data = storage.read_object(&hash)?
                                .ok_or_else(|| CoreError::NotFound(format!("blob {} missing", hash.to_hex())))?;
                            Blob::new(data)
                        }
                        None => Blob::new(vec![]),
                    }
                }
                Err(CoreError::NotFound(_)) => Blob::new(vec![]),
                Err(e) => return Err(e),
            };
            let schema_path = schemahub_types::SchemaPath::new(
                req.project.clone(), req.repo.clone(), schema_name.clone()
            );
            map.insert(schema_path, schema_blob);
        }
        map
    };

    // Collect old blobs for compatibility checking (copy before mutation)
    let old_blobs = plugin_blobs.clone();

    // Apply all mutations via the plugin
    let updated_blobs = plugin.apply_mutations(&plugin_blobs, &req.mutations)
        .map_err(CoreError::MutationError)?;

    // Write back each updated schema blob as __schema__ and build new tree entries
    for (schema_path, new_blob) in &updated_blobs {
        let schema_name = &schema_path.schema_name;

        // Compatibility check if on protected branch
        if config.is_protected(&req.branch) && !req.force {
            if let Some(old_blob) = old_blobs.get(schema_path) {
                let rules = CompatibilityRules { direction: config.compatibility_direction };
                plugin.check_compatibility(old_blob, new_blob, &rules)
                    .map_err(CoreError::CompatibilityViolation)?;
            }
        }

        let new_blob_data = new_blob.as_bytes().to_vec();
        let new_blob_hash = schemahub_types::Hash::of(&new_blob_data);
        write_ops.push(StorageOp::Put {
            key: keys::object_key(&new_blob_hash),
            value: new_blob_data,
        });

        let schema_tree = TreeObject {
            blob_version: 1,
            entries: vec![TreeEntryProto {
                name: "__schema__".to_string(),
                kind: KIND_BLOB,
                hash: new_blob_hash.to_hex(),
            }],
            created_at_unix: unix_now(),
        };
        let schema_tree_encoded = encode_tree(&schema_tree);
        let schema_tree_hash = hash_of_bytes(&schema_tree_encoded);
        write_ops.push(StorageOp::Put {
            key: keys::object_key(&schema_tree_hash),
            value: schema_tree_encoded,
        });

        new_root_entries.retain(|e| !(e.name == *schema_name && e.kind == KIND_SUBTREE));
        new_root_entries.push(TreeEntryProto {
            name: schema_name.clone(),
            kind: KIND_SUBTREE,
            hash: schema_tree_hash.to_hex(),
        });
    }

    // ── Step 10: Build new root tree ──────────────────────────────────────────
    new_root_entries.sort_by(|a, b| a.name.cmp(&b.name));
    let new_root_tree = TreeObject {
        blob_version: 1,
        entries: new_root_entries,
        created_at_unix: unix_now(),
    };
    let new_root_tree_encoded = encode_tree(&new_root_tree);
    let new_root_tree_hash = hash_of_bytes(&new_root_tree_encoded);
    write_ops.push(StorageOp::Put {
        key: keys::object_key(&new_root_tree_hash),
        value: new_root_tree_encoded,
    });

    // ── Step 11: Build new commit ─────────────────────────────────────────────
    let mutation_id = Uuid::new_v4().to_string();
    let pending_key = keys::pending_key(&mutation_id);
    storage.put(&pending_key, new_root_tree_hash.to_hex().as_bytes())?;

    let now = unix_now();
    let new_commit = CommitObject {
        blob_version: 1,
        tree_hash: new_root_tree_hash.to_hex(),
        parent_hashes: vec![current_head.to_hex()],
        timestamp_unix: now,
        author: req.author.clone(),
        message: format!("batch mutation: {} ops", req.mutations.len()),
        force: req.force,
        format_id: format_id.clone(),
        created_at_unix: now,
    };
    let new_commit_encoded = encode_commit(&new_commit);
    let new_commit_hash = hash_of_bytes(&new_commit_encoded);
    write_ops.push(StorageOp::Put {
        key: keys::object_key(&new_commit_hash),
        value: new_commit_encoded,
    });
    write_ops.push(StorageOp::Delete { key: pending_key });

    // ── Step 12: Execute transaction ──────────────────────────────────────────
    storage.write_transaction(write_ops)?;

    // ── Step 13: CAS branch ref ───────────────────────────────────────────────
    let branch_ref_key = keys::branch_ref_key(&req.project, &req.repo, &req.branch);
    let swapped = storage.compare_and_set_ref(&branch_ref_key, &current_head, &new_commit_hash)?;
    if !swapped {
        let actual_head = storage
            .get_ref(&branch_ref_key)?
            .map(|h| h.to_hex())
            .unwrap_or_else(|| "<unknown>".to_string());
        return Err(CoreError::Conflict {
            current_head: actual_head,
            provided_base: current_head.to_hex(),
        });
    }

    // ── Step 14: Store idempotency result ─────────────────────────────────────
    let commit_hex = new_commit_hash.to_hex();
    store_idempotency(
        storage,
        &req.project,
        &req.repo,
        &req.idempotency_key,
        IdempotencyResult::Success { commit_hash: commit_hex.clone() },
        24,
    )?;

    Ok(commit_hex)
}
