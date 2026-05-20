use crate::Hash;

/// An opaque serialized AST blob. The core stores and retrieves these without
/// interpreting their contents. Only the FormatPlugin that produced a blob can
/// deserialize it.
#[derive(Clone, PartialEq, Eq)]
pub struct Blob(pub Vec<u8>);

impl Blob {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn hash(&self) -> Hash {
        Hash::of(&self.0)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for Blob {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blob({} bytes, hash={})", self.0.len(), &self.hash().to_hex()[..12])
    }
}
