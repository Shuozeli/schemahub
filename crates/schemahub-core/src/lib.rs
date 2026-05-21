pub mod error;
pub mod mutation;
pub mod objects;
pub mod plugin_registry;
pub mod repo_config;
pub mod version_control;

pub use error::CoreError;
pub use mutation::{apply_mutation, MutateRequest};
pub use plugin_registry::PluginRegistry;
pub use repo_config::RepoConfig;

use std::sync::Arc;

use schemahub_storage::{StorageBackend, keys};
use schemahub_types::{AuthnProvider, AuthzPolicy, Blob, Hash};

/// The Core struct owns all components and exposes the public API.
pub struct Core {
    pub storage: Arc<dyn StorageBackend>,
    pub plugins: PluginRegistry,
    pub authn: Arc<dyn AuthnProvider>,
    pub authz: Arc<dyn AuthzPolicy>,
}

impl Core {
    pub fn new(
        storage: Arc<dyn StorageBackend>,
        plugins: PluginRegistry,
        authn: Arc<dyn AuthnProvider>,
        authz: Arc<dyn AuthzPolicy>,
    ) -> Self {
        Self { storage, plugins, authn, authz }
    }

    // ── Repo config ───────────────────────────────────────────────────────────

    pub fn get_repo_config(&self, project: &str, repo: &str) -> Result<RepoConfig, CoreError> {
        repo_config::load_repo_config(self.storage.as_ref(), project, repo)
    }

    pub fn set_repo_config(&self, project: &str, repo: &str, config: &RepoConfig) -> Result<(), CoreError> {
        repo_config::save_repo_config(self.storage.as_ref(), project, repo, config)
    }

    // ── Version control ───────────────────────────────────────────────────────

    pub fn create_branch(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        from: &str,
    ) -> Result<(), CoreError> {
        let from_hash = Hash::from_hex(from)
            .map_err(|_| CoreError::InvalidArgument(format!("from is not a valid hash: {from}")))?;
        version_control::branch::create_branch(
            self.storage.as_ref(),
            project,
            repo,
            name,
            &from_hash,
        )
    }

    pub fn delete_branch(&self, project: &str, repo: &str, name: &str) -> Result<(), CoreError> {
        version_control::branch::delete_branch(self.storage.as_ref(), project, repo, name)
    }

    pub fn list_branches(
        &self,
        project: &str,
        repo: &str,
        prefix: &str,
    ) -> Result<Vec<(String, Hash)>, CoreError> {
        version_control::branch::list_branches(self.storage.as_ref(), project, repo, prefix)
    }

    pub fn get_branch_head(
        &self,
        project: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Hash, CoreError> {
        version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, branch)
    }

    // ── Mutations ─────────────────────────────────────────────────────────────

    pub fn apply_mutation(&self, req: MutateRequest) -> Result<String, CoreError> {
        let config = self.get_repo_config(&req.project, &req.repo)?;
        mutation::single::apply_mutation(
            self.storage.as_ref(),
            &self.plugins,
            self.authn.as_ref(),
            self.authz.as_ref(),
            &config,
            &req,
        )
    }

    pub fn apply_mutations(&self, req: mutation::batch::BatchMutateRequest) -> Result<String, CoreError> {
        let config = self.get_repo_config(&req.project, &req.repo)?;
        mutation::batch::apply_mutations(
            self.storage.as_ref(),
            &self.plugins,
            self.authn.as_ref(),
            self.authz.as_ref(),
            &config,
            &req,
        )
    }

    // ── Schema exploration ────────────────────────────────────────────────────

    /// List all schema names in a repo at a given commit.
    /// Returns (schema_name, schema_tree_hash) pairs.
    pub fn list_schemas(
        &self,
        _project: &str,
        _repo: &str,
        commit_hex: &str,
    ) -> Result<Vec<(String, Hash)>, CoreError> {
        let commit_hash = Hash::from_hex(commit_hex)
            .map_err(|_| CoreError::InvalidArgument(format!("commit_hex is not a valid hash: {commit_hex}")))?;
        let commit = version_control::commit::read_commit(self.storage.as_ref(), &commit_hash)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &commit)?;

