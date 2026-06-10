//! [`DbBackend`] — a real [`jj_lib::backend::Backend`] persisted to our
//! [`ObjectDb`] (design.md §4.3).
//!
//! This is the genuine jj-lib commit backend. jj computes content-addressed ids
//! itself (blake2b over the `ContentHash` of each object); we store the bytes
//! keyed by *jj's* id via [`ObjectDb::put_object_at`]. Serialization mirrors
//! jj-lib's own `SimpleBackend` (proto-encoded trees/commits) so the on-disk
//! layout is faithful to jj.
//!
//! Files/trees/commits go to [`ObjectKind::File`]/`Tree`/`Commit`; symlinks to
//! `Symlink` (required by the trait but never used by schemahub). Copy history
//! and git submodules are `Unsupported`, exactly as `SimpleBackend` does — we
//! bypass jj's git interop and working copy (design.md §4.6).

use std::io::Cursor;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use blake2::Blake2b512;
use blake2::Digest as _;
use futures::stream;
use futures::stream::BoxStream;
use futures::StreamExt as _;
use jj_lib::backend::{
    make_root_commit, Backend, BackendError, BackendResult, ChangeId, Commit, CommitId,
    CopyHistory, CopyId, CopyRecord, FileId, MillisSinceEpoch, SecureSig, Signature, SigningFn,
    SymlinkId, Timestamp, Tree, TreeId, TreeValue,
};
use jj_lib::conflict_labels::ConflictLabels;
use jj_lib::content_hash::blake2b_hash;
use jj_lib::index::Index;
use jj_lib::merge::MergeBuilder;
use jj_lib::object_id::ObjectId as _;
use jj_lib::protos::simple_store as protos;
use jj_lib::repo_path::{RepoPath, RepoPathBuf, RepoPathComponentBuf};
use prost::Message as _;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::object_db::{ObjectDb, ObjectId, ObjectKind};

/// Commit-id length in bytes (blake2b-512). Matches jj's `SimpleBackend`.
const COMMIT_ID_LENGTH: usize = 64;
/// Change-id length in bytes. Matches jj's `SimpleBackend`.
const CHANGE_ID_LENGTH: usize = 16;

/// The blake2b hash of the empty `Tree` (same value jj's `SimpleBackend` pins).
const EMPTY_TREE_ID_HEX: &str = "482ae5a29fbe856c7272f2071b8b0f0359ee2d89ff392b8a900643fbd0836eccd067b8bf41909e206c90d45d6e7d8b6686b93ecaee5fe1a9060d87b672101310";

fn to_other_err(err: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> BackendError {
    BackendError::Other(err.into())
}

fn not_found(
    object_type: &str,
    hash: String,
    err: crate::object_db::ObjectDbError,
) -> BackendError {
    BackendError::ObjectNotFound {
        object_type: object_type.to_string(),
        hash,
        source: Box::new(err),
    }
}

/// A jj commit backend whose objects live in an [`ObjectDb`].
#[derive(Debug)]
pub struct DbBackend {
    db: Arc<dyn ObjectDb>,
    root_commit_id: CommitId,
    root_change_id: ChangeId,
    empty_tree_id: TreeId,
}

impl DbBackend {
    /// The jj backend type name written to the store metadata.
    pub fn name() -> &'static str {
        "schemahub-db"
    }

    /// Build a backend over `db`. The empty tree is materialized eagerly so the
    /// root view's empty root tree resolves.
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        let backend = Self {
            db,
            root_commit_id: CommitId::from_bytes(&[0; COMMIT_ID_LENGTH]),
            root_change_id: ChangeId::from_bytes(&[0; CHANGE_ID_LENGTH]),
            empty_tree_id: TreeId::from_hex(EMPTY_TREE_ID_HEX),
        };
        // Persist the empty tree so reads of the root tree succeed.
        let proto = tree_to_proto(&Tree::default());
        let _ = backend.db.put_object_at(
            ObjectKind::Tree,
            &ObjectId(backend.empty_tree_id.to_bytes()),
            &proto.encode_to_vec(),
        );
        backend
    }
}

#[async_trait]
impl Backend for DbBackend {
    fn name(&self) -> &str {
        Self::name()
    }

    fn commit_id_length(&self) -> usize {
        COMMIT_ID_LENGTH
    }

    fn change_id_length(&self) -> usize {
        CHANGE_ID_LENGTH
    }

    fn root_commit_id(&self) -> &CommitId {
        &self.root_commit_id
    }

