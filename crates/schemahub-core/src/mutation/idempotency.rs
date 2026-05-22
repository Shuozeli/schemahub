use serde::{Deserialize, Serialize};
use schemahub_storage::{StorageBackend, keys};

use crate::error::CoreError;
use crate::objects::unix_now;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "status")]
pub enum IdempotencyResult {
    #[serde(rename = "success")]
    Success { commit_hash: String },
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IdempotencyEntry {
    pub result: IdempotencyResult,
    pub expires_at_unix: i64,
}

/// Check whether an idempotency key already has a stored result.
/// Returns `None` if the key is not found or has expired.
pub fn check_idempotency(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    key: &str,
) -> Result<Option<IdempotencyResult>, CoreError> {
    if key.is_empty() {
        return Ok(None);
    }
    let storage_key = keys::idempotency_key(project, repo, key);
    match storage.get(&storage_key)? {
        None => Ok(None),
        Some(bytes) => {
            let entry: IdempotencyEntry = serde_json::from_slice(&bytes)
                .map_err(|e| CoreError::ObjectCorrupted(format!("idempotency entry is malformed: {e}")))?;
            let now = unix_now();
            if entry.expires_at_unix <= now {
                // Expired — treat as absent.
                return Ok(None);
            }
            Ok(Some(entry.result))
        }
    }
}

/// Scan the idempotency/ namespace and delete all expired entries.
/// Returns the count of entries deleted (or that would be deleted in dry_run mode).
pub fn gc_idempotency_entries(
    storage: &dyn StorageBackend,
    dry_run: bool,
) -> Result<u64, CoreError> {
    let now = unix_now();
    let all = storage.scan_prefix("idempotency/")?;
    let mut cleaned = 0u64;
    for (key, value) in &all {
        let expired = serde_json::from_slice::<IdempotencyEntry>(value)
            .map(|e| e.expires_at_unix <= now)
            .unwrap_or(true); // malformed entry — treat as expired
        if expired {
            if !dry_run {
                let _ = storage.delete(key);
            }
            cleaned += 1;
        }
    }
    Ok(cleaned)
}

/// Store an idempotency result with a TTL in hours.
pub fn store_idempotency(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    key: &str,
    result: IdempotencyResult,
    ttl_hours: u32,
) -> Result<(), CoreError> {
    if key.is_empty() {
        return Ok(());
    }
    let expires_at_unix = unix_now() + (ttl_hours as i64) * 3600;
    let entry = IdempotencyEntry { result, expires_at_unix };
    let bytes = serde_json::to_vec(&entry)
        .map_err(|e| CoreError::InvalidArgument(format!("failed to serialize idempotency entry: {e}")))?;
    let storage_key = keys::idempotency_key(project, repo, key);
    storage.put(&storage_key, &bytes)?;
    Ok(())
}
