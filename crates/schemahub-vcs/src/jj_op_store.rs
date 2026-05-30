//! [`DbOpStore`] — a real [`jj_lib::op_store::OpStore`] persisted to our
//! [`ObjectDb`] (design.md §4.4): the operation log.
//!
//! Every schemahub write is one jj [`Operation`] over a [`View`] (bookmarks,
//! tags, heads). We persist operations through [`ObjectDb::put_op`] (per-repo,
//! the audit log) and views as content-addressed [`ObjectKind::View`] objects.
//! Ids are jj's own blake2b hashes (`blake2b_hash`), matching jj's
//! `SimpleOpStore`, so they round-trip through jj's `RepoLoader`.
//!
//! schemahub uses git interop nor remotes, so we serialize only the
//! fields jj's `View` exposes that we touch — head ids, local bookmarks, local
//! tags, and workspace working-copy pointers (kept for fidelity even though we
//! never create a working copy). Remote/git fields are always empty.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use jj_lib::backend::{CommitId, MillisSinceEpoch, Timestamp};
use jj_lib::content_hash::blake2b_hash;
use jj_lib::object_id::{HexPrefix, ObjectId as _, PrefixResolution};
use jj_lib::op_store::{
    Operation, OperationId, OperationMetadata, OpStore, OpStoreError, OpStoreResult, RefTarget,
    RootOperationData, TimestampRange, View, ViewId,
};
use jj_lib::merge::Merge;
use jj_lib::ref_name::{RefNameBuf, WorkspaceNameBuf};
use serde::{Deserialize, Serialize};

use crate::object_db::{ObjectDb, ObjectId, ObjectKind};

const OPERATION_ID_LENGTH: usize = 64;
const VIEW_ID_LENGTH: usize = 64;

fn to_read_err(object_type: &str, hash: String, err: impl std::error::Error + Send + Sync + 'static) -> OpStoreError {
    OpStoreError::ReadObject {
        object_type: object_type.to_string(),
        hash,
        source: Box::new(err),
    }
}

fn not_found(object_type: &str, hash: String, err: crate::object_db::ObjectDbError) -> OpStoreError {
    OpStoreError::ObjectNotFound {
        object_type: object_type.to_string(),
        hash,
        source: Box::new(err),
    }
}

fn write_err(object_type: &'static str, err: crate::object_db::ObjectDbError) -> OpStoreError {
    OpStoreError::WriteObject {
        object_type,
        source: Box::new(err),
    }
}

/// A jj operation store whose operations + views live in an [`ObjectDb`],
/// scoped to one `(project, repo)` via the `repo_key`.
#[derive(Debug)]
pub struct DbOpStore {
    db: Arc<dyn ObjectDb>,
    repo_key: String,
    root_data: RootOperationData,
    root_operation_id: OperationId,
    root_view_id: ViewId,
}

impl DbOpStore {
    pub fn name() -> &'static str {
        "schemahub-db-op-store"
    }

    pub fn new(db: Arc<dyn ObjectDb>, repo_key: String, root_data: RootOperationData) -> Self {
        Self {
            db,
            repo_key,
            root_data,
            root_operation_id: OperationId::from_bytes(&[0; OPERATION_ID_LENGTH]),
            root_view_id: ViewId::from_bytes(&[0; VIEW_ID_LENGTH]),
        }
    }
}

#[async_trait]
impl OpStore for DbOpStore {
    fn name(&self) -> &str {
        Self::name()
    }

    fn root_operation_id(&self) -> &OperationId {
        &self.root_operation_id
    }

    async fn read_view(&self, id: &ViewId) -> OpStoreResult<View> {
        if *id == self.root_view_id {
            return Ok(View::make_root(self.root_data.root_commit_id.clone()));
        }
        let bytes = self
            .db
            .get_object(ObjectKind::View, &ObjectId(id.to_bytes()))
            .map_err(|e| not_found("view", id.hex(), e))?;
        let stored: StoredView =
            serde_json::from_slice(&bytes).map_err(|e| to_read_err("view", id.hex(), e))?;
        Ok(stored.into_view())
    }

