use prost::Message;
use schemahub_types::errors::MutationError;

// ── Operation types ───────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddField {
    #[prost(string, tag = "1")]
    pub field_name: String,
    #[prost(string, tag = "2")]
    pub field_type: String,
    #[prost(string, tag = "3")]
    pub default_value: String,
    #[prost(string, tag = "4")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpDeprecateField {
    #[prost(string, tag = "1")]
    pub field_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRenameField {
    #[prost(string, tag = "1")]
    pub old_field_name: String,
    #[prost(string, tag = "2")]
    pub new_field_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddTable {
    #[prost(string, tag = "1")]
    pub table_name: String,
    #[prost(string, tag = "2")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveTable {
    #[prost(string, tag = "1")]
    pub table_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRenameTable {
    #[prost(string, tag = "1")]
    pub old_name: String,
    #[prost(string, tag = "2")]
    pub new_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddEnum {
    #[prost(string, tag = "1")]
    pub enum_name: String,
    #[prost(string, tag = "2")]
    pub base_type: String,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddEnumValue {
    #[prost(string, tag = "1")]
    pub enum_name: String,
    #[prost(string, tag = "2")]
    pub value_name: String,
    #[prost(int64, tag = "3")]
    pub value: i64,
    #[prost(string, tag = "4")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddUnion {
    #[prost(string, tag = "1")]
    pub union_name: String,
    #[prost(string, repeated, tag = "2")]
    pub members: Vec<String>,
    #[prost(string, tag = "3")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpUpdateImport {
    /// New import path to add; empty string removes all imports (or use remove_import).
    #[prost(string, tag = "1")]
    pub import_path: String,
    #[prost(bool, tag = "2")]
    pub remove: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddUnionMember {
    #[prost(string, tag = "1")]
    pub union_name: String,
    #[prost(string, tag = "2")]
    pub member_type: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveUnionMember {
    #[prost(string, tag = "1")]
    pub union_name: String,
    #[prost(string, tag = "2")]
    pub member_type: String,
}

// ── Tag constants ─────────────────────────────────────────────────────────────

pub mod op_tag {
    pub const ADD_FIELD: u32 = 1;
    pub const DEPRECATE_FIELD: u32 = 2;
    pub const RENAME_FIELD: u32 = 3;
    pub const ADD_TABLE: u32 = 10;
    pub const REMOVE_TABLE: u32 = 11;
    pub const RENAME_TABLE: u32 = 12;
    pub const ADD_ENUM: u32 = 20;
    pub const ADD_ENUM_VALUE: u32 = 21;
    pub const ADD_UNION: u32 = 30;
    pub const ADD_UNION_MEMBER: u32 = 31;
    pub const REMOVE_UNION_MEMBER: u32 = 32;
    pub const UPDATE_IMPORT: u32 = 40;
}

/// The decoded FlatBuffers operation.
#[derive(Clone, Debug)]
pub enum FbsOp {
    AddField(OpAddField),
    DeprecateField(OpDeprecateField),
    RenameField(OpRenameField),
    AddTable(OpAddTable),
    RemoveTable(OpRemoveTable),
    RenameTable(OpRenameTable),
    AddEnum(OpAddEnum),
    AddEnumValue(OpAddEnumValue),
    AddUnion(OpAddUnion),
    AddUnionMember(OpAddUnionMember),
    RemoveUnionMember(OpRemoveUnionMember),
    UpdateImport(OpUpdateImport),
}

/// Thin envelope carrying the tag and raw inner bytes.
#[derive(Clone, PartialEq, prost::Message)]
pub struct FbsOperationEnvelope {
    #[prost(uint32, tag = "1")]
    pub tag: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

impl FbsOperationEnvelope {
    pub fn encode_op(tag: u32, payload: Vec<u8>) -> Vec<u8> {
        FbsOperationEnvelope { tag, payload }.encode_to_vec()
    }
}

/// Decode raw mutation.operation bytes into an FbsOp.
pub fn decode_operation(bytes: &[u8]) -> Result<FbsOp, MutationError> {
    let env = FbsOperationEnvelope::decode(bytes)
        .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;

    let op = match env.tag {
        op_tag::ADD_FIELD => {
            let inner = OpAddField::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::AddField(inner)
        }
        op_tag::DEPRECATE_FIELD => {
            let inner = OpDeprecateField::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::DeprecateField(inner)
        }
        op_tag::RENAME_FIELD => {
            let inner = OpRenameField::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::RenameField(inner)
        }
        op_tag::ADD_TABLE => {
            let inner = OpAddTable::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::AddTable(inner)
        }
        op_tag::REMOVE_TABLE => {
            let inner = OpRemoveTable::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::RemoveTable(inner)
        }
        op_tag::RENAME_TABLE => {
            let inner = OpRenameTable::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::RenameTable(inner)
        }
        op_tag::ADD_ENUM => {
            let inner = OpAddEnum::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::AddEnum(inner)
        }
        op_tag::ADD_ENUM_VALUE => {
            let inner = OpAddEnumValue::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::AddEnumValue(inner)
        }
        op_tag::ADD_UNION => {
            let inner = OpAddUnion::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::AddUnion(inner)
        }
        op_tag::ADD_UNION_MEMBER => {
            let inner = OpAddUnionMember::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::AddUnionMember(inner)
        }
        op_tag::REMOVE_UNION_MEMBER => {
            let inner = OpRemoveUnionMember::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::RemoveUnionMember(inner)
        }
        op_tag::UPDATE_IMPORT => {
            let inner = OpUpdateImport::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            FbsOp::UpdateImport(inner)
        }
        other => {
            return Err(MutationError::InvalidOperationBytes(
                format!("unknown FBS operation tag {other}"),
            ));
        }
    };

    Ok(op)
}
