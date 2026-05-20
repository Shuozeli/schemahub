use prost::Message;
use schemahub_types::errors::MutationError;

// ── Operation types ───────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddField {
    #[prost(string, tag = "1")]
    pub field_name: String,
    #[prost(string, tag = "2")]
    pub field_type: String,
    #[prost(uint32, tag = "3")]
    pub field_number: u32,
    #[prost(bool, tag = "4")]
    pub repeated: bool,
    #[prost(string, tag = "5")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveField {
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
pub struct OpChangeFieldType {
    #[prost(string, tag = "1")]
    pub field_name: String,
    #[prost(string, tag = "2")]
    pub new_type: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpChangeFieldLabel {
    #[prost(string, tag = "1")]
    pub field_name: String,
    /// "optional" or "repeated"
    #[prost(string, tag = "2")]
    pub new_label: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpReorderFields {
    /// All field names in new order
    #[prost(string, repeated, tag = "1")]
    pub field_order: Vec<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddMessage {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, tag = "2")]
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveMessage {
    #[prost(string, tag = "1")]
    pub message_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRenameMessage {
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
    pub doc_comment: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddEnumValue {
    #[prost(string, tag = "1")]
    pub enum_name: String,
    #[prost(string, tag = "2")]
    pub value_name: String,
    #[prost(int32, tag = "3")]
    pub number: i32,
    #[prost(string, tag = "4")]
    pub doc_comment: String,
}

// ── Top-level discriminator ───────────────────────────────────────────────────

/// Tag values for the top-level ProtoOperation oneof.
pub mod op_tag {
    pub const ADD_FIELD: u32 = 1;
    pub const REMOVE_FIELD: u32 = 2;
    pub const RENAME_FIELD: u32 = 3;
    pub const CHANGE_FIELD_TYPE: u32 = 4;
    pub const CHANGE_FIELD_LABEL: u32 = 5;
    pub const REORDER_FIELDS: u32 = 6;
    pub const ADD_MESSAGE: u32 = 10;
    pub const REMOVE_MESSAGE: u32 = 11;
    pub const RENAME_MESSAGE: u32 = 12;
    pub const ADD_ENUM: u32 = 20;
    pub const ADD_ENUM_VALUE: u32 = 21;
}

/// The decoded operation, represented as an enum rather than a prost oneof
/// (prost oneofs require generated code; we decode manually instead).
#[derive(Clone, Debug)]
pub enum ProtoOp {
    AddField(OpAddField),
    RemoveField(OpRemoveField),
    RenameField(OpRenameField),
    ChangeFieldType(OpChangeFieldType),
    ChangeFieldLabel(OpChangeFieldLabel),
    ReorderFields(OpReorderFields),
    AddMessage(OpAddMessage),
    RemoveMessage(OpRemoveMessage),
    RenameMessage(OpRenameMessage),
    AddEnum(OpAddEnum),
    AddEnumValue(OpAddEnumValue),
}

/// A thin envelope that carries the tag and the raw inner bytes.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ProtoOperationEnvelope {
    /// Which operation this is (see op_tag constants).
    #[prost(uint32, tag = "1")]
    pub tag: u32,
    /// Encoded inner operation struct.
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

impl ProtoOperationEnvelope {
    pub fn encode_op(tag: u32, payload: Vec<u8>) -> Vec<u8> {
        ProtoOperationEnvelope { tag, payload }.encode_to_vec()
    }
}

/// Decode raw mutation.operation bytes into a ProtoOp.
pub fn decode_operation(bytes: &[u8]) -> Result<ProtoOp, MutationError> {
    let env = ProtoOperationEnvelope::decode(bytes)
        .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;

    let op = match env.tag {
        op_tag::ADD_FIELD => {
            let inner = OpAddField::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::AddField(inner)
        }
        op_tag::REMOVE_FIELD => {
            let inner = OpRemoveField::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::RemoveField(inner)
        }
        op_tag::RENAME_FIELD => {
            let inner = OpRenameField::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::RenameField(inner)
        }
        op_tag::CHANGE_FIELD_TYPE => {
            let inner = OpChangeFieldType::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::ChangeFieldType(inner)
        }
        op_tag::CHANGE_FIELD_LABEL => {
            let inner = OpChangeFieldLabel::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::ChangeFieldLabel(inner)
        }
        op_tag::REORDER_FIELDS => {
            let inner = OpReorderFields::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::ReorderFields(inner)
        }
        op_tag::ADD_MESSAGE => {
            let inner = OpAddMessage::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::AddMessage(inner)
        }
        op_tag::REMOVE_MESSAGE => {
            let inner = OpRemoveMessage::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::RemoveMessage(inner)
        }
        op_tag::RENAME_MESSAGE => {
            let inner = OpRenameMessage::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::RenameMessage(inner)
        }
        op_tag::ADD_ENUM => {
            let inner = OpAddEnum::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::AddEnum(inner)
        }
        op_tag::ADD_ENUM_VALUE => {
            let inner = OpAddEnumValue::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            ProtoOp::AddEnumValue(inner)
        }
        other => {
            return Err(MutationError::InvalidOperationBytes(
                format!("unknown operation tag {other}"),
            ));
        }
    };

    Ok(op)
}
