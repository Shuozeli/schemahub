//! Stable, versioned blob (de)serialization for the OpenAPI compiler.
//!
//! Encoding choice: `serde_json`. Rationale — the in-tree AST is small,
//! human-debuggable, and serde_json is already a crate dependency (no `bincode`
//! or `prost` build step needed). Determinism: serde serializes struct fields
//! in declaration order and `serde_json` does not reorder, so identical ASTs
//! produce identical bytes. The `blob_version` field (see [`crate::ast`]) rides
//! inside each encoded value for migration.

use schemahub_types::blob::{DeclBlob, MetaBlob};
use schemahub_types::errors::{DiffError, PrintError, ReadError};

use crate::ast::{DocumentMetadataBlob, OpenApiDecl, BLOB_VERSION};

// ── Decl blobs ──────────────────────────────────────────────────────────────────

/// Serialize a self-describing decl payload into a [`DeclBlob`].
pub fn encode_decl(decl: &OpenApiDecl) -> DeclBlob {
    // serde_json::to_vec on a derive-Serialize type cannot fail.
    DeclBlob::new(serde_json::to_vec(decl).expect("OpenApiDecl serialization is infallible"))
}

/// Deserialize a [`DeclBlob`] into an [`OpenApiDecl`], rejecting unknown versions.
pub fn decode_decl(blob: &DeclBlob) -> Result<OpenApiDecl, BlobError> {
    let decl: OpenApiDecl = serde_json::from_slice(blob.as_bytes())
        .map_err(|e| BlobError(format!("OpenApiDecl: {e}")))?;
    if decl.blob_version > BLOB_VERSION {
        return Err(BlobError(format!(
            "unsupported decl blob_version {} (max {})",
            decl.blob_version, BLOB_VERSION
        )));
    }
    Ok(decl)
}

// ── Meta blob ───────────────────────────────────────────────────────────────────

/// Serialize document metadata into a [`MetaBlob`].
pub fn encode_meta(meta: &DocumentMetadataBlob) -> MetaBlob {
    MetaBlob::new(serde_json::to_vec(meta).expect("DocumentMetadataBlob serialization is infallible"))
}

/// Deserialize a [`MetaBlob`] into a [`DocumentMetadataBlob`].
///
/// An empty meta blob (the `MetaBlob::default()` produced before any parse)
/// decodes to a default document with blob_version 0, which the caller treats
/// as "no metadata yet".
pub fn decode_meta(meta: &MetaBlob) -> Result<DocumentMetadataBlob, BlobError> {
    if meta.is_empty() {
        return Ok(DocumentMetadataBlob::default());
    }
    let doc: DocumentMetadataBlob = serde_json::from_slice(meta.as_bytes())
        .map_err(|e| BlobError(format!("DocumentMetadataBlob: {e}")))?;
    if doc.blob_version > BLOB_VERSION {
        return Err(BlobError(format!(
            "unsupported meta blob_version {} (max {})",
            doc.blob_version, BLOB_VERSION
        )));
    }
    Ok(doc)
}

/// A blob (de)serialization failure, convertible into the various trait errors.
#[derive(Debug, Clone)]
pub struct BlobError(pub String);

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<BlobError> for ReadError {
    fn from(e: BlobError) -> Self {
        ReadError::MalformedBlob(e.0)
    }
}

impl From<BlobError> for PrintError {
    fn from(e: BlobError) -> Self {
        PrintError::MalformedBlob(e.0)
    }
}

impl From<BlobError> for DiffError {
    fn from(e: BlobError) -> Self {
        DiffError::MalformedBlob(e.0)
    }
}
