use schemahub_types::Hash;

use crate::StorageError;

/// A single write operation, batched in a StorageBackend::write_transaction call.
#[derive(Debug)]
pub enum StorageOp {
    Put { key: String, value: Vec<u8> },
    Delete { key: String },
}

/// The storage abstraction used by schemahub-core.
/// The core depends only on this trait — not on redb or any other concrete backend.
pub trait StorageBackend: Send + Sync + 'static {
    // ── Objects ───────────────────────────────────────────────────────────────

    fn write_object(&self, hash: &Hash, data: &[u8]) -> Result<(), StorageError>;
    fn read_object(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StorageError>;
    fn object_exists(&self, hash: &Hash) -> Result<bool, StorageError>;

    // ── Refs ──────────────────────────────────────────────────────────────────

    /// Write a ref unconditionally (used for branch creation, tag creation).
    fn set_ref(&self, key: &str, hash: &Hash) -> Result<(), StorageError>;
    fn get_ref(&self, key: &str) -> Result<Option<Hash>, StorageError>;
    fn delete_ref(&self, key: &str) -> Result<(), StorageError>;

    /// Atomic compare-and-swap on a ref.
    /// Returns Ok(true) if the swap succeeded, Ok(false) if `expected` didn't match.
    fn compare_and_set_ref(
        &self,
        key: &str,
        expected: &Hash,
        new: &Hash,
    ) -> Result<bool, StorageError>;

    // ── Generic KV (for index, deps, search, idempotency, pending, roles) ────

    fn put(&self, key: &str, value: &[u8]) -> Result<(), StorageError>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    fn delete(&self, key: &str) -> Result<(), StorageError>;

    /// Scan all keys with the given prefix. Returns (key, value) pairs.
    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StorageError>;

    /// Delete all keys with the given prefix. Returns the count of deleted entries.
    fn delete_prefix(&self, prefix: &str) -> Result<u64, StorageError>;

    // ── Atomic multi-op transaction ───────────────────────────────────────────

    /// Execute a batch of operations atomically. Either all succeed or none do.
    fn write_transaction(&self, ops: Vec<StorageOp>) -> Result<(), StorageError>;

    // ── Object listing (for GC) ───────────────────────────────────────────────

    /// Iterate over all object hashes in the store.
    fn list_all_objects(&self) -> Result<Vec<Hash>, StorageError>;
}