    async fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        let stored = StoredView::from_view(view);
        let bytes = serde_json::to_vec(&stored)
            .map_err(|e| OpStoreError::Other(Box::new(e)))?;
        // jj's id is the blake2b of the View's ContentHash — compute it so the
        // id is identical to what jj's SimpleOpStore would assign.
        let id = ViewId::new(blake2b_hash(view).to_vec());
        self.db
            .put_object_at(ObjectKind::View, &ObjectId(id.to_bytes()), &bytes)
            .map_err(|e| write_err("view", e))?;
        Ok(id)
    }

    async fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        if *id == self.root_operation_id {
            return Ok(Operation::make_root(self.root_view_id.clone()));
        }
        let bytes = self
            .db
            .get_op(&self.repo_key, &crate::object_db::OpId(id.to_bytes()))
            .map_err(|e| not_found("operation", id.hex(), e))?;
        let stored: StoredOperation =
            serde_json::from_slice(&bytes).map_err(|e| to_read_err("operation", id.hex(), e))?;
        Ok(stored.into_operation())
    }

    async fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        assert!(!operation.parents.is_empty());
        let stored = StoredOperation::from_operation(operation);
        let bytes = serde_json::to_vec(&stored)
            .map_err(|e| OpStoreError::Other(Box::new(e)))?;
        let id = OperationId::new(blake2b_hash(operation).to_vec());
        self.db
            .put_op_at(&self.repo_key, &crate::object_db::OpId(id.to_bytes()), &bytes)
            .map_err(|e| write_err("operation", e))?;
        Ok(id)
    }

    async fn resolve_operation_id_prefix(
        &self,
        prefix: &HexPrefix,
    ) -> OpStoreResult<PrefixResolution<OperationId>> {
        let hex_prefix = prefix.hex();
        let mut matched = prefix
            .matches(&self.root_operation_id)
            .then(|| self.root_operation_id.clone());
        let op_ids = self
            .db
            .list_ops(&self.repo_key)
            .map_err(|e| OpStoreError::Other(Box::new(e)))?;
        for op_id in op_ids {
            let id = OperationId::new(op_id.0);
            if !id.hex().starts_with(&hex_prefix) {
                continue;
            }
            if matched.is_some() {
                return Ok(PrefixResolution::AmbiguousMatch);
            }
            matched = Some(id);
        }
        Ok(match matched {
            Some(id) => PrefixResolution::SingleMatch(id),
            None => PrefixResolution::NoMatch,
        })
    }

    async fn gc(&self, _head_ids: &[OperationId], _keep_newer: SystemTime) -> OpStoreResult<()> {
        // Op-log retention is required for `undo`; schemahub keeps the full
        // op-log and runs object GC at the `Vcs::gc` level instead.
        Ok(())
    }
}

// ── Serde-friendly reduced representations ────────────────────────────────────
//
// jj's `View`/`Operation` don't round-trip through serde (they skip fields and
// don't derive `Deserialize`), so we persist a reduced form covering exactly the
// fields schemahub uses. Remote/git refs are always empty in schemahub.

/// A ref target as alternating merge terms (`Vec<Option<commit-hex>>`), matching
/// jj's `Merge<Option<CommitId>>`. A resolved target is a single term.
type StoredRefTarget = Vec<Option<String>>;

fn ref_target_to_stored(target: &RefTarget) -> StoredRefTarget {
    target
        .as_merge()
        .iter()
        .map(|opt| opt.as_ref().map(|id| id.hex()))
        .collect()
}

fn ref_target_from_stored(stored: StoredRefTarget) -> RefTarget {
    let terms: Vec<Option<CommitId>> = stored
        .into_iter()
        .map(|opt| opt.map(|hex| CommitId::new(hex_to_bytes(&hex))))
        .collect();
    RefTarget::from_merge(Merge::from_vec(terms))
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    hex::decode(hex).unwrap_or_default()
}

