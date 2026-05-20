use prost::Message;
use schemahub_types::errors::{MutationError, PrintError, ReadError};

// ── FieldDef ──────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct FieldDef {
    #[prost(string, tag = "1")]
    pub name: String,
    /// e.g. "int32", "string", "[Order]", "PaymentStatus"
    #[prost(string, tag = "2")]
    pub field_type: String,
    #[prost(string, tag = "3")]
    pub default_value: String,
    #[prost(bool, tag = "4")]
    pub deprecated: bool,
    /// Wire identity; cannot change once assigned.
    #[prost(uint32, tag = "5")]
    pub slot_index: u32,
    #[prost(string, tag = "6")]
    pub doc_comment: String,
}

// ── TableBlob ─────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct TableBlob {
    #[prost(string, tag = "1")]
    pub name: String,
    /// Ordered by slot_index.
    #[prost(message, repeated, tag = "2")]
    pub fields: Vec<FieldDef>,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

// ── StructBlob ────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct StructFieldDef {
    #[prost(string, tag = "1")]
    pub name: String,
    /// Only scalars allowed in structs.
    #[prost(string, tag = "2")]
    pub field_type: String,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct StructBlob {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, repeated, tag = "2")]
    pub fields: Vec<StructFieldDef>,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

// ── EnumBlob ──────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnumValueDef {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(int64, tag = "2")]
    pub value: i64,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EnumBlob {
    #[prost(string, tag = "1")]
    pub name: String,
    /// "int8", "int16", "int32", "int64", "uint8", ...
    #[prost(string, tag = "2")]
    pub base_type: String,
    #[prost(message, repeated, tag = "3")]
    pub values: Vec<EnumValueDef>,
    #[prost(string, tag = "4")]
    pub doc_comment: String,
}

// ── UnionBlob ─────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct UnionBlob {
    #[prost(string, tag = "1")]
    pub name: String,
    /// Table names.
    #[prost(string, repeated, tag = "2")]
    pub members: Vec<String>,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

// ── FileMetadataBlob ──────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct FileMetadataBlob {
    #[prost(string, tag = "1")]
    pub namespace: String,
    #[prost(string, repeated, tag = "2")]
    pub imports: Vec<String>,
    #[prost(string, tag = "3")]
    pub root_type: String,
    #[prost(string, repeated, tag = "4")]
    pub file_identifier: Vec<String>,
}

// ── ParseEnvelope ─────────────────────────────────────────────────────────────

/// The envelope blob returned by parse(). The core unwraps this into individual blobs.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ParseEnvelope {
    #[prost(message, repeated, tag = "1")]
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

// ── DeclBlob wrapper ──────────────────────────────────────────────────────────

pub const KIND_TABLE: i32 = 0;
pub const KIND_STRUCT: i32 = 1;
pub const KIND_ENUM: i32 = 2;
pub const KIND_UNION: i32 = 3;
pub const KIND_METADATA: i32 = 4;

/// Wrapper stored in the schema tree for each declaration.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DeclBlob {
    /// 0=table, 1=struct, 2=enum, 3=union, 4=metadata
    #[prost(int32, tag = "1")]
    pub kind: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub data: Vec<u8>,
}

// ── Encode / decode helpers ───────────────────────────────────────────────────

pub fn encode_envelope(env: &ParseEnvelope) -> Vec<u8> {
    env.encode_to_vec()
}

pub fn decode_envelope(data: &[u8]) -> Result<ParseEnvelope, ReadError> {
    ParseEnvelope::decode(data)
        .map_err(|e| ReadError::MalformedBlob(format!("ParseEnvelope: {e}")))
}

pub fn decode_envelope_mutation(data: &[u8]) -> Result<ParseEnvelope, MutationError> {
    ParseEnvelope::decode(data)
        .map_err(|e| MutationError::MalformedBlob(format!("ParseEnvelope: {e}")))
}

