//! The result of diffing two declaration blobs (design.md §2).

use bytes::Bytes;

/// A semantic change between two versions of a declaration.
///
/// The VCS layer uses declaration names for merge/conflict detection; the
/// `detail` bytes are forwarded to clients opaquely for display.
#[derive(Clone, Debug)]
pub enum DeclChange {
    DeclarationAdded {
        name: String,
    },
    DeclarationRemoved {
        name: String,
    },
    /// A declaration present in both old and new blobs with differing content.
    DeclarationModified {
        name: String,
        /// Format-specific change detail. Opaque to the VCS layer.
        detail: Bytes,
    },
}

impl DeclChange {
    pub fn declaration_name(&self) -> &str {
        match self {
            Self::DeclarationAdded { name } => name,
            Self::DeclarationRemoved { name } => name,
            Self::DeclarationModified { name, .. } => name,
        }
    }
}
