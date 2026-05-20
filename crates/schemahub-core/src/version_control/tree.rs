use schemahub_storage::StorageBackend;
use schemahub_types::Hash;

use crate::error::CoreError;
use crate::objects::{
    CommitObject, TreeEntryProto, TreeObject, decode_tree, encode_tree, hash_of_bytes,
    unix_now, KIND_BLOB, KIND_SUBTREE,
};

/// Read a TreeObject from the object store.
pub fn read_tree(storage: &dyn StorageBackend, hash: &Hash) -> Result<TreeObject, CoreError> {
    let data = storage
        .read_object(hash)?
        .ok_or_else(|| CoreError::NotFound(format!("tree object {} not found", hash.to_hex())))?;
    decode_tree(&data)
}

/// Write a TreeObject to the object store and return its hash.
pub fn write_tree(storage: &dyn StorageBackend, tree: &TreeObject) -> Result<Hash, CoreError> {
    let encoded = encode_tree(tree);
    let hash = hash_of_bytes(&encoded);
    storage.write_object(&hash, &encoded)?;
    Ok(hash)
}

/// Load the root tree from a commit object.
pub fn root_tree_from_commit(
    storage: &dyn StorageBackend,
    commit: &CommitObject,
) -> Result<(Hash, TreeObject), CoreError> {
    let hash = Hash::from_hex(&commit.tree_hash).map_err(|_| {
        CoreError::ObjectCorrupted(format!("commit has invalid tree hash: {}", commit.tree_hash))
    })?;
    let tree = read_tree(storage, &hash)?;
    Ok((hash, tree))
}

/// Find a schema's sub-tree within the root tree.
/// Returns `(schema_tree_hash, schema_tree)` or `NotFound`.
pub fn schema_tree_from_root(
    storage: &dyn StorageBackend,
    root: &TreeObject,
    schema_name: &str,
) -> Result<(Hash, TreeObject), CoreError> {
    for entry in &root.entries {
        if entry.name == schema_name && entry.kind == KIND_SUBTREE {
            let hash = Hash::from_hex(&entry.hash).map_err(|_| {
                CoreError::ObjectCorrupted(format!(
                    "root tree entry '{}' has invalid hash: {}",
                    schema_name, entry.hash
                ))
            })?;
            let tree = read_tree(storage, &hash)?;
            return Ok((hash, tree));
        }
    }
    Err(CoreError::NotFound(format!("schema '{schema_name}' not found in root tree")))
}

/// Find a blob hash within a schema tree by declaration name.
pub fn blob_hash_from_schema_tree(
    schema_tree: &TreeObject,
    decl_name: &str,
) -> Result<Hash, CoreError> {
    for entry in &schema_tree.entries {
        if entry.name == decl_name && entry.kind == KIND_BLOB {
            let hash = Hash::from_hex(&entry.hash).map_err(|_| {
                CoreError::ObjectCorrupted(format!(
                    "schema tree entry '{}' has invalid hash: {}",
                    decl_name, entry.hash
                ))
            })?;
            return Ok(hash);
        }
    }
    Err(CoreError::NotFound(format!("declaration '{decl_name}' not found in schema tree")))
}

/// Rebuild a schema tree replacing one entry (decl_name -> new_blob_hash).
/// Creates a new TreeObject and writes it. Returns the new tree hash.
pub fn update_schema_tree(
    storage: &dyn StorageBackend,
    schema_tree: &TreeObject,
    decl_name: &str,
    new_blob_hash: &Hash,
) -> Result<Hash, CoreError> {
    let mut new_entries: Vec<TreeEntryProto> = schema_tree
        .entries
        .iter()
        .filter(|e| e.name != decl_name)
        .cloned()
        .collect();

    new_entries.push(TreeEntryProto {
        name: decl_name.to_string(),
        kind: KIND_BLOB,
        hash: new_blob_hash.to_hex(),
    });

    // Keep sorted by name for determinism.
    new_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let new_tree = TreeObject {
        blob_version: 1,
        entries: new_entries,
        created_at_unix: unix_now(),
    };
    write_tree(storage, &new_tree)
}

/// Rebuild the root tree replacing one schema entry (schema_name -> new_schema_tree_hash).
/// Creates a new TreeObject and writes it. Returns the new root tree hash.
pub fn update_root_tree(
    storage: &dyn StorageBackend,
    root_tree: &TreeObject,
    schema_name: &str,
    new_schema_hash: &Hash,
) -> Result<Hash, CoreError> {
    let mut new_entries: Vec<TreeEntryProto> = root_tree
        .entries
        .iter()
        .filter(|e| e.name != schema_name)
        .cloned()
        .collect();

    new_entries.push(TreeEntryProto {
        name: schema_name.to_string(),
        kind: KIND_SUBTREE,
        hash: new_schema_hash.to_hex(),
    });

    new_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let new_root = TreeObject {
        blob_version: 1,
        entries: new_entries,
        created_at_unix: unix_now(),
    };
    write_tree(storage, &new_root)
}
