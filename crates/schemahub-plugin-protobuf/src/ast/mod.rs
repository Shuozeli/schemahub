use prost::Message;
use schemahub_types::errors::{MutationError, PrintError, ReadError};

// ── MessageBlob ──────────────────────────────────────────────────────────────

/// One blob per message definition. Stored in schema tree as "MessageName" → blob.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessageBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "3")]
    pub fields: Vec<FieldDef>,
    #[prost(message, repeated, tag = "4")]
    pub reserved_numbers: Vec<ReservedRange>,
    #[prost(string, repeated, tag = "5")]
    pub reserved_names: Vec<String>,
    #[prost(string, tag = "6")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct FieldDef {
    #[prost(string, tag = "1")]
    pub name: String,
    /// e.g. "string", "int32", "payments.User"
    #[prost(string, tag = "2")]
    pub field_type: String,
    #[prost(uint32, tag = "3")]
    pub number: u32,
    #[prost(bool, tag = "4")]
    pub repeated: bool,
    #[prost(string, tag = "5")]
    pub doc_comment: String,
}

/// Inclusive range for reserved field numbers. For single "reserved 1;" start == end.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ReservedRange {
    #[prost(uint32, tag = "1")]
    pub start: u32,
    #[prost(uint32, tag = "2")]
    pub end: u32,
}

// ── EnumBlob ─────────────────────────────────────────────────────────────────

/// One blob per enum definition.
#[derive(Clone, PartialEq, prost::Message)]
pub struct EnumBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "3")]
    pub values: Vec<EnumValueDef>,
    #[prost(string, tag = "4")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnumValueDef {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(int32, tag = "2")]
    pub number: i32,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

// ── ServiceBlob ──────────────────────────────────────────────────────────────

/// One blob per service definition.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ServiceBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "3")]
    pub rpcs: Vec<RpcDef>,
    #[prost(string, tag = "4")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct RpcDef {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub request_type: String,
    #[prost(string, tag = "3")]
    pub response_type: String,
    #[prost(bool, tag = "4")]
    pub client_streaming: bool,
    #[prost(bool, tag = "5")]
    pub server_streaming: bool,
    #[prost(string, tag = "6")]
    pub doc_comment: String,
}

// ── FileMetadataBlob ─────────────────────────────────────────────────────────

/// File-level metadata (package, imports, syntax version).
/// Stored under the reserved key "__metadata__" in the schema tree.
#[derive(Clone, PartialEq, prost::Message)]
pub struct FileMetadataBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub syntax: String,
    #[prost(string, tag = "3")]
    pub package: String,
    #[prost(message, repeated, tag = "4")]
    pub imports: Vec<ImportDef>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ImportDef {
    /// Logical schemahub path: "project/repo/schema.proto"
    #[prost(string, tag = "1")]
    pub path: String,
    /// Pinned commit; empty on initial CreateSchema
    #[prost(string, tag = "2")]
    pub resolved_commit: String,
}

// ── ParseEnvelope ─────────────────────────────────────────────────────────────

/// The envelope blob returned by parse(). The core unwraps this into individual blobs.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ParseEnvelope {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(message, repeated, tag = "2")]
    pub declarations: Vec<ParsedDecl>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ParsedDecl {
    /// Schema tree key: declaration name, or "__metadata__"
    #[prost(string, tag = "1")]
    pub tree_key: String,
    /// Encoded DeclBlob bytes
    #[prost(bytes = "vec", tag = "2")]
    pub blob_bytes: Vec<u8>,
}

// ── DeclBlob wrapper ─────────────────────────────────────────────────────────

pub const KIND_MESSAGE: i32 = 0;
pub const KIND_ENUM: i32 = 1;
pub const KIND_SERVICE: i32 = 2;
pub const KIND_METADATA: i32 = 3;

/// Wrapper stored in the schema tree for each declaration.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DeclBlob {
    /// 0=message, 1=enum, 2=service, 3=metadata
    #[prost(int32, tag = "1")]
    pub kind: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub data: Vec<u8>,
}

// ── Encode / decode helpers ───────────────────────────────────────────────────

pub fn encode_message(msg: &MessageBlob) -> Vec<u8> {
    msg.encode_to_vec()
}

pub fn decode_message(data: &[u8]) -> Result<MessageBlob, MutationError> {
    MessageBlob::decode(data)
        .map_err(|e| MutationError::MalformedBlob(format!("MessageBlob: {e}")))
}

pub fn encode_enum(e: &EnumBlob) -> Vec<u8> {
    e.encode_to_vec()
}

pub fn decode_enum(data: &[u8]) -> Result<EnumBlob, MutationError> {
    EnumBlob::decode(data)
        .map_err(|e| MutationError::MalformedBlob(format!("EnumBlob: {e}")))
}

pub fn encode_service(s: &ServiceBlob) -> Vec<u8> {
    s.encode_to_vec()
}

pub fn decode_service(data: &[u8]) -> Result<ServiceBlob, MutationError> {
    ServiceBlob::decode(data)
        .map_err(|e| MutationError::MalformedBlob(format!("ServiceBlob: {e}")))
}

pub fn encode_metadata(m: &FileMetadataBlob) -> Vec<u8> {
    m.encode_to_vec()
}

