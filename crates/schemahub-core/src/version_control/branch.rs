use schemahub_storage::{StorageBackend, keys};
use schemahub_types::Hash;

use crate::error::CoreError;

/// Retrieve the current HEAD hash for a branch.
pub fn get_branch_head(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    branch: &str,
) -> Result<Hash, CoreError> {
    let key = keys::branch_ref_key(project, repo, branch);
    storage
        .get_ref(&key)?
        .ok_or_else(|| CoreError::NotFound(format!("branch '{branch}' not found in {project}/{repo}")))
}

/// Unconditionally set the HEAD hash for a branch.
pub fn set_branch_head(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    branch: &str,
    hash: &Hash,
) -> Result<(), CoreError> {
    let key = keys::branch_ref_key(project, repo, branch);
    storage.set_ref(&key, hash)?;
    Ok(())
}

/// Create a new branch pointing at `from_hash`.
/// Returns `CoreError::AlreadyExists` if the branch already exists.
/// Returns `CoreError::NotFound` if the commit does not exist in storage.
pub fn create_branch(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    name: &str,
    from_hash: &Hash,
) -> Result<(), CoreError> {
    let key = keys::branch_ref_key(project, repo, name);
    if storage.get_ref(&key)?.is_some() {
        return Err(CoreError::AlreadyExists(format!(
            "branch '{name}' already exists in {project}/{repo}"
        )));
    }
    // Validate that the commit actually exists in storage.
    let obj_key = schemahub_storage::keys::object_key(from_hash);
    if storage.get(&obj_key)?.is_none() {
        return Err(CoreError::NotFound(format!(
            "commit {} not found in storage", from_hash.to_hex()
        )));
    }
    storage.set_ref(&key, from_hash)?;
    Ok(())
}

/// Delete a branch ref.
pub fn delete_branch(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    name: &str,
) -> Result<(), CoreError> {
    let key = keys::branch_ref_key(project, repo, name);
    storage.delete_ref(&key)?;
    Ok(())
}

/// List all branches in a repo, optionally filtered by prefix.
/// The `prefix` is applied after "heads/" — e.g. "" returns all, "feature/" returns feature branches.
/// Returns a list of (branch_name, Hash) pairs.
pub fn list_branches(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    prefix: &str,
) -> Result<Vec<(String, Hash)>, CoreError> {
    let scan_prefix = format!("{}{}", keys::branch_refs_prefix(project, repo), prefix);
    let entries = storage.scan_prefix(&scan_prefix)?;

    let heads_prefix = keys::branch_refs_prefix(project, repo);
    let mut branches = Vec::new();
    for (key, value) in entries {
        let branch_name = key
            .strip_prefix(&heads_prefix)
            .unwrap_or(&key)
            .to_string();
        let hex = std::str::from_utf8(&value).map_err(|_| {
            CoreError::ObjectCorrupted(format!("branch ref '{key}' value is not valid UTF-8"))
        })?;
        let hash = Hash::from_hex(hex).map_err(|_| {
            CoreError::ObjectCorrupted(format!("branch ref '{key}' value is not a valid hash"))
        })?;
        branches.push((branch_name, hash));
    }
    Ok(branches)
}
