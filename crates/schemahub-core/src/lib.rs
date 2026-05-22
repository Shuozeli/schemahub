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

    /// Return the raw source bytes of a schema as originally stored.
    /// Returns `None` if the schema does not exist at the given commit.
    pub fn get_schema_source(
        &self,
        _project: &str,
        _repo: &str,
        schema_name: &str,
        commit_hex: &str,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        let commit_hash = Hash::from_hex(commit_hex)
            .map_err(|_| CoreError::InvalidArgument(format!("commit_hex is not a valid hash: {commit_hex}")))?;
        let commit = version_control::commit::read_commit(self.storage.as_ref(), &commit_hash)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &commit)?;

        let schema_entry = root_tree.entries.iter()
            .find(|e| e.name == schema_name && e.kind == objects::KIND_SUBTREE);
        let Some(schema_tree_entry) = schema_entry else { return Ok(None); };

        let schema_tree_hash = Hash::from_hex(&schema_tree_entry.hash).map_err(|_| {
            CoreError::ObjectCorrupted(format!(
                "root tree entry '{}' has invalid hash: {}", schema_name, schema_tree_entry.hash
            ))
        })?;
        let schema_tree_data = self.storage.read_object(&schema_tree_hash)?
            .ok_or_else(|| CoreError::ObjectCorrupted(format!("schema tree {} not found", schema_tree_hash.to_hex())))?;
        let schema_tree = objects::decode_tree(&schema_tree_data)
            .map_err(|e| CoreError::ObjectCorrupted(format!("decode schema tree: {e}")))?;

        let blob_entry = schema_tree.entries.iter()
            .find(|e| e.name == "__schema__" && e.kind == objects::KIND_BLOB);
        let Some(blob_entry) = blob_entry else { return Ok(None); };

        let blob_hash = Hash::from_hex(&blob_entry.hash).map_err(|_| {
            CoreError::ObjectCorrupted(format!("__schema__ entry has invalid hash: {}", blob_entry.hash))
        })?;
        let blob_data = self.storage.read_object(&blob_hash)?
            .ok_or_else(|| CoreError::NotFound(format!("blob {} not found", blob_hash.to_hex())))?;

        // Use the plugin's `print` method to reconstruct canonical source text from
        // the internal blob representation. Falls back to raw bytes if no plugin found.
        let format_id = detect_format_from_name(schema_name)
            .unwrap_or_else(|| commit.format_id.clone());
        if let Some(plugin) = self.plugins.get(&format_id) {
            let blob = Blob::new(blob_data);
            match plugin.print(&blob) {
                Ok(src) => return Ok(Some(src.into_bytes())),
                Err(_) => return Ok(Some(blob.as_bytes().to_vec())),
            }
        }

        Ok(Some(blob_data))
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

        // Find __schema__ blob
        let schema_entry = schema_tree.entries.iter()
            .find(|e| e.name == "__schema__" && e.kind == objects::KIND_BLOB);
        let Some(entry) = schema_entry else { return Ok(result); };
        let blob_hash = Hash::from_hex(&entry.hash).map_err(|_| {
            CoreError::ObjectCorrupted(format!("__schema__ entry has invalid hash: {}", entry.hash))
        })?;
        let blob_data = self.storage.read_object(&blob_hash)?
            .ok_or_else(|| CoreError::NotFound(format!("blob {} not found", blob_hash.to_hex())))?;
        let blob = Blob::new(blob_data);
        result = plugin.list_declarations(&blob)
            .map_err(|e| CoreError::InvalidArgument(format!("blob is malformed: {e}")))?;

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

        // Find __schema__ blob
        let schema_entry = schema_tree.entries.iter()
            .find(|e| e.name == "__schema__" && e.kind == objects::KIND_BLOB);
        let Some(entry) = schema_entry else { return Ok(None); };
        let blob_hash = Hash::from_hex(&entry.hash).map_err(|_| {
            CoreError::ObjectCorrupted(format!("__schema__ entry has invalid hash: {}", entry.hash))
        })?;
        let blob_data = self.storage.read_object(&blob_hash)?
            .ok_or_else(|| CoreError::NotFound(format!("blob {} not found", blob_hash.to_hex())))?;
        let blob = Blob::new(blob_data);
        match plugin.get_declaration(&blob, decl_name) {
            Ok(detail) => return Ok(Some(detail)),
            Err(schemahub_types::ReadError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(CoreError::InvalidArgument(e.to_string())),
        }
    }

    // ── Search ────────────────────────────────────────────────────────────────

    /// Search declarations in a repo using the KV search index for prefix
    /// matching, then loading blobs only for schemas that have hits.
    ///
    /// Returns `(DeclSummary, schema_name)` pairs. Applies substring matching
    /// within each candidate schema so the semantics match the previous full-scan
    /// implementation, but only loads the schemas the index identifies as having
    /// at least one declaration whose name starts with `query`.
    ///
    /// Falls back to scanning all schemas when `query` is empty.
    pub fn search_declarations(
        &self,
        project: &str,
        repo: &str,
        query: &str,
        commit_hex: &str,
        limit: usize,
    ) -> Result<Vec<(schemahub_types::DeclSummary, String)>, CoreError> {
        use std::collections::{HashMap, HashSet};

        let commit_hash = Hash::from_hex(commit_hex)
            .map_err(|_| CoreError::InvalidArgument(format!("invalid commit hex: {commit_hex}")))?;
        let commit = version_control::commit::read_commit(self.storage.as_ref(), &commit_hash)?;
        let (_, root_tree) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &commit)?;

        // ── Step 1: Determine candidate schema names ──────────────────────────
        // When query is non-empty, scan the search index for all entries whose
        // declaration name starts with `query` and belong to this project/repo.
        // When query is empty, fall back to all schemas at the commit.
        let candidate_schemas: Vec<String> = if !query.is_empty() {
            let prefix = format!("search/{query}");
            let repo_infix = format!("/{project}/{repo}/");
            let entries = self.storage.scan_prefix(&prefix)
                .map_err(CoreError::Storage)?;
            let mut seen: HashSet<String> = HashSet::new();
            for (key, _) in &entries {
                // key = "search/{name}/{project}/{repo}/{schema}"
                let after_search = match key.strip_prefix("search/") {
                    Some(s) => s,
                    None => continue,
                };
                // Split off the decl name to get "/{project}/{repo}/{schema}"
                let slash = match after_search.find('/') {
                    Some(i) => i,
                    None => continue,
                };
                let rest = &after_search[slash..];
                if !rest.starts_with(&repo_infix) {
                    continue;
                }
                let schema_name = rest[repo_infix.len()..].to_string();
                if !schema_name.is_empty() {
                    seen.insert(schema_name);
                }
            }
            seen.into_iter().collect()
        } else {
            root_tree.entries.iter()
                .filter(|e| e.kind == objects::KIND_SUBTREE)
                .map(|e| e.name.clone())
                .collect()
        };

        // ── Step 2: Load each candidate schema and collect matching decls ─────
        let query_lower = query.to_lowercase();
        let mut results: Vec<(schemahub_types::DeclSummary, String)> = Vec::new();

        'outer: for schema_name in &candidate_schemas {
            let format_id = detect_format_from_name(schema_name)
                .unwrap_or_else(|| commit.format_id.clone());
            let plugin = match self.plugins.get(&format_id) {
                Some(p) => p,
                None => continue,
            };

            let (_, schema_tree) = match version_control::tree::schema_tree_from_root(
                self.storage.as_ref(), &root_tree, schema_name,
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let entry = match schema_tree.entries.iter()
                .find(|e| e.name == "__schema__" && e.kind == objects::KIND_BLOB)
            {
                Some(e) => e,
                None => continue,
            };
            let blob_hash = match Hash::from_hex(&entry.hash) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let blob_data = match self.storage.read_object(&blob_hash) {
                Ok(Some(d)) => d,
                _ => continue,
            };
            let blob = Blob::new(blob_data);

            if let Ok(decls) = plugin.list_declarations(&blob) {
                for decl in decls {
                    if decl.name.to_lowercase().contains(&query_lower) {
                        results.push((decl, schema_name.clone()));
                        if results.len() >= limit {
                            break 'outer;
                        }
                    }
                }
            }
        }

        Ok(results)
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

        // Update search index (best-effort — commit already succeeded).
        if let Ok(decls) = plugin.list_declarations(&envelope_blob) {
            for decl in &decls {
                let key = keys::search_key(&decl.name, project, repo, schema_name);
                let _ = self.storage.put(&key, schema_name.as_bytes());
            }
        }

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

        // Load the schema tree to get the current __schema__ blob (for compat check).
        let (_, schema_tree) = version_control::tree::schema_tree_from_root(
            self.storage.as_ref(), &root_tree, schema_name,
        ).map_err(|_| CoreError::NotFound(format!("schema '{schema_name}' not found")))?;

        let old_blob = match version_control::tree::blob_hash_from_schema_tree(&schema_tree, "__schema__") {
            Ok(blob_hash) => {
                let data = self.storage.read_object(&blob_hash)?
                    .ok_or_else(|| CoreError::NotFound(format!("blob {} not found", blob_hash.to_hex())))?;
                schemahub_types::Blob::new(data)
            }
            Err(_) => schemahub_types::Blob::new(vec![]),
        };

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

        // Enforce compatibility on protected branches (unless force=true).
        let repo_config = self.get_repo_config(project, repo)?;
        if repo_config.is_protected(branch) && !force {
            let rules = schemahub_types::CompatibilityRules {
                direction: repo_config.compatibility_direction,
            };
            plugin.check_compatibility(&old_blob, &envelope_blob, &rules)
                .map_err(CoreError::CompatibilityViolation)?;
        }

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

        // Update search index: remove stale entries, write fresh ones (best-effort).
        if let Ok(old_decls) = plugin.list_declarations(&old_blob) {
            for decl in &old_decls {
                let key = keys::search_key(&decl.name, project, repo, schema_name);
                let _ = self.storage.delete(&key);
            }
        }
        if let Ok(new_decls) = plugin.list_declarations(&envelope_blob) {
            for decl in &new_decls {
                let key = keys::search_key(&decl.name, project, repo, schema_name);
                let _ = self.storage.put(&key, schema_name.as_bytes());
            }
        }

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

        // Ensure schema exists and load the __schema__ blob for search index cleanup.
        let (_, existing_schema_tree) = version_control::tree::schema_tree_from_root(
            self.storage.as_ref(), &root_tree, schema_name,
        )?;
        let old_schema_blob = version_control::tree::blob_hash_from_schema_tree(
            &existing_schema_tree, "__schema__",
        ).ok().and_then(|h| {
            self.storage.read_object(&h).ok().flatten().map(Blob::new)
        });

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

        // Remove search index entries for the deleted schema (best-effort).
        if let Some(blob) = &old_schema_blob {
            if let Some(fid) = detect_format_from_name(schema_name) {
                if let Some(plugin) = self.plugins.get(&fid) {
                    if let Ok(decls) = plugin.list_declarations(blob) {
                        for decl in &decls {
                            let key = keys::search_key(&decl.name, project, repo, schema_name);
                            let _ = self.storage.delete(&key);
                        }
                    }
                }
            }
        }

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

        let mut schema_entries: Vec<TreeEntryProto> = Vec::new();

        // Store the whole parse result blob as __schema__.
        let blob_data = envelope_blob.as_bytes().to_vec();
        let blob_hash = Hash::of(&blob_data);
        self.storage.write_object(&blob_hash, &blob_data)?;
        schema_entries.push(TreeEntryProto {
            name: "__schema__".to_string(),
            kind: KIND_BLOB,
            hash: blob_hash.to_hex(),
        });
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

    // ── Project management ────────────────────────────────────────────────────

    pub fn create_project(&self, name: &str, is_public: bool) -> Result<(), CoreError> {
        let key = format!("project-meta/{name}");
        if self.storage.get(&key)?.is_some() {
            return Err(CoreError::AlreadyExists(format!("project '{name}' already exists")));
        }
        let meta = serde_json::json!({ "name": name, "is_public": is_public, "owner": "" });
        let bytes = serde_json::to_vec(&meta)
            .map_err(|e| CoreError::InvalidArgument(e.to_string()))?;
        self.storage.put(&key, &bytes)?;
        Ok(())
    }

    pub fn get_project_meta(&self, name: &str) -> Result<Option<serde_json::Value>, CoreError> {
        let key = format!("project-meta/{name}");
        match self.storage.get(&key)? {
            None => Ok(None),
            Some(bytes) => {
                let v = serde_json::from_slice(&bytes)
                    .map_err(|e| CoreError::ObjectCorrupted(e.to_string()))?;
                Ok(Some(v))
            }
        }
    }

    pub fn list_projects(&self, prefix: &str) -> Result<Vec<serde_json::Value>, CoreError> {
        let scan_prefix = format!("project-meta/{prefix}");
        let entries = self.storage.scan_prefix(&scan_prefix)?;
        let mut results = Vec::new();
        for (_, bytes) in entries {
            match serde_json::from_slice(&bytes) {
                Ok(v) => results.push(v),
                Err(_) => {} // skip malformed
            }
        }
        Ok(results)
    }

    pub fn delete_project(&self, name: &str, force: bool) -> Result<(), CoreError> {
        let key = format!("project-meta/{name}");
        if self.storage.get(&key)?.is_none() {
            return Err(CoreError::NotFound(format!("project '{name}' not found")));
        }
        if !force {
            // Check if there are any repos in this project.
            let repo_prefix = format!("config/{name}/");
            let repos = self.storage.scan_prefix(&repo_prefix)?;
            if !repos.is_empty() {
                return Err(CoreError::InvalidArgument(
                    "project has repos; use force=true to delete anyway".to_string(),
                ));
            }
        }
        self.storage.delete(&key)?;
        Ok(())
    }

    // ── Repo management ───────────────────────────────────────────────────────

    /// Initialize a new repo: stores config and creates the initial empty commit on the default branch.
    pub fn initialize_repo(
        &self,
        project: &str,
        repo: &str,
        config: &RepoConfig,
    ) -> Result<(), CoreError> {
        use objects::{CommitObject, TreeObject, encode_commit, encode_tree, hash_of_bytes, unix_now};

        // Check not already initialized.
        let config_key = format!("config/{project}/{repo}");
        if self.storage.get(&config_key)?.is_some() {
            return Err(CoreError::AlreadyExists(format!(
                "repo '{project}/{repo}' already exists"
            )));
        }

        // Create empty root tree.
        let root_tree = TreeObject { blob_version: 1, entries: vec![], created_at_unix: unix_now() };
        let root_tree_encoded = encode_tree(&root_tree);
        let root_tree_hash = hash_of_bytes(&root_tree_encoded);
        self.storage.write_object(&root_tree_hash, &root_tree_encoded)?;

        // Create initial commit.
        let now = unix_now();
        let init_commit = CommitObject {
            blob_version: 1,
            tree_hash: root_tree_hash.to_hex(),
            parent_hashes: vec![],
            timestamp_unix: now,
            author: "schemahub".to_string(),
            message: "initial commit".to_string(),
            force: false,
            format_id: String::new(),
            created_at_unix: now,
        };
        let init_commit_encoded = encode_commit(&init_commit);
        let init_commit_hash = hash_of_bytes(&init_commit_encoded);
        self.storage.write_object(&init_commit_hash, &init_commit_encoded)?;

        // Create default branch.
        version_control::branch::set_branch_head(
            self.storage.as_ref(),
            project,
            repo,
            &config.default_branch,
            &init_commit_hash,
        )?;

        // Persist config.
        self.set_repo_config(project, repo, config)?;
        Ok(())
    }

    pub fn list_repos(&self, project: &str, prefix: &str) -> Result<Vec<(String, RepoConfig)>, CoreError> {
        let scan_prefix = format!("config/{project}/{prefix}");
        let entries = self.storage.scan_prefix(&scan_prefix)?;
        let config_prefix = format!("config/{project}/");
        let mut results = Vec::new();
        for (key, bytes) in entries {
            let repo_name = key
                .strip_prefix(&config_prefix)
                .unwrap_or(&key)
                .to_string();
            match serde_json::from_slice::<RepoConfig>(&bytes) {
                Ok(cfg) => results.push((repo_name, cfg)),
                Err(_) => {} // skip malformed
            }
        }
        Ok(results)
    }

    pub fn delete_repo(&self, project: &str, repo: &str, force: bool) -> Result<(), CoreError> {
        let config_key = format!("config/{project}/{repo}");
        if self.storage.get(&config_key)?.is_none() {
            return Err(CoreError::NotFound(format!("repo '{project}/{repo}' not found")));
        }
        if !force {
            // Check if there are schemas.
            let branch_prefix = keys::branch_refs_prefix(project, repo);
            let branches = self.storage.scan_prefix(&branch_prefix)?;
            // Check if the default branch's commit has a non-empty root tree.
            if let Some((_, head_hash_bytes)) = branches.first() {
                if let Ok(hex) = std::str::from_utf8(head_hash_bytes) {
                    if let Ok(hash) = Hash::from_hex(hex) {
                        if let Ok(commit) = version_control::commit::read_commit(self.storage.as_ref(), &hash) {
                            if let Ok((_, root_tree)) = version_control::tree::root_tree_from_commit(self.storage.as_ref(), &commit) {
                                if !root_tree.entries.is_empty() {
                                    return Err(CoreError::InvalidArgument(
                                        "repo has schemas; use force=true to delete anyway".to_string(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        // Delete config and all refs for this repo.
        self.storage.delete(&config_key)?;
        let refs_prefix = keys::refs_prefix(project, repo);
        self.storage.delete_prefix(&refs_prefix)?;
        Ok(())
    }

    // ── Member/ACL management ─────────────────────────────────────────────────

    pub fn add_member(&self, project: &str, identity: &str, role: &str) -> Result<(), CoreError> {
        let key = keys::role_key(project, identity);
        self.storage.put(&key, role.as_bytes())?;
        Ok(())
    }

    pub fn remove_member(&self, project: &str, identity: &str) -> Result<(), CoreError> {
        let key = keys::role_key(project, identity);
        self.storage.delete(&key)?;
        Ok(())
    }

    pub fn list_members(&self, project: &str) -> Result<Vec<(String, String)>, CoreError> {
        let prefix = keys::roles_prefix(project);
        let entries = self.storage.scan_prefix(&prefix)?;
        let mut members = Vec::new();
        for (key, value) in entries {
            let identity = key.strip_prefix(&prefix).unwrap_or(&key).to_string();
            let role = String::from_utf8(value).unwrap_or_else(|_| "unknown".to_string());
            members.push((identity, role));
        }
        Ok(members)
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
