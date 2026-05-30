//! The content-addressed object store + op-log + ref-table abstraction
//! (design.md §4.3–§4.5).
//!
//! The jj-style backend/op-store is written against this small trait so the
//! concrete database (`redb` default, in-memory for tests, `postgres` server)
//! is swappable. Object ids are content hashes; the op-log is a per-repo append
//! log of operations; the ref table holds the per-repo "current operation
//! head" pointer (the substrate for undo).

use thiserror::Error;

/// The kind of object being stored. Lets one table serve all object kinds while
/// keeping ids namespaced per kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    /// A per-declaration blob or file-level meta blob (jj `FileId`).
    File,
    /// A content-addressed directory tree (jj `TreeId`).
    Tree,
    /// A commit object (jj `CommitId` — root tree + change id + metadata).
    Commit,
    /// A stored conflict. Retained for backward compatibility; with jj-lib,
    /// conflicts are represented inline as conflicted (multi-side) trees rather
    /// than separate objects, so this kind is currently unused by the backend.
    Conflict,
    /// A symlink target (jj `SymlinkId`). Required to satisfy jj's `Backend`
    /// trait; schemahub never writes symlinks in practice.
    Symlink,
    /// A jj operation-log `View` (bookmarks/tags/heads), content-addressed by
    /// `ViewId`. Stored as an object so views dedup like every other object.
    View,
}

impl ObjectKind {
    /// A stable single-byte tag, prepended to ids so kinds share one keyspace
    /// without colliding.
    pub fn tag(self) -> u8 {
        match self {
            ObjectKind::File => 0,
            ObjectKind::Tree => 1,
            ObjectKind::Commit => 2,
            ObjectKind::Conflict => 3,
            ObjectKind::Symlink => 4,
            ObjectKind::View => 5,
        }
    }
}

/// A content-addressed object id (the hash of the object's bytes).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub Vec<u8>);

impl ObjectId {
    /// Render as lowercase hex (the wire/display form returned to callers).
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// Parse from a hex string.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        Ok(ObjectId(hex::decode(s)?))
    }
}

impl std::fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjectId({})", self.to_hex())
    }
}

/// An operation-log entry id (also a content hash of the operation record).
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct OpId(pub Vec<u8>);

impl OpId {
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        Ok(OpId(hex::decode(s)?))
    }
}

#[derive(Debug, Error)]
pub enum ObjectDbError {
    #[error("object not found")]
    NotFound,
    #[error("database error: {0}")]
    Backend(String),
}

pub type ObjectDbResult<T> = Result<T, ObjectDbError>;

/// Content-addressed object store + op-log + ref persistence.
///
/// Implementations: [`RedbObjectDb`](crate::redb_db::RedbObjectDb) (embedded
/// default) and [`MemoryObjectDb`](crate::memory_db::MemoryObjectDb) (tests).
/// All objects dedup globally by content hash; op-logs and refs are
/// partitioned per `(project, repo)` (passed as a single `repo` key, e.g.
/// `"project/repo"`).
pub trait ObjectDb: std::fmt::Debug + Send + Sync + 'static {
    // ── Content-addressed objects ────────────────────────────────────────────
    /// Store `bytes` under its content hash and return the id. Idempotent: a
    /// second put of identical bytes returns the same id and does not duplicate.
    fn put_object(&self, kind: ObjectKind, bytes: &[u8]) -> ObjectDbResult<ObjectId>;

    /// Store `bytes` under a caller-supplied `id` (jj computes its own
    /// content-addressed ids via blake2b, so the jj `Backend`/`OpStore`
    /// implementations must key objects by *jj's* id, not by our hash).
    /// Idempotent: writing the same id is a no-op.
    fn put_object_at(&self, kind: ObjectKind, id: &ObjectId, bytes: &[u8]) -> ObjectDbResult<()>;

    /// Fetch an object's bytes by id.
    fn get_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<Vec<u8>>;

    /// Whether an object exists (for dedup / gc reachability).
    fn has_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<bool>;

    /// All object ids of a kind (for GC sweep). Returned in unspecified order.
    fn list_objects(&self, kind: ObjectKind) -> ObjectDbResult<Vec<ObjectId>>;

    /// Delete an object (GC sweep). Deleting a missing object is a no-op.
    fn delete_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<()>;

    // ── Operation log (per repo) ──────────────────────────────────────────────
    /// Store an operation record (content-addressed) for a repo and return its
    /// id. Idempotent.
    fn put_op(&self, repo: &str, op_bytes: &[u8]) -> ObjectDbResult<OpId>;

    /// Store an operation record under a caller-supplied `id` (jj computes its
    /// own operation ids). Idempotent.
    fn put_op_at(&self, repo: &str, id: &OpId, op_bytes: &[u8]) -> ObjectDbResult<()>;

    /// Read an operation record by id.
    fn get_op(&self, repo: &str, id: &OpId) -> ObjectDbResult<Vec<u8>>;

    /// All operation ids stored for a repo (unordered; order is reconstructed
    /// from the operations' parent links).
    fn list_ops(&self, repo: &str) -> ObjectDbResult<Vec<OpId>>;

    // ── Refs (per repo) ───────────────────────────────────────────────────────
    /// Set a named ref for a repo (used for the operation-head pointer).
    fn set_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<()>;

    /// Get a named ref's value, or `None` if unset.
    fn get_ref(&self, repo: &str, name: &str) -> ObjectDbResult<Option<Vec<u8>>>;
}
