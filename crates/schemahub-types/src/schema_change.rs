use bytes::Bytes;

/// A semantic change between two versions of a declaration.
/// The core uses declaration names for merge conflict detection.
/// The `detail` bytes are forwarded to clients opaquely for display.
#[derive(Clone, Debug)]
pub enum SchemaChange {
    DeclarationAdded {
        name: String,
    },
    DeclarationRemoved {
        name: String,
    },
    /// A declaration present in both old and new blobs with differing content.
    DeclarationModified {
        name: String,
        /// Format-specific change detail. Opaque to the core.
        detail: Bytes,
    },
}

impl SchemaChange {
    pub fn declaration_name(&self) -> &str {
        match self {
            Self::DeclarationAdded { name } => name,
            Self::DeclarationRemoved { name } => name,
            Self::DeclarationModified { name, .. } => name,
        }
    }
}
