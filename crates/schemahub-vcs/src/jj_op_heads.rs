//! [`DbOpHeadsStore`] — a real [`jj_lib::op_heads_store::OpHeadsStore`] whose
//! head set is persisted to our [`ObjectDb`] refs.
//!
//! The op-heads store holds the current head(s) of the operation graph — the
//! durable pointer that makes `undo`/`op restore` and reload-at-head work across
//! `Vcs` instances. We keep it in the per-repo ref table (`set_ref`/`get_ref`),
//! newline-joined hex ids. The lock is a no-op (single-writer embedded use); the
//! design (§4.4) follows jj's normal concurrency model.

use std::sync::Arc;

use async_trait::async_trait;
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_heads_store::{OpHeadsStore, OpHeadsStoreError, OpHeadsStoreLock};
use jj_lib::op_store::OperationId;

use crate::object_db::ObjectDb;

/// Ref name under which the newline-joined op-head ids are stored.
const OP_HEADS_REF: &str = "op_heads";

#[derive(Debug)]
pub struct DbOpHeadsStore {
    db: Arc<dyn ObjectDb>,
    repo_key: String,
}

impl DbOpHeadsStore {
    pub fn name() -> &'static str {
        "schemahub-db-op-heads"
    }

    pub fn new(db: Arc<dyn ObjectDb>, repo_key: String) -> Self {
        Self { db, repo_key }
    }

    fn read(&self) -> Result<Vec<OperationId>, OpHeadsStoreError> {
        let bytes = self
            .db
            .get_ref(&self.repo_key, OP_HEADS_REF)
            .map_err(|e| OpHeadsStoreError::Read(Box::new(e)))?;
        let Some(bytes) = bytes else {
            return Ok(vec![]);
        };
        let text = String::from_utf8(bytes).map_err(|e| OpHeadsStoreError::Read(Box::new(e)))?;
        // Fail-fast on a malformed line. Silently dropping bad hex would
        // shrink the head set on the read path and then have `update_op_heads`
        // write the smaller set back, permanently losing the corrupted head
        // rather than surfacing the corruption.
        let mut heads = Vec::new();
        for hex in text.split('\n').filter(|s| !s.is_empty()) {
            let id = OperationId::try_from_hex(hex).ok_or_else(|| {
                OpHeadsStoreError::Read(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("op-heads ref contains malformed hex id: {hex:?}"),
                )))
            })?;
            heads.push(id);
        }
        Ok(heads)
    }

    fn write(&self, ids: &[OperationId]) -> Result<(), OpHeadsStoreError> {
        let text = ids.iter().map(|id| id.hex()).collect::<Vec<_>>().join("\n");
        self.db
            .set_ref(&self.repo_key, OP_HEADS_REF, text.as_bytes())
            .map_err(|e| OpHeadsStoreError::Write {
                new_op_id: ids
                    .last()
                    .cloned()
                    .unwrap_or_else(|| OperationId::new(vec![])),
                source: Box::new(e),
            })
    }
}

struct NoopLock;
impl OpHeadsStoreLock for NoopLock {}

#[async_trait]
impl OpHeadsStore for DbOpHeadsStore {
    fn name(&self) -> &str {
        Self::name()
    }

    async fn update_op_heads(
        &self,
        old_ids: &[OperationId],
        new_id: &OperationId,
    ) -> Result<(), OpHeadsStoreError> {
        let mut heads = self.read()?;
        heads.retain(|id| !old_ids.contains(id));
        if !heads.contains(new_id) {
            heads.push(new_id.clone());
        }
        self.write(&heads)
    }

    async fn get_op_heads(&self) -> Result<Vec<OperationId>, OpHeadsStoreError> {
        self.read()
    }

    async fn lock(&self) -> Result<Box<dyn OpHeadsStoreLock + '_>, OpHeadsStoreError> {
        Ok(Box::new(NoopLock))
    }
}