    fn root_change_id(&self) -> &ChangeId {
        &self.root_change_id
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    fn concurrency(&self) -> usize {
        1
    }

    async fn read_file(
        &self,
        _path: &RepoPath,
        id: &FileId,
    ) -> BackendResult<Pin<Box<dyn AsyncRead + Send>>> {
        let bytes = self
            .db
            .get_object(ObjectKind::File, &ObjectId(id.to_bytes()))
            .map_err(|e| not_found("file", id.hex(), e))?;
        Ok(Box::pin(Cursor::new(bytes)))
    }

    async fn write_file(
        &self,
        _path: &RepoPath,
        contents: &mut (dyn AsyncRead + Send + Unpin),
    ) -> BackendResult<FileId> {
        let mut buf = Vec::new();
        contents.read_to_end(&mut buf).await.map_err(to_other_err)?;
        let id = FileId::new(Blake2b512::digest(&buf).to_vec());
        self.db
            .put_object_at(ObjectKind::File, &ObjectId(id.to_bytes()), &buf)
            .map_err(to_other_err)?;
        Ok(id)
    }

    async fn read_symlink(&self, _path: &RepoPath, id: &SymlinkId) -> BackendResult<String> {
        let bytes = self
            .db
            .get_object(ObjectKind::Symlink, &ObjectId(id.to_bytes()))
            .map_err(|e| not_found("symlink", id.hex(), e))?;
        String::from_utf8(bytes).map_err(to_other_err)
    }

    async fn write_symlink(&self, _path: &RepoPath, target: &str) -> BackendResult<SymlinkId> {
        let id = SymlinkId::new(Blake2b512::digest(target.as_bytes()).to_vec());
        self.db
            .put_object_at(
                ObjectKind::Symlink,
                &ObjectId(id.to_bytes()),
                target.as_bytes(),
            )
            .map_err(to_other_err)?;
        Ok(id)
    }

    async fn read_copy(&self, _id: &CopyId) -> BackendResult<CopyHistory> {
        Err(BackendError::Unsupported(
            "schemahub backend does not support copy tracking".to_string(),
        ))
    }

    async fn write_copy(&self, _copy: &CopyHistory) -> BackendResult<CopyId> {
        Err(BackendError::Unsupported(
            "schemahub backend does not support copy tracking".to_string(),
        ))
    }

    async fn get_related_copies(
        &self,
        _copy_id: &CopyId,
    ) -> BackendResult<Vec<jj_lib::backend::RelatedCopy>> {
        Err(BackendError::Unsupported(
            "schemahub backend does not support copy tracking".to_string(),
        ))
    }

    async fn read_tree(&self, _path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        let bytes = self
            .db
            .get_object(ObjectKind::Tree, &ObjectId(id.to_bytes()))
            .map_err(|e| not_found("tree", id.hex(), e))?;
        let proto = protos::Tree::decode(&*bytes).map_err(to_other_err)?;
        tree_from_proto(proto)
    }

    async fn write_tree(&self, _path: &RepoPath, tree: &Tree) -> BackendResult<TreeId> {
        let proto = tree_to_proto(tree);
        let id = TreeId::new(blake2b_hash(tree).to_vec());
        self.db
            .put_object_at(
                ObjectKind::Tree,
                &ObjectId(id.to_bytes()),
                &proto.encode_to_vec(),
            )
            .map_err(to_other_err)?;
        Ok(id)
    }

    async fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        if *id == self.root_commit_id {
            return Ok(make_root_commit(
                self.root_change_id.clone(),
                self.empty_tree_id.clone(),
            ));
        }
        let bytes = self
            .db
            .get_object(ObjectKind::Commit, &ObjectId(id.to_bytes()))
            .map_err(|e| not_found("commit", id.hex(), e))?;
        let proto = protos::Commit::decode(&*bytes).map_err(to_other_err)?;
        Ok(commit_from_proto(proto))
    }

    async fn write_commit(
        &self,
        mut commit: Commit,
        sign_with: Option<&mut SigningFn>,
    ) -> BackendResult<(CommitId, Commit)> {
        assert!(commit.secure_sig.is_none(), "commit.secure_sig was set");
        if commit.parents.is_empty() {
            return Err(BackendError::Other(
                "Cannot write a commit with no parents".into(),
            ));
        }
        let mut proto = commit_to_proto(&commit);
        if let Some(sign) = sign_with {
            let data = proto.encode_to_vec();
            let sig = sign(&data).map_err(to_other_err)?;
            proto.secure_sig = Some(sig.clone());
            commit.secure_sig = Some(SecureSig { data, sig });
        }
        let id = CommitId::new(blake2b_hash(&commit).to_vec());
        self.db
            .put_object_at(
                ObjectKind::Commit,
                &ObjectId(id.to_bytes()),
                &proto.encode_to_vec(),
            )
            .map_err(to_other_err)?;
        Ok((id, commit))
    }

    fn get_copy_records(
        &self,
        _paths: Option<&[RepoPathBuf]>,
        _root: &CommitId,
        _head: &CommitId,
    ) -> BackendResult<BoxStream<'_, BackendResult<CopyRecord>>> {
        Ok(stream::empty().boxed())
    }

    fn gc(&self, _index: &dyn Index, _keep_newer: SystemTime) -> BackendResult<()> {
        // Reachability-based GC is implemented at the schemahub `Jj::gc` level
        // over the `ObjectDb`; jj's per-backend gc is a no-op here.
        Ok(())
    }
}

// ── proto conversions (faithful to jj-lib's SimpleBackend) ────────────────────

fn tree_to_proto(tree: &Tree) -> protos::Tree {
    let mut proto = protos::Tree::default();
    for entry in tree.entries() {
        proto.entries.push(protos::tree::Entry {
            name: entry.name().as_internal_str().to_owned(),
            value: Some(tree_value_to_proto(entry.value())),
        });
    }
    proto
}

