use schemahub_storage::{StorageBackend, StorageOp, keys};
use schemahub_types::{
    Action, AuthnProvider, AuthzPolicy, Blob, CompatibilityRules, Mutation, ResourcePath,
};
use uuid::Uuid;

use crate::error::CoreError;
use crate::mutation::idempotency::{
    check_idempotency, store_idempotency, IdempotencyResult,
};
use crate::objects::{encode_commit, hash_of_bytes, unix_now};
use crate::plugin_registry::PluginRegistry;
use crate::repo_config::RepoConfig;
use crate::version_control::{
    branch::get_branch_head,
    commit::read_commit,
    tree::{
        blob_hash_from_schema_tree, root_tree_from_commit, schema_tree_from_root,
    },
};
use crate::objects::{CommitObject, TreeObject, TreeEntryProto, encode_tree, unix_now as now_unix, KIND_BLOB, KIND_SUBTREE};

/// Request to apply a single mutation to a branch.
pub struct MutateRequest {
    pub project: String,
    pub repo: String,
    pub branch: String,
    /// Commit hash hex to use as the expected current HEAD.
    /// If empty, skips the CAS check and uses the current HEAD as parent.
    pub base_revision: String,
    /// Optional idempotency key. If non-empty and a result already exists, return it.
    pub idempotency_key: String,
    pub force: bool,
    pub mutation: Mutation,
    pub token: Option<String>,
    pub author: String,
}