#[derive(Serialize, Deserialize)]
struct StoredView {
    head_ids: Vec<String>,
    local_bookmarks: Vec<(String, StoredRefTarget)>,
    local_tags: Vec<(String, StoredRefTarget)>,
    wc_commit_ids: Vec<(String, String)>,
}

impl StoredView {
    fn from_view(view: &View) -> Self {
        Self {
            head_ids: view.head_ids.iter().map(|id| id.hex()).collect(),
            local_bookmarks: view
                .local_bookmarks
                .iter()
                .map(|(name, target)| (name.as_str().to_owned(), ref_target_to_stored(target)))
                .collect(),
            local_tags: view
                .local_tags
                .iter()
                .map(|(name, target)| (name.as_str().to_owned(), ref_target_to_stored(target)))
                .collect(),
            wc_commit_ids: view
                .wc_commit_ids
                .iter()
                .map(|(name, id)| (name.as_str().to_owned(), id.hex()))
                .collect(),
        }
    }

    fn into_view(self) -> View {
        View {
            head_ids: self
                .head_ids
                .into_iter()
                .map(|h| CommitId::new(hex_to_bytes(&h)))
                .collect(),
            local_bookmarks: self
                .local_bookmarks
                .into_iter()
                .map(|(name, target)| (RefNameBuf::from(name), ref_target_from_stored(target)))
                .collect(),
            local_tags: self
                .local_tags
                .into_iter()
                .map(|(name, target)| (RefNameBuf::from(name), ref_target_from_stored(target)))
                .collect(),
            remote_views: BTreeMap::new(),
            git_refs: BTreeMap::new(),
            git_head: RefTarget::absent(),
            wc_commit_ids: self
                .wc_commit_ids
                .into_iter()
                .map(|(name, id)| (WorkspaceNameBuf::from(name), CommitId::new(hex_to_bytes(&id))))
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredOperation {
    view_id: String,
    parents: Vec<String>,
    start_millis: i64,
    start_tz: i32,
    end_millis: i64,
    end_tz: i32,
    description: String,
    hostname: String,
    username: String,
    is_snapshot: bool,
    workspace_name: Option<String>,
    attributes: Vec<(String, String)>,
}

impl StoredOperation {
    fn from_operation(op: &Operation) -> Self {
        Self {
            view_id: op.view_id.hex(),
            parents: op.parents.iter().map(|id| id.hex()).collect(),
            start_millis: op.metadata.time.start.timestamp.0,
            start_tz: op.metadata.time.start.tz_offset,
            end_millis: op.metadata.time.end.timestamp.0,
            end_tz: op.metadata.time.end.tz_offset,
            description: op.metadata.description.clone(),
            hostname: op.metadata.hostname.clone(),
            username: op.metadata.username.clone(),
            is_snapshot: op.metadata.is_snapshot,
            workspace_name: op.metadata.workspace_name.as_ref().map(|n| n.as_str().to_owned()),
            attributes: op.metadata.attributes.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    fn into_operation(self) -> Operation {
        let metadata = OperationMetadata {
            time: TimestampRange {
                start: Timestamp {
                    timestamp: MillisSinceEpoch(self.start_millis),
                    tz_offset: self.start_tz,
                },
                end: Timestamp {
                    timestamp: MillisSinceEpoch(self.end_millis),
                    tz_offset: self.end_tz,
                },
            },
            description: self.description,
            hostname: self.hostname,
            username: self.username,
            is_snapshot: self.is_snapshot,
            workspace_name: self.workspace_name.map(WorkspaceNameBuf::from),
            attributes: self.attributes.into_iter().collect(),
        };
        Operation {
            view_id: ViewId::new(hex_to_bytes(&self.view_id)),
            parents: self.parents.into_iter().map(|h| OperationId::new(hex_to_bytes(&h))).collect(),
            metadata,
            // schemahub doesn't track commit predecessors across operations.
            commit_predecessors: None,
        }
    }
}

/// Marker used by `HashSet`-based reachability in case it's needed later.
#[allow(dead_code)]
type _ReachableViews = HashSet<ViewId>;