fn tree_from_proto(proto: protos::Tree) -> BackendResult<Tree> {
    let mut entries = Vec::with_capacity(proto.entries.len());
    for proto_entry in proto.entries {
        // The on-disk tree bytes are content-addressed by jj's hash, so a
        // missing oneof / malformed component name means the object store
        // returned bytes that don't decode to a valid Tree. Surface that as
        // a backend error instead of panicking inside a server worker.
        let raw_value = proto_entry.value.ok_or_else(|| {
            to_other_err(format!(
                "malformed tree entry {:?}: missing tree_value oneof",
                proto_entry.name
            ))
        })?;
        let value = tree_value_from_proto(raw_value)?;
        let name = RepoPathComponentBuf::new(proto_entry.name.clone()).map_err(|e| {
            to_other_err(format!(
                "malformed tree entry name {:?}: {e}",
                proto_entry.name
            ))
        })?;
        entries.push((name, value));
    }
    Ok(Tree::from_sorted_entries(entries))
}

fn tree_value_to_proto(value: &TreeValue) -> protos::TreeValue {
    let mut proto = protos::TreeValue::default();
    match value {
        TreeValue::File {
            id,
            executable,
            copy_id,
        } => {
            proto.value = Some(protos::tree_value::Value::File(protos::tree_value::File {
                id: id.to_bytes(),
                executable: *executable,
                copy_id: copy_id.to_bytes(),
            }));
        }
        TreeValue::Symlink(id) => {
            proto.value = Some(protos::tree_value::Value::SymlinkId(id.to_bytes()));
        }
        TreeValue::GitSubmodule(_id) => {
            panic!("cannot store git submodules in the schemahub backend");
        }
        TreeValue::Tree(id) => {
            proto.value = Some(protos::tree_value::Value::TreeId(id.to_bytes()));
        }
    }
    proto
}

fn tree_value_from_proto(proto: protos::TreeValue) -> BackendResult<TreeValue> {
    let value = proto
        .value
        .ok_or_else(|| to_other_err("malformed tree value: missing oneof".to_string()))?;
    Ok(match value {
        protos::tree_value::Value::TreeId(id) => TreeValue::Tree(TreeId::new(id)),
        protos::tree_value::Value::File(protos::tree_value::File {
            id,
            executable,
            copy_id,
        }) => TreeValue::File {
            id: FileId::new(id),
            executable,
            copy_id: CopyId::new(copy_id),
        },
        protos::tree_value::Value::SymlinkId(id) => TreeValue::Symlink(SymlinkId::new(id)),
    })
}

fn commit_to_proto(commit: &Commit) -> protos::Commit {
    let mut proto = protos::Commit::default();
    for parent in &commit.parents {
        proto.parents.push(parent.to_bytes());
    }
    for predecessor in &commit.predecessors {
        proto.predecessors.push(predecessor.to_bytes());
    }
    proto.root_tree = commit.root_tree.iter().map(|id| id.to_bytes()).collect();
    if !commit.conflict_labels.is_resolved() {
        proto.conflict_labels = commit.conflict_labels.as_slice().to_owned();
    }
    proto.change_id = commit.change_id.to_bytes();
    proto.description = commit.description.clone();
    proto.author = Some(signature_to_proto(&commit.author));
    proto.committer = Some(signature_to_proto(&commit.committer));
    proto
}

fn commit_from_proto(mut proto: protos::Commit) -> Commit {
    let secure_sig = proto.secure_sig.take().map(|sig| SecureSig {
        data: proto.encode_to_vec(),
        sig,
    });
    let parents = proto.parents.into_iter().map(CommitId::new).collect();
    let predecessors = proto.predecessors.into_iter().map(CommitId::new).collect();
    let merge_builder: MergeBuilder<_> = proto.root_tree.into_iter().map(TreeId::new).collect();
    let root_tree = merge_builder.build();
    let conflict_labels = ConflictLabels::from_vec(proto.conflict_labels);
    let change_id = ChangeId::new(proto.change_id);
    Commit {
        parents,
        predecessors,
        root_tree,
        conflict_labels: conflict_labels.into_merge(),
        change_id,
        description: proto.description,
        author: signature_from_proto(proto.author.unwrap_or_default()),
        committer: signature_from_proto(proto.committer.unwrap_or_default()),
        secure_sig,
    }
}

fn signature_to_proto(signature: &Signature) -> protos::commit::Signature {
    protos::commit::Signature {
        name: signature.name.clone(),
        email: signature.email.clone(),
        timestamp: Some(protos::commit::Timestamp {
            millis_since_epoch: signature.timestamp.timestamp.0,
            tz_offset: signature.timestamp.tz_offset,
        }),
    }
}

fn signature_from_proto(proto: protos::commit::Signature) -> Signature {
    let timestamp = proto.timestamp.unwrap_or_default();
    Signature {
        name: proto.name,
        email: proto.email,
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(timestamp.millis_since_epoch),
            tz_offset: timestamp.tz_offset,
        },
    }
}
