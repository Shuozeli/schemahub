//! RPC-edge idempotency dedupe (design.md §5.1 step 1).
//!
//! A literal network retry carrying the same `idempotency_key` must return the
//! stored result rather than re-applying the mutation. This is *not* durable
//! identity — that is the `ChangeId` the VCS assigns. The store here only
//! collapses retries within the process lifetime; the server may back it with a
//! durable table later.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::request::MutationResponse;

/// A simple keyed cache of prior mutation responses. Thread-safe so it can live
/// behind the shared `Arc<Core>`.
#[derive(Default)]
pub struct IdempotencyStore {
    seen: Mutex<HashMap<String, MutationResponse>>,
}

impl IdempotencyStore {
    pub fn new() -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Return a previously stored result for `key`, if any.
    pub fn get(&self, key: &str) -> Option<MutationResponse> {
        self.seen.lock().expect("idempotency lock").get(key).cloned()
    }

    /// Record the result for `key`.
    pub fn put(&self, key: &str, result: MutationResponse) {
        self.seen
            .lock()
            .expect("idempotency lock")
            .insert(key.to_string(), result);
    }
}