        let mut schemas = Vec::new();
        for entry in &root_tree.entries {
            if entry.kind == objects::KIND_SUBTREE {
                let hash = Hash::from_hex(&entry.hash).map_err(|_| {
                    CoreError::ObjectCorrupted(format!(
                        "root tree entry '{}' has invalid hash: {}",
                        entry.name, entry.hash
                    ))
                })?;
                schemas.push((entry.name.clone(), hash));
            }
        }
        Ok(schemas)
    }

    /// List all declarations in a schema at a given commit.
    pub fn list_declarations(
        &self,
        _project: &str,
        _repo: &str,
        schema_name: &str,
        commit_hex: &str,
    ) -> Result<Vec<schemahub_types::DeclSummary>, CoreError> {
        let commit_hash = Hash::from_hex(commit_hex)
            .map_err(|_| CoreError::InvalidArgument(format!("commit_hex is not a valid hash: {commit_hex}")))?;
        let commit = version_control::commit::read_commit(self.storage.as_ref(), &commit_hash)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &commit)?;
        let (_, schema_tree) = version_control::tree::schema_tree_from_root(
            self.storage.as_ref(),
            &root_tree,
            schema_name,
        )?;

        let format_id = detect_format_from_name(schema_name)
            .unwrap_or_else(|| commit.format_id.clone());
        let plugin = self.plugins.get(&format_id).ok_or_else(|| {
            CoreError::InvalidArgument(format!("unknown format_id: {format_id}"))
        })?;

        let mut result = Vec::new();
        for entry in &schema_tree.entries {
            if entry.kind == objects::KIND_BLOB {
                let blob_hash = Hash::from_hex(&entry.hash).map_err(|_| {
                    CoreError::ObjectCorrupted(format!(
                        "schema tree entry '{}' has invalid hash: {}",
                        entry.name, entry.hash
                    ))
                })?;
                let blob_data = self
                    .storage
                    .read_object(&blob_hash)?
                    .ok_or_else(|| CoreError::NotFound(format!("blob {} not found", blob_hash.to_hex())))?;
                let blob = Blob::new(blob_data);
                let decls = plugin
                    .list_declarations(&blob)
                    .map_err(|e| CoreError::InvalidArgument(e.to_string()))?;
                result.extend(decls);
            }
        }
        Ok(result)
    }

    /// Get a single declaration detail.
    pub fn get_declaration(
        &self,
        _project: &str,
        _repo: &str,
        schema_name: &str,
        decl_name: &str,
        commit_hex: &str,
    ) -> Result<Option<schemahub_types::DeclDetail>, CoreError> {
        let commit_hash = Hash::from_hex(commit_hex)
            .map_err(|_| CoreError::InvalidArgument(format!("commit_hex is not a valid hash: {commit_hex}")))?;
        let commit = version_control::commit::read_commit(self.storage.as_ref(), &commit_hash)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &commit)?;
        let (_, schema_tree) = match version_control::tree::schema_tree_from_root(
            self.storage.as_ref(),
            &root_tree,
            schema_name,
        ) {
            Ok(v) => v,
            Err(CoreError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };

        let format_id = detect_format_from_name(schema_name)
            .unwrap_or_else(|| commit.format_id.clone());
        let plugin = self.plugins.get(&format_id).ok_or_else(|| {
            CoreError::InvalidArgument(format!("unknown format_id: {format_id}"))
        })?;

        // Search all blobs in schema tree for the declaration.
        for entry in &schema_tree.entries {
            if entry.kind != objects::KIND_BLOB {
                continue;
            }
            let blob_hash = Hash::from_hex(&entry.hash).map_err(|_| {
                CoreError::ObjectCorrupted(format!(
                    "schema tree entry '{}' has invalid hash: {}",
                    entry.name, entry.hash
                ))
            })?;
            let blob_data = self
                .storage
                .read_object(&blob_hash)?
                .ok_or_else(|| CoreError::NotFound(format!("blob {} not found", blob_hash.to_hex())))?;
            let blob = Blob::new(blob_data);
            match plugin.get_declaration(&blob, decl_name) {
                Ok(detail) => return Ok(Some(detail)),
                Err(schemahub_types::ReadError::NotFound(_)) => continue,
                Err(e) => return Err(CoreError::InvalidArgument(e.to_string())),
            }
        }
        Ok(None)
    }

    // ── Schema lifecycle ──────────────────────────────────────────────────────

    /// Create a new schema (first push of a schema file).
    pub fn create_schema(
        &self,
        project: &str,
        repo: &str,
        branch: &str,
        schema_name: &str,
        source: &[u8],
        format_id: &str,
        base_revision: &str,
        idempotency_key: &str,
        author: &str,
        token: Option<&str>,
    ) -> Result<String, CoreError> {
        use mutation::idempotency::{check_idempotency, store_idempotency, IdempotencyResult};
        use schemahub_types::{Action, ResourcePath};

        // Idempotency check.
        if let Some(existing) = check_idempotency(self.storage.as_ref(), project, repo, idempotency_key)? {
            match existing {
                IdempotencyResult::Success { commit_hash } => return Ok(commit_hash),
                IdempotencyResult::Error { code: _, message } => {
                    return Err(CoreError::InvalidArgument(format!(
                        "prior idempotent call failed: {message}"
                    )));
                }
            }
        }

        // AuthN/AuthZ.
        let identity = self.authn.identify(token)
            .map_err(|e| CoreError::Unauthenticated(e.to_string()))?;
        let resource = ResourcePath::repo(project, repo);
        self.authz
            .check(&identity, Action::Write, &resource)
            .map_err(|e| CoreError::PermissionDenied(e.to_string()))?;

        // Load current branch HEAD.
        let current_head = version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, branch)?;
        if !base_revision.is_empty() {
            let provided_base = Hash::from_hex(base_revision)
                .map_err(|_| CoreError::InvalidArgument(format!("base_revision is not a valid hash: {base_revision}")))?;
            if current_head != provided_base {
                return Err(CoreError::Conflict {
                    current_head: current_head.to_hex(),
                    provided_base: base_revision.to_string(),
                });
            }
        }

        // Load root tree.
        let head_commit = version_control::commit::read_commit(self.storage.as_ref(), &current_head)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &head_commit)?;

        // Check schema doesn't already exist.
        if version_control::tree::schema_tree_from_root(self.storage.as_ref(), &root_tree, schema_name).is_ok() {
            return Err(CoreError::AlreadyExists(format!("schema '{schema_name}' already exists")));
        }

        // Get plugin and parse source.
        let plugin = self.plugins.get(format_id).ok_or_else(|| {
            CoreError::InvalidArgument(format!("unknown format_id: {format_id}"))
        })?;
        let source_str = std::str::from_utf8(source)
            .map_err(|_| CoreError::InvalidArgument("source is not valid UTF-8".to_string()))?;
        let envelope_blob = plugin.parse(source_str)
            .map_err(|e| CoreError::InvalidArgument(e.to_string()))?;

        // Build schema tree from declarations.
        let new_root_tree_hash = self.build_schema_tree_and_root(
            &root_tree,
            schema_name,
            &envelope_blob,
            plugin.as_ref(),
        )?;

        // Create commit.
        let new_commit_hash = version_control::commit::create_commit(
            self.storage.as_ref(),
            new_root_tree_hash,
            vec![current_head],
            author,
            &format!("create schema: {schema_name}"),
            false,
            format_id,
        )?;

        // CAS update branch ref.
        let branch_ref_key = keys::branch_ref_key(project, repo, branch);
        let swapped = self.storage.compare_and_set_ref(&branch_ref_key, &current_head, &new_commit_hash)?;
        if !swapped {
            let actual_head = self.storage.get_ref(&branch_ref_key)?
                .map(|h| h.to_hex())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(CoreError::Conflict {
                current_head: actual_head,
                provided_base: current_head.to_hex(),
            });
        }

        let commit_hex = new_commit_hash.to_hex();
        store_idempotency(
            self.storage.as_ref(),
            project,
            repo,
            idempotency_key,
            IdempotencyResult::Success { commit_hash: commit_hex.clone() },
            24,
        )?;
        Ok(commit_hex)
    }

    /// Update an existing schema (whole-document push for all formats).
    pub fn update_schema(
        &self,
        project: &str,
        repo: &str,
        branch: &str,
        schema_name: &str,
        source: &[u8],
        base_revision: &str,
        idempotency_key: &str,
        force: bool,
        author: &str,
        token: Option<&str>,
    ) -> Result<String, CoreError> {
        use mutation::idempotency::{check_idempotency, store_idempotency, IdempotencyResult};
        use schemahub_types::{Action, ResourcePath};

        // Idempotency check.
        if let Some(existing) = check_idempotency(self.storage.as_ref(), project, repo, idempotency_key)? {
            match existing {
                IdempotencyResult::Success { commit_hash } => return Ok(commit_hash),
                IdempotencyResult::Error { code: _, message } => {
                    return Err(CoreError::InvalidArgument(format!(
                        "prior idempotent call failed: {message}"
                    )));
                }
            }
        }

        // AuthN/AuthZ.
        let identity = self.authn.identify(token)
            .map_err(|e| CoreError::Unauthenticated(e.to_string()))?;
        let resource = ResourcePath::repo(project, repo);
        let action = if force { Action::Force } else { Action::Write };
        self.authz
            .check(&identity, action, &resource)
            .map_err(|e| CoreError::PermissionDenied(e.to_string()))?;

        // Load current branch HEAD.
        let current_head = version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, branch)?;
        if !base_revision.is_empty() {
            let provided_base = Hash::from_hex(base_revision)
                .map_err(|_| CoreError::InvalidArgument(format!("base_revision is not a valid hash: {base_revision}")))?;
            if current_head != provided_base {
                return Err(CoreError::Conflict {
                    current_head: current_head.to_hex(),
                    provided_base: base_revision.to_string(),
                });
            }
        }

        // Load root tree.
        let head_commit = version_control::commit::read_commit(self.storage.as_ref(), &current_head)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &head_commit)?;

        // Detect format from schema name.
        let format_id = detect_format_from_name(schema_name)
            .unwrap_or_else(|| head_commit.format_id.clone());
        let plugin = self.plugins.get(&format_id).ok_or_else(|| {
            CoreError::InvalidArgument(format!("unknown format_id: {format_id}"))
        })?;

        let source_str = std::str::from_utf8(source)
            .map_err(|_| CoreError::InvalidArgument("source is not valid UTF-8".to_string()))?;
        let envelope_blob = plugin.parse(source_str)
            .map_err(|e| CoreError::InvalidArgument(e.to_string()))?;

        // Build schema tree and root.
        let new_root_tree_hash = self.build_schema_tree_and_root(
            &root_tree,
            schema_name,
            &envelope_blob,
            plugin.as_ref(),
        )?;

        // Create commit.
        let new_commit_hash = version_control::commit::create_commit(
            self.storage.as_ref(),
            new_root_tree_hash,
            vec![current_head],
            author,
            &format!("update schema: {schema_name}"),
            force,
            &format_id,
        )?;

        // CAS update branch ref.
        let branch_ref_key = keys::branch_ref_key(project, repo, branch);
        let swapped = self.storage.compare_and_set_ref(&branch_ref_key, &current_head, &new_commit_hash)?;
        if !swapped {
            let actual_head = self.storage.get_ref(&branch_ref_key)?
                .map(|h| h.to_hex())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(CoreError::Conflict {
                current_head: actual_head,
                provided_base: current_head.to_hex(),
            });
        }

        let commit_hex = new_commit_hash.to_hex();
        store_idempotency(
            self.storage.as_ref(),
            project,
            repo,
            idempotency_key,
            IdempotencyResult::Success { commit_hash: commit_hex.clone() },
            24,
        )?;
        Ok(commit_hex)
    }

    /// Delete a schema from a branch.
    pub fn delete_schema(
        &self,
        project: &str,
        repo: &str,
        branch: &str,
        schema_name: &str,
        base_revision: &str,
        idempotency_key: &str,
        force: bool,
        author: &str,
        token: Option<&str>,
    ) -> Result<String, CoreError> {
        use mutation::idempotency::{check_idempotency, store_idempotency, IdempotencyResult};
        use schemahub_types::{Action, ResourcePath};
        use objects::{TreeEntryProto, TreeObject, encode_tree, hash_of_bytes, unix_now, KIND_SUBTREE};

        // Idempotency check.
        if let Some(existing) = check_idempotency(self.storage.as_ref(), project, repo, idempotency_key)? {
            match existing {
                IdempotencyResult::Success { commit_hash } => return Ok(commit_hash),
                IdempotencyResult::Error { code: _, message } => {
                    return Err(CoreError::InvalidArgument(format!(
                        "prior idempotent call failed: {message}"
                    )));
                }
            }
        }

        // AuthN/AuthZ.
        let identity = self.authn.identify(token)
            .map_err(|e| CoreError::Unauthenticated(e.to_string()))?;
        let resource = ResourcePath::repo(project, repo);
        let action = if force { Action::Force } else { Action::Write };
        self.authz
            .check(&identity, action, &resource)
            .map_err(|e| CoreError::PermissionDenied(e.to_string()))?;

        // Load current HEAD.
        let current_head = version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, branch)?;
        if !base_revision.is_empty() {
            let provided_base = Hash::from_hex(base_revision)
                .map_err(|_| CoreError::InvalidArgument(format!("base_revision is not a valid hash: {base_revision}")))?;
            if current_head != provided_base {
                return Err(CoreError::Conflict {
                    current_head: current_head.to_hex(),
                    provided_base: base_revision.to_string(),
                });
            }
        }

        // Load root tree.
        let head_commit = version_control::commit::read_commit(self.storage.as_ref(), &current_head)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &head_commit)?;

        // Ensure schema exists.
        version_control::tree::schema_tree_from_root(self.storage.as_ref(), &root_tree, schema_name)?;

        // Build new root tree without the schema entry.
        let new_entries: Vec<TreeEntryProto> = root_tree
            .entries
            .iter()
            .filter(|e| !(e.name == schema_name && e.kind == KIND_SUBTREE))
            .cloned()
            .collect();
        let new_root_tree = TreeObject {
            blob_version: 1,
            entries: new_entries,
            created_at_unix: unix_now(),
        };
        let new_root_tree_encoded = encode_tree(&new_root_tree);
        let new_root_tree_hash = hash_of_bytes(&new_root_tree_encoded);
        self.storage.write_object(&new_root_tree_hash, &new_root_tree_encoded)?;

        // Create commit.
        let new_commit_hash = version_control::commit::create_commit(
            self.storage.as_ref(),
            new_root_tree_hash,
            vec![current_head],
            author,
            &format!("delete schema: {schema_name}"),
            force,
            "",
        )?;

        // CAS update branch ref.
        let branch_ref_key = keys::branch_ref_key(project, repo, branch);
        let swapped = self.storage.compare_and_set_ref(&branch_ref_key, &current_head, &new_commit_hash)?;
        if !swapped {
            let actual_head = self.storage.get_ref(&branch_ref_key)?
                .map(|h| h.to_hex())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(CoreError::Conflict {
                current_head: actual_head,
                provided_base: current_head.to_hex(),
            });
        }

        let commit_hex = new_commit_hash.to_hex();
        store_idempotency(
            self.storage.as_ref(),
            project,
            repo,
            idempotency_key,
            IdempotencyResult::Success { commit_hash: commit_hex.clone() },
            24,
        )?;
        Ok(commit_hex)
    }

    // ── Commit history ────────────────────────────────────────────────────────

    /// Get a single commit by hex hash.
    pub fn get_commit(
        &self,
        _project: &str,
        _repo: &str,
        commit_hex: &str,
    ) -> Result<objects::CommitObject, CoreError> {
        let hash = Hash::from_hex(commit_hex)
            .map_err(|_| CoreError::InvalidArgument(format!("commit_hex is not a valid hash: {commit_hex}")))?;
        version_control::commit::read_commit(self.storage.as_ref(), &hash)
    }

    /// Walk commit history from a branch or commit, up to `limit` entries.
    pub fn list_commits(
        &self,
        project: &str,
        repo: &str,
        from_branch: Option<&str>,
        from_commit: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(Hash, objects::CommitObject)>, CoreError> {
        let start_hash = match (from_branch, from_commit) {
            (_, Some(hex)) => Hash::from_hex(hex)
                .map_err(|_| CoreError::InvalidArgument(format!("from_commit is not a valid hash: {hex}")))?,
            (Some(branch), None) => {
                version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, branch)?
            }
            (None, None) => {
                // Default to the default branch.
                let config = self.get_repo_config(project, repo)?;
                version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, &config.default_branch)?
            }
        };
        version_control::commit::walk_history(self.storage.as_ref(), &start_hash, None, limit)
    }

    /// Fast-forward merge source_branch into target_branch.
    pub fn merge_branches(
        &self,
        project: &str,
        repo: &str,
        source_branch: &str,
        target_branch: &str,
        base_revision: &str,
        idempotency_key: &str,
        author: &str,
        token: Option<&str>,
    ) -> Result<String, CoreError> {
        use mutation::idempotency::{check_idempotency, store_idempotency, IdempotencyResult};
        use schemahub_types::{Action, ResourcePath};

        // Idempotency check.
        if let Some(existing) = check_idempotency(self.storage.as_ref(), project, repo, idempotency_key)? {
            match existing {
                IdempotencyResult::Success { commit_hash } => return Ok(commit_hash),
                IdempotencyResult::Error { code: _, message } => {
                    return Err(CoreError::InvalidArgument(format!(
                        "prior idempotent call failed: {message}"
                    )));
                }
            }
        }

        let _ = author; // may be used for merge commits in future

        // AuthN/AuthZ.
        let identity = self.authn.identify(token)
            .map_err(|e| CoreError::Unauthenticated(e.to_string()))?;
        let resource = ResourcePath::repo(project, repo);
        self.authz
            .check(&identity, Action::Write, &resource)
            .map_err(|e| CoreError::PermissionDenied(e.to_string()))?;

        // Load target and source HEADs.
        let target_head = version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, target_branch)?;
        let source_head = version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, source_branch)?;

        // Check base_revision matches target HEAD.
        if !base_revision.is_empty() {
            let provided_base = Hash::from_hex(base_revision)
                .map_err(|_| CoreError::InvalidArgument(format!("base_revision is not a valid hash: {base_revision}")))?;
            if target_head != provided_base {
                return Err(CoreError::Conflict {
                    current_head: target_head.to_hex(),
                    provided_base: base_revision.to_string(),
                });
            }
        }

        // If already equal, nothing to do.
        if source_head == target_head {
            let commit_hex = source_head.to_hex();
            store_idempotency(
                self.storage.as_ref(),
                project,
                repo,
                idempotency_key,
                IdempotencyResult::Success { commit_hash: commit_hex.clone() },
                24,
            )?;
            return Ok(commit_hex);
        }

        // Walk source history to check if target_head is an ancestor.
        let history = version_control::commit::walk_history(
            self.storage.as_ref(),
            &source_head,
            None,
            1000,
        )?;
        let is_ancestor = history.iter().any(|(h, _)| *h == target_head);
        if !is_ancestor {
            return Err(CoreError::Conflict {
                current_head: target_head.to_hex(),
                provided_base: source_head.to_hex(),
            });
        }

        // Fast-forward: CAS update target branch to source HEAD.
        let branch_ref_key = keys::branch_ref_key(project, repo, target_branch);
        let swapped = self.storage.compare_and_set_ref(&branch_ref_key, &target_head, &source_head)?;
        if !swapped {
            let actual_head = self.storage.get_ref(&branch_ref_key)?
                .map(|h| h.to_hex())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(CoreError::Conflict {
                current_head: actual_head,
                provided_base: target_head.to_hex(),
            });
        }

        let commit_hex = source_head.to_hex();
        store_idempotency(
            self.storage.as_ref(),
            project,
            repo,
            idempotency_key,
            IdempotencyResult::Success { commit_hash: commit_hex.clone() },
            24,
        )?;
        Ok(commit_hex)
    }

    // ── Tags ──────────────────────────────────────────────────────────────────

    pub fn create_tag(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        commit_hex: &str,
        _message: Option<&str>,
    ) -> Result<(), CoreError> {
        let hash = Hash::from_hex(commit_hex)
            .map_err(|_| CoreError::InvalidArgument(format!("commit_hex is not a valid hash: {commit_hex}")))?;
        let key = keys::tag_ref_key(project, repo, name);
        if self.storage.get_ref(&key)?.is_some() {
            return Err(CoreError::AlreadyExists(format!("tag '{name}' already exists")));
        }
        self.storage.set_ref(&key, &hash)?;
        Ok(())
    }

    pub fn delete_tag(&self, project: &str, repo: &str, name: &str) -> Result<(), CoreError> {
        let key = keys::tag_ref_key(project, repo, name);
        self.storage.delete_ref(&key)?;
        Ok(())
    }

    pub fn list_tags(
        &self,
        project: &str,
        repo: &str,
        prefix: &str,
    ) -> Result<Vec<(String, Hash)>, CoreError> {
        let scan_prefix = format!("{}{}", keys::tag_refs_prefix(project, repo), prefix);
        let entries = self.storage.scan_prefix(&scan_prefix)?;

        let tags_prefix = keys::tag_refs_prefix(project, repo);
        let mut tags = Vec::new();
        for (key, value) in entries {
            let tag_name = key
                .strip_prefix(&tags_prefix)
                .unwrap_or(&key)
                .to_string();
            let hex = std::str::from_utf8(&value).map_err(|_| {
                CoreError::ObjectCorrupted(format!("tag ref '{key}' value is not valid UTF-8"))
            })?;
            let hash = Hash::from_hex(hex).map_err(|_| {
                CoreError::ObjectCorrupted(format!("tag ref '{key}' value is not a valid hash"))
            })?;
            tags.push((tag_name, hash));
        }
        Ok(tags)
    }

    /// Resolve a VersionRef to a commit Hash.
    pub fn resolve_ref(
        &self,
        project: &str,
        repo: &str,
        ver: &version_ref::VersionRef,
    ) -> Result<Hash, CoreError> {
        match ver {
            version_ref::VersionRef::Branch(b) => {
                version_control::branch::get_branch_head(self.storage.as_ref(), project, repo, b)
            }
            version_ref::VersionRef::Tag(t) => {
                let key = keys::tag_ref_key(project, repo, t);
                self.storage
                    .get_ref(&key)?
                    .ok_or_else(|| CoreError::NotFound(format!("tag '{t}' not found")))
            }
            version_ref::VersionRef::Commit(c) => {
                Hash::from_hex(c).map_err(|_| {
                    CoreError::InvalidArgument(format!("commit ref is not a valid hash: {c}"))
                })
            }
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Parse envelope blob, extract declarations, store blobs, build schema tree,
    /// update root tree and write it. Returns new root tree hash.
    fn build_schema_tree_and_root(
        &self,
        root_tree: &objects::TreeObject,
        schema_name: &str,
        envelope_blob: &Blob,
        plugin: &dyn schemahub_types::FormatPlugin,
    ) -> Result<Hash, CoreError> {
        use objects::{TreeEntryProto, TreeObject, encode_tree, hash_of_bytes, unix_now, KIND_BLOB, KIND_SUBTREE};

        // List all declarations in the envelope.
        let decl_summaries = plugin
            .list_declarations(envelope_blob)
            .map_err(|e| CoreError::InvalidArgument(e.to_string()))?;

        let mut schema_entries: Vec<TreeEntryProto> = Vec::new();

        if decl_summaries.is_empty() {
            // Store the whole envelope as a single blob keyed by "__source__".
            let blob_data = envelope_blob.as_bytes().to_vec();
            let blob_hash = Hash::of(&blob_data);
            self.storage.write_object(&blob_hash, &blob_data)?;
            schema_entries.push(TreeEntryProto {
                name: "__source__".to_string(),
                kind: KIND_BLOB,
                hash: blob_hash.to_hex(),
            });
        } else {
            for summary in &decl_summaries {
                let detail = plugin
                    .get_declaration(envelope_blob, &summary.name)
                    .map_err(|e| CoreError::InvalidArgument(e.to_string()))?;
                let blob_data: Vec<u8> = detail.as_bytes().to_vec();
                let blob_hash = Hash::of(&blob_data);
                self.storage.write_object(&blob_hash, &blob_data)?;
                schema_entries.push(TreeEntryProto {
                    name: summary.name.clone(),
                    kind: KIND_BLOB,
                    hash: blob_hash.to_hex(),
                });
            }
        }

        schema_entries.sort_by(|a, b| a.name.cmp(&b.name));
        let schema_tree = TreeObject {
            blob_version: 1,
            entries: schema_entries,
            created_at_unix: unix_now(),
        };
        let schema_tree_encoded = encode_tree(&schema_tree);
        let schema_tree_hash = hash_of_bytes(&schema_tree_encoded);
        self.storage.write_object(&schema_tree_hash, &schema_tree_encoded)?;

        // Build new root tree.
        let mut new_root_entries: Vec<TreeEntryProto> = root_tree
            .entries
            .iter()
            .filter(|e| !(e.name == schema_name && e.kind == KIND_SUBTREE))
            .cloned()
            .collect();
        new_root_entries.push(TreeEntryProto {
            name: schema_name.to_string(),
            kind: KIND_SUBTREE,
            hash: schema_tree_hash.to_hex(),
        });
        new_root_entries.sort_by(|a, b| a.name.cmp(&b.name));

        let new_root_tree = TreeObject {
            blob_version: 1,
            entries: new_root_entries,
            created_at_unix: unix_now(),
        };
        let new_root_tree_encoded = encode_tree(&new_root_tree);
        let new_root_tree_hash = hash_of_bytes(&new_root_tree_encoded);
        self.storage.write_object(&new_root_tree_hash, &new_root_tree_encoded)?;

        Ok(new_root_tree_hash)
    }
}

// ── VersionRef enum (internal) ────────────────────────────────────────────────

pub mod version_ref {
    /// A simple version reference used by resolve_ref.
    pub enum VersionRef {
        Branch(String),
        Tag(String),
        Commit(String),
    }
}

// ── Format detection ──────────────────────────────────────────────────────────

/// Detect format_id from schema name extension.
/// `.proto` → "protobuf", `.fbs` → "flatbuffers", `.yaml`/`.yml`/`.json` → "openapi".
pub fn detect_format_from_name(schema_name: &str) -> Option<String> {
    if schema_name.ends_with(".proto") {
        Some("protobuf".to_string())
    } else if schema_name.ends_with(".fbs") {
        Some("flatbuffers".to_string())
    } else if schema_name.ends_with(".yaml")
        || schema_name.ends_with(".yml")
        || schema_name.ends_with(".json")
    {
        Some("openapi".to_string())
    } else {
        None
    }
}
