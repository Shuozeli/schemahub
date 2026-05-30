//! Opaque per-declaration and per-file blobs (design.md §2).
//!
//! A [`DeclBlob`] is the serialized AST of one top-level declaration; a
//! [`MetaBlob`] is the file-level metadata (package, imports, syntax/edition).
//! Both are opaque `Vec<u8>` to the VCS layer — only the `Compiler` that
//! produced a blob can deserialize it.

/// The serialized AST of one top-level declaration.
#[derive(Clone, PartialEq, Eq)]
pub struct DeclBlob(pub Vec<u8>);

impl DeclBlob {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for DeclBlob {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<&[u8]> for DeclBlob {
    fn from(v: &[u8]) -> Self {
        Self(v.to_vec())
    }
}

impl std::fmt::Debug for DeclBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeclBlob({} bytes)", self.0.len())
    }
}

/// File-level metadata: package, imports, syntax/edition, file options.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct MetaBlob(pub Vec<u8>);

impl MetaBlob {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for MetaBlob {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<&[u8]> for MetaBlob {
    fn from(v: &[u8]) -> Self {
        Self(v.to_vec())
    }
}

impl std::fmt::Debug for MetaBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MetaBlob({} bytes)", self.0.len())
    }
}