pub fn decode_metadata(data: &[u8]) -> Result<FileMetadataBlob, MutationError> {
    FileMetadataBlob::decode(data)
        .map_err(|e| MutationError::MalformedBlob(format!("FileMetadataBlob: {e}")))
}

pub fn encode_envelope(env: &ParseEnvelope) -> Vec<u8> {
    env.encode_to_vec()
}

pub fn decode_envelope(data: &[u8]) -> Result<ParseEnvelope, ReadError> {
    ParseEnvelope::decode(data)
        .map_err(|e| ReadError::MalformedBlob(format!("ParseEnvelope: {e}")))
}

pub fn encode_decl_blob(db: &DeclBlob) -> Vec<u8> {
    db.encode_to_vec()
}

pub fn decode_decl_blob(data: &[u8]) -> Result<DeclBlob, MutationError> {
    DeclBlob::decode(data)
        .map_err(|e| MutationError::MalformedBlob(format!("DeclBlob: {e}")))
}

pub fn decode_decl_blob_read(data: &[u8]) -> Result<DeclBlob, ReadError> {
    DeclBlob::decode(data)
        .map_err(|e| ReadError::MalformedBlob(format!("DeclBlob: {e}")))
}

pub fn decode_envelope_mutation(data: &[u8]) -> Result<ParseEnvelope, MutationError> {
    ParseEnvelope::decode(data)
        .map_err(|e| MutationError::MalformedBlob(format!("ParseEnvelope: {e}")))
}

/// Wrap a MessageBlob as a DeclBlob.
pub fn wrap_message(msg: &MessageBlob) -> DeclBlob {
    DeclBlob {
        kind: KIND_MESSAGE,
        data: encode_message(msg),
    }
}

/// Wrap an EnumBlob as a DeclBlob.
pub fn wrap_enum(e: &EnumBlob) -> DeclBlob {
    DeclBlob {
        kind: KIND_ENUM,
        data: encode_enum(e),
    }
}

/// Wrap a ServiceBlob as a DeclBlob.
pub fn wrap_service(s: &ServiceBlob) -> DeclBlob {
    DeclBlob {
        kind: KIND_SERVICE,
        data: encode_service(s),
    }
}

/// Wrap a FileMetadataBlob as a DeclBlob.
pub fn wrap_metadata(m: &FileMetadataBlob) -> DeclBlob {
    DeclBlob {
        kind: KIND_METADATA,
        data: encode_metadata(m),
    }
}

/// Decode a DeclBlob's inner data as a MessageBlob.
pub fn unwrap_message(db: &DeclBlob) -> Result<MessageBlob, MutationError> {
    decode_message(&db.data)
}

/// Decode a DeclBlob's inner data as an EnumBlob.
pub fn unwrap_enum(db: &DeclBlob) -> Result<EnumBlob, MutationError> {
    decode_enum(&db.data)
}

/// Decode a DeclBlob's inner data as a ServiceBlob.
pub fn unwrap_service(db: &DeclBlob) -> Result<ServiceBlob, MutationError> {
    decode_service(&db.data)
}

/// Decode a DeclBlob's inner data as a FileMetadataBlob.
pub fn unwrap_metadata(db: &DeclBlob) -> Result<FileMetadataBlob, MutationError> {
    decode_metadata(&db.data)
}

/// Same as unwrap_message but returns ReadError.
pub fn unwrap_message_read(db: &DeclBlob) -> Result<MessageBlob, ReadError> {
    MessageBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("MessageBlob: {e}")))
}

pub fn unwrap_enum_read(db: &DeclBlob) -> Result<EnumBlob, ReadError> {
    EnumBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("EnumBlob: {e}")))
}

pub fn unwrap_service_read(db: &DeclBlob) -> Result<ServiceBlob, ReadError> {
    ServiceBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("ServiceBlob: {e}")))
}

pub fn unwrap_metadata_read(db: &DeclBlob) -> Result<FileMetadataBlob, ReadError> {
    FileMetadataBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("FileMetadataBlob: {e}")))
}

/// Print-error variants of decoding.
pub fn decode_decl_blob_print(data: &[u8]) -> Result<DeclBlob, PrintError> {
    DeclBlob::decode(data)
        .map_err(|e| PrintError::MalformedBlob(format!("DeclBlob: {e}")))
}

pub fn decode_envelope_print(data: &[u8]) -> Result<ParseEnvelope, PrintError> {
    ParseEnvelope::decode(data)
        .map_err(|e| PrintError::MalformedBlob(format!("ParseEnvelope: {e}")))
}

pub fn unwrap_message_print(db: &DeclBlob) -> Result<MessageBlob, PrintError> {
    MessageBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("MessageBlob: {e}")))
}

pub fn unwrap_enum_print(db: &DeclBlob) -> Result<EnumBlob, PrintError> {
    EnumBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("EnumBlob: {e}")))
}

pub fn unwrap_service_print(db: &DeclBlob) -> Result<ServiceBlob, PrintError> {
    ServiceBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("ServiceBlob: {e}")))
}

pub fn unwrap_metadata_print(db: &DeclBlob) -> Result<FileMetadataBlob, PrintError> {
    FileMetadataBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("FileMetadataBlob: {e}")))
}
