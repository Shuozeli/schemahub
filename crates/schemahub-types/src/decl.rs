use bytes::Bytes;

/// The kind of a top-level declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    // Protobuf
    Message,
    Enum,
    Service,
    // FlatBuffers
    Table,
    Struct,
    FbsEnum,
    Union,
    // OpenAPI
    PathItem,
    ComponentSchema,
    ComponentParameter,
    ComponentResponse,
    ComponentRequestBody,
    DocumentMetadata,
}

/// A lightweight summary of one top-level declaration.
/// Returned by `Compiler::summarize_decl` and the ListDeclarations RPC.
#[derive(Clone, Debug)]
pub struct DeclSummary {
    /// The declaration's name as it appears in the schema tree key.
    /// e.g. "UserRequest", "path:/users", "schema:User"
    pub name: String,
    pub kind: DeclKind,
    /// First line of the leading doc comment; empty if none.
    pub doc_comment: String,
}

/// Full detail for one named declaration, as format-specific bytes.
/// The core forwards this to the client without interpretation.
/// The client-side library deserializes it for display.
#[derive(Clone, Debug)]
pub struct DeclDetail(pub Bytes);

impl DeclDetail {
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self(data.into())
    }

    pub fn as_bytes(&self) -> &Bytes {
        &self.0
    }
}

/// A type name a declaration references (for FollowType / rename propagation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeRef {
    pub name: String,
}

impl TypeRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}
