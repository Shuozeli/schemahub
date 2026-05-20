use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use prost::Message;
use schemahub_types::Hash;

use crate::error::CoreError;

// ── Prost-encoded internal object types ──────────────────────────────────────

/// A commit in the object store.
#[derive(Clone, PartialEq, prost::Message)]
pub struct CommitObject {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    /// Hex hash of root TreeObject.
    #[prost(string, tag = "2")]
    pub tree_hash: String,
    #[prost(string, repeated, tag = "3")]
    pub parent_hashes: Vec<String>,
    #[prost(int64, tag = "4")]
    pub timestamp_unix: i64,
    #[prost(string, tag = "5")]
    pub author: String,
    #[prost(string, tag = "6")]
    pub message: String,
    #[prost(bool, tag = "7")]
    pub force: bool,
    #[prost(string, tag = "8")]
    pub format_id: String,
    /// For GC age threshold.
    #[prost(int64, tag = "9")]
    pub created_at_unix: i64,
}

/// TreeEntry kind: 0 = SubTree, 1 = Blob
#[derive(Clone, PartialEq, prost::Message)]
pub struct TreeEntryProto {
    #[prost(string, tag = "1")]
    pub name: String,
    /// 0 = SubTree, 1 = Blob
    #[prost(int32, tag = "2")]
    pub kind: i32,
    /// Hex hash.
    #[prost(string, tag = "3")]
    pub hash: String,
}

/// A tree object (directory) in the object store.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TreeObject {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    /// Sorted by name.
    #[prost(message, repeated, tag = "2")]
    pub entries: Vec<TreeEntryProto>,
    #[prost(int64, tag = "3")]
    pub created_at_unix: i64,
}

/// A tag object in the object store.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TagObject {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    /// Hex hash of the commit.
    #[prost(string, tag = "2")]
    pub commit_hash: String,
    #[prost(string, tag = "3")]
    pub tagger: String,
    #[prost(int64, tag = "4")]
    pub timestamp_unix: i64,
    #[prost(string, tag = "5")]
    pub message: String,
    #[prost(int64, tag = "6")]
    pub created_at_unix: i64,
}

// ── Tree entry kinds ──────────────────────────────────────────────────────────

pub const KIND_SUBTREE: i32 = 0;
pub const KIND_BLOB: i32 = 1;

// ── Encode / decode helpers ───────────────────────────────────────────────────

pub fn encode_commit(c: &CommitObject) -> Vec<u8> {
    c.encode_to_vec()
}

pub fn decode_commit(data: &[u8]) -> Result<CommitObject, CoreError> {
    CommitObject::decode(data)
        .map_err(|e| CoreError::ObjectCorrupted(format!("commit decode failed: {e}")))
}

pub fn encode_tree(t: &TreeObject) -> Vec<u8> {
    t.encode_to_vec()
}

pub fn decode_tree(data: &[u8]) -> Result<TreeObject, CoreError> {
    TreeObject::decode(data)
        .map_err(|e| CoreError::ObjectCorrupted(format!("tree decode failed: {e}")))
}

pub fn encode_tag(t: &TagObject) -> Vec<u8> {
    t.encode_to_vec()
}

pub fn decode_tag(data: &[u8]) -> Result<TagObject, CoreError> {
    TagObject::decode(data)
        .map_err(|e| CoreError::ObjectCorrupted(format!("tag decode failed: {e}")))
}

/// Build a TreeObject from a HashMap<name, hash_hex>.
/// Entries are sorted by name for determinism.
pub fn tree_from_map(entries: HashMap<String, String>, kind: i32) -> TreeObject {
    let now = unix_now();
    let mut sorted: Vec<(String, String)> = entries.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    TreeObject {
        blob_version: 1,
        entries: sorted
            .into_iter()
            .map(|(name, hash)| TreeEntryProto { name, kind, hash })
            .collect(),
        created_at_unix: now,
    }
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Compute the content hash for an encoded object.
pub fn hash_of_bytes(data: &[u8]) -> Hash {
    Hash::of(data)
}
