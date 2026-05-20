use redb::{Database, ReadableTable, TableDefinition};
use schemahub_types::Hash;

use crate::{
    backend::{StorageBackend, StorageOp},
    error::StorageError,
    keys::object_key,
};

/// redb table definitions.
/// redb uses typed table definitions; we use a single table with string keys
/// and byte-slice values for simplicity and uniform prefix scanning.
const KV: TableDefinition<&str, &[u8]> = TableDefinition::new("kv");

/// The redb-backed storage implementation.
pub struct RedbBackend {
    db: Database,
}

impl RedbBackend {
    pub fn open(path: &str) -> Result<Self, StorageError> {
        let db = Database::create(path)?;
        // Ensure the table exists.
        let write_txn = db.begin_write()?;
        write_txn.open_table(KV)?;
        write_txn.commit()?;
        Ok(Self { db })
    }

    /// In-memory backend for tests (uses a temp file).
    #[cfg(test)]
    pub fn ephemeral() -> Result<Self, StorageError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = format!("/tmp/schemahub-test-{}.redb", COUNTER.fetch_add(1, Ordering::Relaxed));
        Self::open(&path)
    }
}

impl StorageBackend for RedbBackend {
    fn write_object(&self, hash: &Hash, data: &[u8]) -> Result<(), StorageError> {
        let key = object_key(hash);
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            table.insert(key.as_str(), data)?;
        }
        txn.commit()?;
        Ok(())
    }

    fn read_object(&self, hash: &Hash) -> Result<Option<Vec<u8>>, StorageError> {
        let key = object_key(hash);
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        Ok(table.get(key.as_str())?.map(|v| v.value().to_vec()))
    }

    fn object_exists(&self, hash: &Hash) -> Result<bool, StorageError> {
        let key = object_key(hash);
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        Ok(table.get(key.as_str())?.is_some())
    }

    fn set_ref(&self, key: &str, hash: &Hash) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            table.insert(key, hash.to_hex().as_bytes())?;
        }
        txn.commit()?;
        Ok(())
    }

    fn get_ref(&self, key: &str) -> Result<Option<Hash>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        match table.get(key)? {
            None => Ok(None),
            Some(v) => {
                let hex = std::str::from_utf8(v.value()).map_err(|_| {
                    StorageError::ValueEncoding(format!("ref value for '{key}' is not valid UTF-8"))
                })?;
                let hash = Hash::from_hex(hex).map_err(|_| {
                    StorageError::ValueEncoding(format!("ref value for '{key}' is not a valid hash hex"))
                })?;
                Ok(Some(hash))
            }
        }
    }

    fn delete_ref(&self, key: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            table.remove(key)?;
        }
        txn.commit()?;
        Ok(())
    }

    fn compare_and_set_ref(
        &self,
        key: &str,
        expected: &Hash,
        new: &Hash,
    ) -> Result<bool, StorageError> {
        let txn = self.db.begin_write()?;
        let swapped = {
            let mut table = txn.open_table(KV)?;
            // Read and drop the guard before taking a mutable borrow.
            let current_bytes: Option<Vec<u8>> = table
                .get(key)?
                .map(|v| v.value().to_vec());
            match current_bytes {
                Some(bytes) => {
                        let expected_hex = expected.to_hex();
                    let current_hex = std::str::from_utf8(&bytes).unwrap_or("");
                    if current_hex == expected_hex {
                        table.insert(key, new.to_hex().as_bytes())?;
                        true
                    } else {
                        false
                    }
                }
                None => false,
            }
        };
        if swapped {
            txn.commit()?;
        }
        Ok(swapped)
    }

    fn put(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            table.insert(key, value)?;
        }
        txn.commit()?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        Ok(table.get(key)?.map(|v| v.value().to_vec()))
    }

    fn delete(&self, key: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            table.remove(key)?;
        }
        txn.commit()?;
        Ok(())
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        let mut results = Vec::new();
        for entry in table.range(prefix..)? {
            let (k, v) = entry?;
            let key = k.value();
            if !key.starts_with(prefix) {
                break;
            }
            results.push((key.to_string(), v.value().to_vec()));
        }
        Ok(results)
    }

    fn delete_prefix(&self, prefix: &str) -> Result<u64, StorageError> {
        // Collect keys first (can't mutate while iterating in redb).
        let keys: Vec<String> = {
            let txn = self.db.begin_read()?;
            let table = txn.open_table(KV)?;
            let mut keys = Vec::new();
            for entry in table.range(prefix..)? {
                let (k, _) = entry?;
                let key = k.value();
                if !key.starts_with(prefix) {
                    break;
                }
                keys.push(key.to_string());
            }
            keys
        };

        let count = keys.len() as u64;
        if count > 0 {
            let txn = self.db.begin_write()?;
            {
                let mut table = txn.open_table(KV)?;
                for key in &keys {
                    table.remove(key.as_str())?;
                }
            }
            txn.commit()?;
        }
        Ok(count)
    }

    fn write_transaction(&self, ops: Vec<StorageOp>) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            for op in ops {
                match op {
                    StorageOp::Put { key, value } => {
                        table.insert(key.as_str(), value.as_slice())?;
                    }
                    StorageOp::Delete { key } => {
                        table.remove(key.as_str())?;
                    }
                }
            }
        }
        txn.commit()?;
        Ok(())
    }

    fn list_all_objects(&self) -> Result<Vec<Hash>, StorageError> {
        const PREFIX: &str = "objects/";
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        let mut hashes = Vec::new();
        for entry in table.range(PREFIX..)? {
            let (k, _) = entry?;
            let key = k.value();
            if !key.starts_with(PREFIX) {
                break;
            }
            let hex = &key[PREFIX.len()..];
            match Hash::from_hex(hex) {
                Ok(h) => hashes.push(h),
                Err(_) => {} // skip malformed keys
            }
        }
        Ok(hashes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_object() {
        let db = RedbBackend::ephemeral().unwrap();
        let data = b"hello world";
        let hash = Hash::of(data);
        db.write_object(&hash, data).unwrap();
        let got = db.read_object(&hash).unwrap().unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn cas_succeeds_and_fails() {
        let db = RedbBackend::ephemeral().unwrap();
        let key = "refs/test/repo/heads/main";
        let h1 = Hash::of(b"commit1");
        let h2 = Hash::of(b"commit2");
        let h3 = Hash::of(b"commit3");

        // Seed the ref.
        db.set_ref(key, &h1).unwrap();

        // CAS with correct expected value succeeds.
        assert!(db.compare_and_set_ref(key, &h1, &h2).unwrap());
        assert_eq!(db.get_ref(key).unwrap().unwrap(), h2);

        // CAS with wrong expected value fails.
        assert!(!db.compare_and_set_ref(key, &h1, &h3).unwrap());
        assert_eq!(db.get_ref(key).unwrap().unwrap(), h2);
    }

    #[test]
    fn scan_prefix() {
        let db = RedbBackend::ephemeral().unwrap();
        db.put("search/User/payments/core/user.proto", b"message").unwrap();
        db.put("search/User/payments/core/order.proto", b"message").unwrap();
        db.put("search/Order/payments/core/order.proto", b"message").unwrap();

        let results = db.scan_prefix("search/User/").unwrap();
        assert_eq!(results.len(), 2);
    }
}