pub fn decode_envelope_print(data: &[u8]) -> Result<ParseEnvelope, PrintError> {
    ParseEnvelope::decode(data)
        .map_err(|e| PrintError::MalformedBlob(format!("ParseEnvelope: {e}")))
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

pub fn decode_decl_blob_print(data: &[u8]) -> Result<DeclBlob, PrintError> {
    DeclBlob::decode(data)
        .map_err(|e| PrintError::MalformedBlob(format!("DeclBlob: {e}")))
}

// ── Wrap helpers ──────────────────────────────────────────────────────────────

pub fn wrap_table(t: &TableBlob) -> DeclBlob {
    DeclBlob { kind: KIND_TABLE, data: t.encode_to_vec() }
}

pub fn wrap_struct(s: &StructBlob) -> DeclBlob {
    DeclBlob { kind: KIND_STRUCT, data: s.encode_to_vec() }
}

pub fn wrap_enum(e: &EnumBlob) -> DeclBlob {
    DeclBlob { kind: KIND_ENUM, data: e.encode_to_vec() }
}

pub fn wrap_union(u: &UnionBlob) -> DeclBlob {
    DeclBlob { kind: KIND_UNION, data: u.encode_to_vec() }
}

pub fn wrap_metadata(m: &FileMetadataBlob) -> DeclBlob {
    DeclBlob { kind: KIND_METADATA, data: m.encode_to_vec() }
}

// ── Unwrap helpers (MutationError) ───────────────────────────────────────────

pub fn unwrap_table(db: &DeclBlob) -> Result<TableBlob, MutationError> {
    TableBlob::decode(db.data.as_slice())
        .map_err(|e| MutationError::MalformedBlob(format!("TableBlob: {e}")))
}

pub fn unwrap_struct(db: &DeclBlob) -> Result<StructBlob, MutationError> {
    StructBlob::decode(db.data.as_slice())
        .map_err(|e| MutationError::MalformedBlob(format!("StructBlob: {e}")))
}

pub fn unwrap_enum(db: &DeclBlob) -> Result<EnumBlob, MutationError> {
    EnumBlob::decode(db.data.as_slice())
        .map_err(|e| MutationError::MalformedBlob(format!("EnumBlob: {e}")))
}

pub fn unwrap_union(db: &DeclBlob) -> Result<UnionBlob, MutationError> {
    UnionBlob::decode(db.data.as_slice())
        .map_err(|e| MutationError::MalformedBlob(format!("UnionBlob: {e}")))
}

pub fn unwrap_metadata(db: &DeclBlob) -> Result<FileMetadataBlob, MutationError> {
    FileMetadataBlob::decode(db.data.as_slice())
        .map_err(|e| MutationError::MalformedBlob(format!("FileMetadataBlob: {e}")))
}

// ── Unwrap helpers (ReadError) ────────────────────────────────────────────────

pub fn unwrap_table_read(db: &DeclBlob) -> Result<TableBlob, ReadError> {
    TableBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("TableBlob: {e}")))
}

pub fn unwrap_struct_read(db: &DeclBlob) -> Result<StructBlob, ReadError> {
    StructBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("StructBlob: {e}")))
}

pub fn unwrap_enum_read(db: &DeclBlob) -> Result<EnumBlob, ReadError> {
    EnumBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("EnumBlob: {e}")))
}

pub fn unwrap_union_read(db: &DeclBlob) -> Result<UnionBlob, ReadError> {
    UnionBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("UnionBlob: {e}")))
}

pub fn unwrap_metadata_read(db: &DeclBlob) -> Result<FileMetadataBlob, ReadError> {
    FileMetadataBlob::decode(db.data.as_slice())
        .map_err(|e| ReadError::MalformedBlob(format!("FileMetadataBlob: {e}")))
}

// ── Unwrap helpers (PrintError) ───────────────────────────────────────────────

pub fn unwrap_table_print(db: &DeclBlob) -> Result<TableBlob, PrintError> {
    TableBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("TableBlob: {e}")))
}

pub fn unwrap_struct_print(db: &DeclBlob) -> Result<StructBlob, PrintError> {
    StructBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("StructBlob: {e}")))
}

pub fn unwrap_enum_print(db: &DeclBlob) -> Result<EnumBlob, PrintError> {
    EnumBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("EnumBlob: {e}")))
}

pub fn unwrap_union_print(db: &DeclBlob) -> Result<UnionBlob, PrintError> {
    UnionBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("UnionBlob: {e}")))
}

pub fn unwrap_metadata_print(db: &DeclBlob) -> Result<FileMetadataBlob, PrintError> {
    FileMetadataBlob::decode(db.data.as_slice())
        .map_err(|e| PrintError::MalformedBlob(format!("FileMetadataBlob: {e}")))
}
