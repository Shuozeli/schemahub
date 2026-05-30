//! The typed protobuf-mutation op envelope (design.md §3.1 mutation validator).
//!
//! The VCS layer passes `Mutation.operation` as opaque `Bytes`. This crate owns
//! the decoding: a `ProtoOp` enum whose variants carry the parameters for each
//! granular edit. We use `prost` to (de)serialize a wire-compatible envelope so
//! the server can construct these from generated gRPC types.
//!
//! Encoding: a `ProtoOperation` prost message with a `oneof op` — but `prost`'s
//! derive needs a concrete oneof enum, so we model it as a tagged envelope:
//! field 1 = `op_kind` (u32 tag), field 2..N = the variant payloads. To keep
//! this dependency-light and explicit, we hand-roll a thin tag dispatch over
//! prost-encoded payload structs.

use prost::Message;
use schemahub_types::MutationError;

// ── Per-op payload structs (prost messages) ────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddField {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, tag = "2")]
    pub field_name: String,
    /// Scalar type name (e.g. "int32") or a message/enum type name.
    #[prost(string, tag = "3")]
    pub field_type: String,
    #[prost(uint32, tag = "4")]
    pub field_number: u32,
    /// "optional" | "required" | "repeated" | "" (singular).
    #[prost(string, tag = "5")]
    pub cardinality: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveField {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, tag = "2")]
    pub field_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRenameField {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, tag = "2")]
    pub old_field_name: String,
    #[prost(string, tag = "3")]
    pub new_field_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpChangeFieldType {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, tag = "2")]
    pub field_name: String,
    #[prost(string, tag = "3")]
    pub new_type: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpChangeCardinality {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, tag = "2")]
    pub field_name: String,
    /// "optional" | "required" | "repeated" | "singular".
    #[prost(string, tag = "3")]
    pub new_cardinality: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpReorderFields {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, repeated, tag = "2")]
    pub field_order: Vec<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpChangeFieldNumber {
    #[prost(string, tag = "1")]
    pub message_name: String,
    #[prost(string, tag = "2")]
    pub field_name: String,
    #[prost(int32, tag = "3")]
    pub new_number: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpCreateMessage {
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
pub struct OpDeleteMessage {
    #[prost(string, tag = "1")]
    pub message_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpCreateEnum {
    #[prost(string, tag = "1")]
    pub enum_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpDeleteEnum {
    #[prost(string, tag = "1")]
    pub enum_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddEnumValue {
    #[prost(string, tag = "1")]
    pub enum_name: String,
    #[prost(string, tag = "2")]
    pub value_name: String,
    #[prost(int32, tag = "3")]
    pub number: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveEnumValue {
    #[prost(string, tag = "1")]
    pub enum_name: String,
    #[prost(string, tag = "2")]
    pub value_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRenameEnumValue {
    #[prost(string, tag = "1")]
    pub enum_name: String,
    #[prost(string, tag = "2")]
    pub old_value_name: String,
    #[prost(string, tag = "3")]
    pub new_value_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddService {
    #[prost(string, tag = "1")]
    pub service_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveService {
    #[prost(string, tag = "1")]
    pub service_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRenameService {
    #[prost(string, tag = "1")]
    pub old_name: String,
    #[prost(string, tag = "2")]
    pub new_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddRpc {
    #[prost(string, tag = "1")]
    pub service_name: String,
    #[prost(string, tag = "2")]
    pub rpc_name: String,
    #[prost(string, tag = "3")]
    pub request_type: String,
    #[prost(string, tag = "4")]
    pub response_type: String,
    #[prost(bool, tag = "5")]
    pub client_streaming: bool,
    #[prost(bool, tag = "6")]
    pub server_streaming: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveRpc {
    #[prost(string, tag = "1")]
    pub service_name: String,
    #[prost(string, tag = "2")]
    pub rpc_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRenameRpc {
    #[prost(string, tag = "1")]
    pub service_name: String,
    #[prost(string, tag = "2")]
    pub old_rpc_name: String,
    #[prost(string, tag = "3")]
    pub new_rpc_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpChangeRpcType {
    #[prost(string, tag = "1")]
    pub service_name: String,
    #[prost(string, tag = "2")]
    pub rpc_name: String,
    #[prost(string, tag = "3")]
    pub new_request_type: String,
    #[prost(string, tag = "4")]
    pub new_response_type: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpUpdateImport {
    /// Logical import path "project/repo/schema.proto".
    #[prost(string, tag = "1")]
    pub import_path: String,
    #[prost(string, tag = "2")]
    pub resolved_commit: String,
    /// add = false → add/update; true → remove.
    #[prost(bool, tag = "3")]
    pub remove: bool,
}

// ── Tag-dispatched envelope ─────────────────────────────────────────────────

/// Wire envelope: `tag` selects the variant; `payload` is the prost-encoded
/// per-op struct. The server constructs `{ tag, payload }` and the compiler
/// decodes back into [`ProtoOp`].
#[derive(Clone, PartialEq, prost::Message)]
pub struct ProtoOperation {
    #[prost(uint32, tag = "1")]
    pub tag: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

pub mod tag {
    pub const ADD_FIELD: u32 = 1;
    pub const REMOVE_FIELD: u32 = 2;
    pub const RENAME_FIELD: u32 = 3;
    pub const CHANGE_FIELD_TYPE: u32 = 4;
    pub const CHANGE_CARDINALITY: u32 = 5;
    pub const REORDER_FIELDS: u32 = 6;
    pub const CHANGE_FIELD_NUMBER: u32 = 7;
    pub const CREATE_MESSAGE: u32 = 10;
    pub const RENAME_MESSAGE: u32 = 11;
    pub const DELETE_MESSAGE: u32 = 12;
    pub const CREATE_ENUM: u32 = 20;
    pub const DELETE_ENUM: u32 = 21;
    pub const ADD_ENUM_VALUE: u32 = 22;
    pub const REMOVE_ENUM_VALUE: u32 = 23;
    pub const RENAME_ENUM_VALUE: u32 = 24;
    pub const ADD_SERVICE: u32 = 30;
    pub const REMOVE_SERVICE: u32 = 31;
    pub const RENAME_SERVICE: u32 = 32;
    pub const ADD_RPC: u32 = 33;
    pub const REMOVE_RPC: u32 = 34;
    pub const RENAME_RPC: u32 = 35;
    pub const CHANGE_RPC_TYPE: u32 = 36;
    pub const UPDATE_IMPORT: u32 = 40;
}

/// The decoded mutation operation.
#[derive(Clone, Debug, PartialEq)]
pub enum ProtoOp {
    AddField(OpAddField),
    RemoveField(OpRemoveField),
    RenameField(OpRenameField),
    ChangeFieldType(OpChangeFieldType),
    ChangeCardinality(OpChangeCardinality),
    ReorderFields(OpReorderFields),
    ChangeFieldNumber(OpChangeFieldNumber),
    CreateMessage(OpCreateMessage),
    RenameMessage(OpRenameMessage),
    DeleteMessage(OpDeleteMessage),
    CreateEnum(OpCreateEnum),
    DeleteEnum(OpDeleteEnum),
    AddEnumValue(OpAddEnumValue),
    RemoveEnumValue(OpRemoveEnumValue),
    RenameEnumValue(OpRenameEnumValue),
    AddService(OpAddService),
    RemoveService(OpRemoveService),
    RenameService(OpRenameService),
    AddRpc(OpAddRpc),
    RemoveRpc(OpRemoveRpc),
    RenameRpc(OpRenameRpc),
    ChangeRpcType(OpChangeRpcType),
    UpdateImport(OpUpdateImport),
}

impl ProtoOp {
    /// Encode this op to the opaque envelope bytes the VCS layer carries.
    pub fn encode(&self) -> Vec<u8> {
        let (tag, payload) = match self {
            ProtoOp::AddField(o) => (tag::ADD_FIELD, o.encode_to_vec()),
            ProtoOp::RemoveField(o) => (tag::REMOVE_FIELD, o.encode_to_vec()),
            ProtoOp::RenameField(o) => (tag::RENAME_FIELD, o.encode_to_vec()),
            ProtoOp::ChangeFieldType(o) => (tag::CHANGE_FIELD_TYPE, o.encode_to_vec()),
            ProtoOp::ChangeCardinality(o) => (tag::CHANGE_CARDINALITY, o.encode_to_vec()),
            ProtoOp::ReorderFields(o) => (tag::REORDER_FIELDS, o.encode_to_vec()),
            ProtoOp::ChangeFieldNumber(o) => (tag::CHANGE_FIELD_NUMBER, o.encode_to_vec()),
            ProtoOp::CreateMessage(o) => (tag::CREATE_MESSAGE, o.encode_to_vec()),
            ProtoOp::RenameMessage(o) => (tag::RENAME_MESSAGE, o.encode_to_vec()),
            ProtoOp::DeleteMessage(o) => (tag::DELETE_MESSAGE, o.encode_to_vec()),
            ProtoOp::CreateEnum(o) => (tag::CREATE_ENUM, o.encode_to_vec()),
            ProtoOp::DeleteEnum(o) => (tag::DELETE_ENUM, o.encode_to_vec()),
            ProtoOp::AddEnumValue(o) => (tag::ADD_ENUM_VALUE, o.encode_to_vec()),
            ProtoOp::RemoveEnumValue(o) => (tag::REMOVE_ENUM_VALUE, o.encode_to_vec()),
            ProtoOp::RenameEnumValue(o) => (tag::RENAME_ENUM_VALUE, o.encode_to_vec()),
            ProtoOp::AddService(o) => (tag::ADD_SERVICE, o.encode_to_vec()),
            ProtoOp::RemoveService(o) => (tag::REMOVE_SERVICE, o.encode_to_vec()),
            ProtoOp::RenameService(o) => (tag::RENAME_SERVICE, o.encode_to_vec()),
            ProtoOp::AddRpc(o) => (tag::ADD_RPC, o.encode_to_vec()),
            ProtoOp::RemoveRpc(o) => (tag::REMOVE_RPC, o.encode_to_vec()),
            ProtoOp::RenameRpc(o) => (tag::RENAME_RPC, o.encode_to_vec()),
            ProtoOp::ChangeRpcType(o) => (tag::CHANGE_RPC_TYPE, o.encode_to_vec()),
            ProtoOp::UpdateImport(o) => (tag::UPDATE_IMPORT, o.encode_to_vec()),
        };
        ProtoOperation { tag, payload }.encode_to_vec()
    }

    /// Decode an op from the opaque envelope bytes.
    pub fn decode(bytes: &[u8]) -> Result<ProtoOp, MutationError> {
        let env = ProtoOperation::decode(bytes)
            .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
        let p = &env.payload[..];
        let bad = |e: prost::DecodeError| MutationError::InvalidOperationBytes(e.to_string());
        Ok(match env.tag {
            tag::ADD_FIELD => ProtoOp::AddField(OpAddField::decode(p).map_err(bad)?),
            tag::REMOVE_FIELD => ProtoOp::RemoveField(OpRemoveField::decode(p).map_err(bad)?),
            tag::RENAME_FIELD => ProtoOp::RenameField(OpRenameField::decode(p).map_err(bad)?),
            tag::CHANGE_FIELD_TYPE => {
                ProtoOp::ChangeFieldType(OpChangeFieldType::decode(p).map_err(bad)?)
            }
            tag::CHANGE_CARDINALITY => {
                ProtoOp::ChangeCardinality(OpChangeCardinality::decode(p).map_err(bad)?)
            }
            tag::REORDER_FIELDS => ProtoOp::ReorderFields(OpReorderFields::decode(p).map_err(bad)?),
            tag::CHANGE_FIELD_NUMBER => {
                ProtoOp::ChangeFieldNumber(OpChangeFieldNumber::decode(p).map_err(bad)?)
            }
            tag::CREATE_MESSAGE => ProtoOp::CreateMessage(OpCreateMessage::decode(p).map_err(bad)?),
            tag::RENAME_MESSAGE => ProtoOp::RenameMessage(OpRenameMessage::decode(p).map_err(bad)?),
            tag::DELETE_MESSAGE => ProtoOp::DeleteMessage(OpDeleteMessage::decode(p).map_err(bad)?),
            tag::CREATE_ENUM => ProtoOp::CreateEnum(OpCreateEnum::decode(p).map_err(bad)?),
            tag::DELETE_ENUM => ProtoOp::DeleteEnum(OpDeleteEnum::decode(p).map_err(bad)?),
            tag::ADD_ENUM_VALUE => ProtoOp::AddEnumValue(OpAddEnumValue::decode(p).map_err(bad)?),
            tag::REMOVE_ENUM_VALUE => {
                ProtoOp::RemoveEnumValue(OpRemoveEnumValue::decode(p).map_err(bad)?)
            }
            tag::RENAME_ENUM_VALUE => {
                ProtoOp::RenameEnumValue(OpRenameEnumValue::decode(p).map_err(bad)?)
            }
            tag::ADD_SERVICE => ProtoOp::AddService(OpAddService::decode(p).map_err(bad)?),
            tag::REMOVE_SERVICE => ProtoOp::RemoveService(OpRemoveService::decode(p).map_err(bad)?),
            tag::RENAME_SERVICE => ProtoOp::RenameService(OpRenameService::decode(p).map_err(bad)?),
            tag::ADD_RPC => ProtoOp::AddRpc(OpAddRpc::decode(p).map_err(bad)?),
            tag::REMOVE_RPC => ProtoOp::RemoveRpc(OpRemoveRpc::decode(p).map_err(bad)?),
            tag::RENAME_RPC => ProtoOp::RenameRpc(OpRenameRpc::decode(p).map_err(bad)?),
            tag::CHANGE_RPC_TYPE => ProtoOp::ChangeRpcType(OpChangeRpcType::decode(p).map_err(bad)?),
            tag::UPDATE_IMPORT => ProtoOp::UpdateImport(OpUpdateImport::decode(p).map_err(bad)?),
            other => {
                return Err(MutationError::InvalidOperationBytes(format!(
                    "unknown op tag: {other}"
                )))
            }
        })
    }
}
