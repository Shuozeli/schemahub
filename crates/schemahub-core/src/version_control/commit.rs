use schemahub_storage::StorageBackend;
use schemahub_types::Hash;

use crate::error::CoreError;
use crate::objects::{
    CommitObject, decode_commit, encode_commit, hash_of_bytes, unix_now,
};

/// Create a new commit object, write it to the object store, and return its hash.
pub fn create_commit(
    storage: &dyn StorageBackend,
    tree_hash: Hash,
    parent_hashes: Vec<Hash>,
    author: &str,
    message: &str,
    force: bool,
    format_id: &str,
) -> Result<Hash, CoreError> {
    let now = unix_now();
    let commit = CommitObject {
        blob_version: 1,
        tree_hash: tree_hash.to_hex(),
        parent_hashes: parent_hashes.iter().map(|h| h.to_hex()).collect(),
        timestamp_unix: now,
        author: author.to_string(),
        message: message.to_string(),
        force,
        format_id: format_id.to_string(),
        created_at_unix: now,
    };
    let encoded = encode_commit(&commit);
    let hash = hash_of_bytes(&encoded);
    storage.write_object(&hash, &encoded)?;
    Ok(hash)
}

/// Read a commit from the object store.
pub fn read_commit(storage: &dyn StorageBackend, hash: &Hash) -> Result<CommitObject, CoreError> {
    let data = storage
        .read_object(hash)?
        .ok_or_else(|| CoreError::NotFound(format!("commit object {} not found", hash.to_hex())))?;
    decode_commit(&data)
}

/// Walk commit history from a starting hash, stopping at `stop_at` (exclusive).
/// Returns commits newest-first, up to `limit` entries.
pub fn walk_history(
    storage: &dyn StorageBackend,
    from: &Hash,
    stop_at: Option<&Hash>,
    limit: usize,
) -> Result<Vec<(Hash, CommitObject)>, CoreError> {
    let mut result = Vec::new();
    let mut frontier = vec![*from];

    while !frontier.is_empty() && result.len() < limit {
        let current_hash = frontier.remove(0);
        if let Some(stop) = stop_at {
            if current_hash == *stop {
                continue;
            }
        }
        let commit = read_commit(storage, &current_hash)?;
        for parent_hex in &commit.parent_hashes {
            let parent_hash = Hash::from_hex(parent_hex).map_err(|_| {
                CoreError::ObjectCorrupted(format!("commit {} has invalid parent hash {parent_hex}", current_hash.to_hex()))
            })?;
            frontier.push(parent_hash);
        }
        result.push((current_hash, commit));
    }

    Ok(result)
}
