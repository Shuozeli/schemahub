//! RPC-edge idempotency dedupe (design.md §5.1 step 1).
//!
//! A literal network retry carrying the same `idempotency_key` must return the
//! stored result rather than re-applying the mutation. This is *not* durable
//! identity — that is the `ChangeId` the VCS assigns. The store here only
//! collapses retries within the process lifetime; the server may back it with a
//! durable table later.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::request::MutationResponse;

/// Default maximum number of distinct idempotency keys retained. The previous
/// behaviour (unbounded `HashMap`) was a slow memory leak on a long-running
/// server — every unique key accumulated a full `MutationResponse` forever.
/// Bounded at 1024 entries by default, which is more than enough for typical
/// burst-then-quiesce retry windows; oldest insertion is evicted when full.
pub const DEFAULT_MAX_ENTRIES: usize = 1024;

/// A bounded keyed cache of prior mutation responses, evicting the
/// oldest-inserted entry when the cap is hit. Thread-safe so it can live
/// behind the shared `Arc<Core>`.
pub struct IdempotencyStore {
    inner: Mutex<Inner>,
}

struct Inner {
    seen: HashMap<String, MutationResponse>,
    order: VecDeque<String>,
    max_entries: usize,
}

impl IdempotencyStore {
    /// Build a store with the default cap.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_ENTRIES)
    }

    /// Build a store with an explicit per-process cap. A `max_entries` of 0
    /// disables caching (every call goes through to the mutation flow).
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                seen: HashMap::new(),
                order: VecDeque::new(),
                max_entries,
            }),
        }
    }

    /// Return a previously stored result for `key`, if any.
    pub fn get(&self, key: &str) -> Option<MutationResponse> {
        self.inner
            .lock()
            .expect("idempotency lock")
            .seen
            .get(key)
            .cloned()
    }

    /// Record the result for `key`. Evicts the oldest entry when the cap is
    /// hit. Updating an existing key keeps its original insertion position
    /// (the cache is bounded by *distinct* keys, not by writes).
    pub fn put(&self, key: &str, result: MutationResponse) {
        let mut g = self.inner.lock().expect("idempotency lock");
        if g.max_entries == 0 {
            return;
        }
        if !g.seen.contains_key(key) {
            // New key: enforce the cap by evicting the oldest insertion.
            while g.seen.len() >= g.max_entries {
                if let Some(stale) = g.order.pop_front() {
                    g.seen.remove(&stale);
                } else {
                    break;
                }
            }
            g.order.push_back(key.to_string());
        }
        g.seen.insert(key.to_string(), result);
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(commit: &str) -> MutationResponse {
        MutationResponse {
            commit_id: commit.to_string(),
            change_id: String::new(),
            conflicted_decls: vec![],
        }
    }

    #[test]
    fn put_then_get_returns_stored_value() {
        // Arrange
        let store = IdempotencyStore::new();

        // Act
        store.put("k1", resp("commit-1"));

        // Assert
        assert_eq!(
            store.get("k1").map(|r| r.commit_id),
            Some("commit-1".to_string())
        );
    }

    #[test]
    fn capacity_zero_disables_caching() {
        // Arrange
        let store = IdempotencyStore::with_capacity(0);

        // Act
        store.put("k", resp("c"));

        // Assert
        assert!(store.get("k").is_none());
    }

    #[test]
    fn cap_evicts_oldest_insertion_first() {
        // Arrange
        let store = IdempotencyStore::with_capacity(2);
        store.put("k1", resp("c1"));
        store.put("k2", resp("c2"));

        // Act
        store.put("k3", resp("c3"));

        // Assert
        assert!(store.get("k1").is_none(), "oldest must have been evicted");
        assert!(store.get("k2").is_some());
        assert!(store.get("k3").is_some());
    }

    #[test]
    fn updating_existing_key_does_not_evict() {
        // Arrange
        let store = IdempotencyStore::with_capacity(2);
        store.put("k1", resp("c1"));
        store.put("k2", resp("c2"));

        // Act: re-put k1 — must not push k1 to the "newest" slot in a way
        // that evicts k2 spuriously.
        store.put("k1", resp("c1-updated"));

        // Assert: both still present; the next *new* insertion evicts k1
        // (the older one), proving the order tracker wasn't touched.
        assert_eq!(
            store.get("k1").map(|r| r.commit_id),
            Some("c1-updated".to_string())
        );
        store.put("k3", resp("c3"));
        assert!(
            store.get("k1").is_none(),
            "k1 was inserted first; must evict first"
        );
        assert!(store.get("k2").is_some());
        assert!(store.get("k3").is_some());
    }
}