/// Apply a single mutation to a branch, following the 11-step flow.
/// Returns the new commit hash as a hex string.
pub fn apply_mutation(
    storage: &dyn StorageBackend,
    plugins: &PluginRegistry,
    authn: &dyn AuthnProvider,
    authz: &dyn AuthzPolicy,
    config: &RepoConfig,
    req: &MutateRequest,
) -> Result<String, CoreError> {
    // ── Step 1: Check idempotency key ────────────────────────────────────────
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

    // ── Step 2: AuthN ────────────────────────────────────────────────────────
    let identity = authn
        .identify(req.token.as_deref())
        .map_err(|e| CoreError::Unauthenticated(e.to_string()))?;

    // ── Step 3: AuthZ ────────────────────────────────────────────────────────
    let resource = ResourcePath::repo(&req.project, &req.repo);
    let action = if req.force { Action::Force } else { Action::Write };
    authz
        .check(&identity, action, &resource)
        .map_err(|e| CoreError::PermissionDenied(e.to_string()))?;

    // ── Step 4: CAS check — get current HEAD ─────────────────────────────────
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

    // ── Step 5: Load current root tree from HEAD commit ───────────────────────
    let head_commit = read_commit(storage, &current_head)?;
    let (_, root_tree) = root_tree_from_commit(storage, &head_commit)?;

    // ── Step 6: Navigate tree: root_tree -> schema sub-tree -> __schema__ blob
    let schema_name = &req.mutation.schema_path.schema_name;
    let _decl_name = &req.mutation.declaration_name;

    // Get or create the schema sub-tree.
    // The whole schema is stored as a single "__schema__" blob (ParseEnvelope).
    let (schema_tree_result, old_blob) = match schema_tree_from_root(storage, &root_tree, schema_name) {
        Ok((_, schema_tree)) => {
            // Load the whole-schema ParseEnvelope blob stored under "__schema__".
            let old_blob = match blob_hash_from_schema_tree(&schema_tree, "__schema__") {
                Ok(blob_hash) => {
                    let blob_data = storage
                        .read_object(&blob_hash)?
                        .ok_or_else(|| CoreError::NotFound(format!("blob {} not found", blob_hash.to_hex())))?;
                    Blob::new(blob_data)
                }
                // Schema blob doesn't exist yet — pass empty blob.
                Err(CoreError::NotFound(_)) => Blob::new(vec![]),
                Err(e) => return Err(e),
            };
            (Some(schema_tree), old_blob)
        }
        // Schema sub-tree doesn't exist yet — new schema.
        Err(CoreError::NotFound(_)) => (None, Blob::new(vec![])),
        Err(e) => return Err(e),
    };

    // ── Step 7: Find the plugin by format_id ─────────────────────────────────
    let plugin = plugins
        .get(&req.mutation.format_id)
        .ok_or_else(|| CoreError::InvalidArgument(format!("unknown format_id: {}", req.mutation.format_id)))?;

    // ── Step 8: Call plugin.apply_mutation(blob, &req.mutation) ──────────────
    let new_blob = plugin.apply_mutation(&old_blob, &req.mutation)?;

    // ── Step 9: Compatibility check if branch is protected and not force ──────
    if config.is_protected(&req.branch) && !req.force {
        let rules = CompatibilityRules {
            direction: config.compatibility_direction,
        };
        plugin
            .check_compatibility(&old_blob, &new_blob, &rules)
            .map_err(CoreError::CompatibilityViolation)?;
    }

    // ── Step 10: Generate a mutation_id ──────────────────────────────────────
    let mutation_id = Uuid::new_v4().to_string();

    // ── Step 11: Write pending/<mutation_id> pointing to new blob ────────────
    let new_blob_hash = schemahub_types::Hash::of(new_blob.as_bytes());
    let pending_key = keys::pending_key(&mutation_id);
    storage.put(&pending_key, new_blob_hash.to_hex().as_bytes())?;

    // ── Step 12: Build and execute the KV transaction ────────────────────────
    // 12a: Encode new blob.
    let new_blob_data = new_blob.as_bytes().to_vec();

    // 12b: Rebuild the schema tree with the updated __schema__ blob.
    let schema_tree_for_update = schema_tree_result.unwrap_or_else(|| TreeObject {
        blob_version: 1,
        entries: vec![],
        created_at_unix: now_unix(),
    });
    let new_schema_tree_encoded = {
        // Replace the __schema__ entry with the new blob hash.
        let mut new_entries: Vec<TreeEntryProto> = schema_tree_for_update
            .entries
            .iter()
            .filter(|e| e.name != "__schema__")
            .cloned()
            .collect();
        new_entries.push(TreeEntryProto {
            name: "__schema__".to_string(),
            kind: KIND_BLOB,
            hash: new_blob_hash.to_hex(),
        });
        new_entries.sort_by(|a, b| a.name.cmp(&b.name));
        let new_schema_tree = TreeObject {
            blob_version: 1,
            entries: new_entries,
            created_at_unix: now_unix(),
        };
        encode_tree(&new_schema_tree)
    };
    let new_schema_tree_hash = hash_of_bytes(&new_schema_tree_encoded);

    // 12c: Rebuild the root tree with the new schema sub-tree.
    let new_root_tree_encoded = {
        let mut new_entries: Vec<TreeEntryProto> = root_tree
            .entries
            .iter()
            .filter(|e| e.name != schema_name.as_str())
            .cloned()
            .collect();
        new_entries.push(TreeEntryProto {
            name: schema_name.clone(),
            kind: KIND_SUBTREE,
            hash: new_schema_tree_hash.to_hex(),
        });
        new_entries.sort_by(|a, b| a.name.cmp(&b.name));
        let new_root_tree = TreeObject {
            blob_version: 1,
            entries: new_entries,
            created_at_unix: now_unix(),
        };
        encode_tree(&new_root_tree)
    };
    let new_root_tree_hash = hash_of_bytes(&new_root_tree_encoded);

    // 12d: Build the new commit object.
    let now = unix_now();
    let new_commit = CommitObject {
        blob_version: 1,
        tree_hash: new_root_tree_hash.to_hex(),
        parent_hashes: vec![current_head.to_hex()],
        timestamp_unix: now,
        author: req.author.clone(),
        message: format!("mutation: {}", req.mutation.format_id),
        force: req.force,
        format_id: req.mutation.format_id.clone(),
        created_at_unix: now,
    };
    let new_commit_encoded = encode_commit(&new_commit);
    let new_commit_hash = hash_of_bytes(&new_commit_encoded);

    // 12e & beyond: Build the transaction ops.
    let branch_ref_key = keys::branch_ref_key(&req.project, &req.repo, &req.branch);

    use schemahub_storage::keys::object_key;
    let ops = vec![
        // Write new blob.
        StorageOp::Put {
            key: object_key(&new_blob_hash),
            value: new_blob_data,
        },
        // Write new schema tree.
        StorageOp::Put {
            key: object_key(&new_schema_tree_hash),
            value: new_schema_tree_encoded,
        },
        // Write new root tree.
        StorageOp::Put {
            key: object_key(&new_root_tree_hash),
            value: new_root_tree_encoded,
        },
        // Write new commit.
        StorageOp::Put {
            key: object_key(&new_commit_hash),
            value: new_commit_encoded,
        },
        // Delete pending marker.
        StorageOp::Delete {
            key: pending_key.clone(),
        },
    ];

    // Execute the transaction.
    storage.write_transaction(ops)?;

    // 12e: CAS-update the branch ref.
    // Note: the write_transaction doesn't include the ref update because
    // compare_and_set_ref needs its own atomic operation on the ref table.
    // We use a separate CAS after the objects are written.
    let swapped = storage.compare_and_set_ref(&branch_ref_key, &current_head, &new_commit_hash)?;
    if !swapped {
        // Concurrent write happened — fetch actual current HEAD for the error.
        let actual_head = storage
            .get_ref(&branch_ref_key)?
            .map(|h| h.to_hex())
            .unwrap_or_else(|| "<unknown>".to_string());
        return Err(CoreError::Conflict {
            current_head: actual_head,
            provided_base: current_head.to_hex(),
        });
    }

    // ── Step 13: Store idempotency result (24h TTL) ───────────────────────────
    let commit_hex = new_commit_hash.to_hex();
    store_idempotency(
        storage,
        &req.project,
        &req.repo,
        &req.idempotency_key,
        IdempotencyResult::Success { commit_hash: commit_hex.clone() },
        24,
    )?;

    // ── Step 14: Update search index (best-effort) ────────────────────────────
    if let Ok(old_decls) = plugin.list_declarations(&old_blob) {
        for decl in &old_decls {
            let key = keys::search_key(&decl.name, &req.project, &req.repo, schema_name);
            let _ = storage.delete(&key);
        }
    }
    if let Ok(new_decls) = plugin.list_declarations(&new_blob) {
        for decl in &new_decls {
            let key = keys::search_key(&decl.name, &req.project, &req.repo, schema_name);
            let _ = storage.put(&key, schema_name.as_bytes());
        }
    }

    // ── Step 15: Return new commit hash ──────────────────────────────────────
    Ok(commit_hex)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use bytes::Bytes;
    use schemahub_storage::RedbBackend;
    use schemahub_types::{
        Blob, CompatibilityRules, CompatibilityViolation,
        DeclDetail, DeclSummary, DescriptorError, DiffError, FormatPlugin,
        Import, Language, Mutation, MutationError, NoopAuthn, NoopAuthz, ParseError,
        PrintError, ReadError, SchemaChange, SchemaPath,
        errors::CodegenError,
    };

    use crate::objects::{encode_tree, TreeObject, hash_of_bytes};
    use crate::objects::unix_now;
    use crate::plugin_registry::PluginRegistry;
    use crate::repo_config::RepoConfig;
    use crate::version_control::branch::set_branch_head;
    use crate::version_control::commit::create_commit;

    fn ephemeral_storage() -> RedbBackend {
        // Use a UUID path so stale files from previous test runs never interfere.
        let path = format!("/tmp/schemahub-core-test-{}.redb", Uuid::new_v4());
        RedbBackend::open(&path).unwrap()
    }

    /// A mock plugin that returns the input blob unchanged.
    struct PassthroughPlugin;

    impl FormatPlugin for PassthroughPlugin {
        fn format_id(&self) -> &'static str { "passthrough" }

        fn parse(&self, source: &str) -> Result<Blob, ParseError> {
            Ok(Blob::new(source.as_bytes().to_vec()))
        }

        fn print(&self, blob: &Blob) -> Result<String, PrintError> {
            Ok(String::from_utf8_lossy(blob.as_bytes()).to_string())
        }

        fn diff(&self, _old: &Blob, _new: &Blob) -> Result<Vec<SchemaChange>, DiffError> {
            Ok(vec![])
        }

        fn apply_mutation(&self, blob: &Blob, _mutation: &Mutation) -> Result<Blob, MutationError> {
            // Return the blob unchanged (or a trivially modified version).
            Ok(blob.clone())
        }

        fn apply_mutations(
            &self,
            blobs: &HashMap<SchemaPath, Blob>,
            _mutations: &[Mutation],
        ) -> Result<HashMap<SchemaPath, Blob>, MutationError> {
            Ok(blobs.clone())
        }

        fn check_compatibility(
            &self,
            _old: &Blob,
            _new: &Blob,
            _rules: &CompatibilityRules,
        ) -> Result<(), Vec<CompatibilityViolation>> {
            Ok(())
        }

        fn list_declarations(&self, _blob: &Blob) -> Result<Vec<DeclSummary>, ReadError> {
            Ok(vec![])
        }

        fn get_declaration(&self, _blob: &Blob, _name: &str) -> Result<DeclDetail, ReadError> {
            Err(ReadError::NotFound("none".to_string()))
        }

        fn imports(&self, _blob: &Blob) -> Result<Vec<Import>, ReadError> {
            Ok(vec![])
        }

        fn generate_descriptors(
            &self,
            _blobs: &HashMap<SchemaPath, Blob>,
        ) -> Result<Bytes, DescriptorError> {
            Ok(Bytes::new())
        }

        fn generate_code(
            &self,
            _blobs: &HashMap<SchemaPath, Blob>,
            language: Language,
        ) -> Result<String, CodegenError> {
            Err(CodegenError::UnsupportedLanguage(language))
        }
    }

    /// Set up an ephemeral storage with an initial commit and branch.
    fn setup_storage() -> (RedbBackend, String) {
        let storage = ephemeral_storage();

        // Create an empty root tree.
        let root_tree = TreeObject {
            blob_version: 1,
            entries: vec![],
            created_at_unix: unix_now(),
        };
        let root_tree_encoded = encode_tree(&root_tree);
        let root_tree_hash = hash_of_bytes(&root_tree_encoded);
        storage.write_object(&root_tree_hash, &root_tree_encoded).unwrap();

        // Create initial commit.
        let initial_commit_hash = create_commit(
            &storage,
            root_tree_hash,
            vec![],
            "system",
            "initial commit",
            false,
            "passthrough",
        )
        .unwrap();

        // Create the main branch pointing at the initial commit.
        set_branch_head(&storage, "myproject", "myrepo", "main", &initial_commit_hash).unwrap();

        (storage, initial_commit_hash.to_hex())
    }

    fn make_plugins() -> PluginRegistry {
        let mut plugins = PluginRegistry::new();
        plugins.register(Arc::new(PassthroughPlugin));
        plugins
    }

    fn make_config() -> RepoConfig {
        let mut config = RepoConfig::default();
        config.protected_branches = vec![]; // no protection for tests
        config
    }

    fn make_mutation() -> Mutation {
        Mutation {
            schema_path: SchemaPath::new("myproject", "myrepo", "user"),
            format_id: "passthrough".to_string(),
            declaration_name: "UserMessage".to_string(),
            operation: Bytes::new(),
        }
    }

    #[test]
    fn test_happy_path() {
        let (storage, initial_head) = setup_storage();
        let plugins = make_plugins();
        let authn = NoopAuthn;
        let authz = NoopAuthz;
        let config = make_config();

        let req = MutateRequest {
            project: "myproject".to_string(),
            repo: "myrepo".to_string(),
            branch: "main".to_string(),
            base_revision: initial_head.clone(),
            idempotency_key: "key-happy".to_string(),
            force: false,
            mutation: make_mutation(),
            token: None,
            author: "alice".to_string(),
        };

        let result = apply_mutation(&storage, &plugins, &authn, &authz, &config, &req);
        assert!(result.is_ok(), "happy path should succeed: {:?}", result);

        let new_head = result.unwrap();
        assert_ne!(new_head, initial_head, "new commit hash should differ from initial");

        // Verify branch was updated.
        let current_head = crate::version_control::branch::get_branch_head(
            &storage, "myproject", "myrepo", "main",
        )
        .unwrap();
        assert_eq!(current_head.to_hex(), new_head);
    }

    #[test]
    fn test_idempotency() {
        let (storage, initial_head) = setup_storage();
        let plugins = make_plugins();
        let authn = NoopAuthn;
        let authz = NoopAuthz;
        let config = make_config();

        let req = MutateRequest {
            project: "myproject".to_string(),
            repo: "myrepo".to_string(),
            branch: "main".to_string(),
            base_revision: initial_head.clone(),
            idempotency_key: "key-idempotent".to_string(),
            force: false,
            mutation: make_mutation(),
            token: None,
            author: "alice".to_string(),
        };

        let first = apply_mutation(&storage, &plugins, &authn, &authz, &config, &req)
            .expect("first call should succeed");

        // Second call with same idempotency key — base_revision no longer matches
        // but that doesn't matter because the idempotency key short-circuits.
        let req2 = MutateRequest {
            project: "myproject".to_string(),
            repo: "myrepo".to_string(),
            branch: "main".to_string(),
            base_revision: initial_head.clone(), // stale now, but irrelevant
            idempotency_key: "key-idempotent".to_string(),
            force: false,
            mutation: make_mutation(),
            token: None,
            author: "alice".to_string(),
        };

        let second = apply_mutation(&storage, &plugins, &authn, &authz, &config, &req2)
            .expect("second call should succeed via idempotency");

        assert_eq!(first, second, "idempotent call must return the same commit hash");
    }

    #[test]
    fn test_conflict() {
        let (storage, initial_head) = setup_storage();
        let plugins = make_plugins();
        let authn = NoopAuthn;
        let authz = NoopAuthz;
        let config = make_config();

        let wrong_base = schemahub_types::Hash::of(b"wrong-base").to_hex();

        let req = MutateRequest {
            project: "myproject".to_string(),
            repo: "myrepo".to_string(),
            branch: "main".to_string(),
            base_revision: wrong_base.clone(),
            idempotency_key: "key-conflict".to_string(),
            force: false,
            mutation: make_mutation(),
            token: None,
            author: "alice".to_string(),
        };

        let result = apply_mutation(&storage, &plugins, &authn, &authz, &config, &req);
        match result {
            Err(CoreError::Conflict { current_head, provided_base }) => {
                assert_eq!(current_head, initial_head);
                assert_eq!(provided_base, wrong_base);
            }
            other => panic!("expected Conflict error, got {:?}", other),
        }
    }
}
