use prost::Message;
use schemahub_types::errors::MutationError;

// ── Tag constants ─────────────────────────────────────────────────────────────

pub mod op_tag {
    pub const PUSH_DOCUMENT:          u32 = 1;
    pub const ADD_PATH:               u32 = 10;
    pub const REMOVE_PATH:            u32 = 11;
    pub const ADD_OPERATION:          u32 = 20;
    pub const REMOVE_OPERATION:       u32 = 21;
    pub const ADD_COMPONENT_SCHEMA:   u32 = 30;
    pub const REMOVE_COMPONENT_SCHEMA: u32 = 31;
}

// ── Prost-encoded envelope ────────────────────────────────────────────────────

/// Wire envelope for all OpenAPI plugin operations.
/// tag=1: u32 operation tag; tag=2: bytes inner payload.
#[derive(Clone, PartialEq, prost::Message)]
pub struct OpenApiOperationEnvelope {
    #[prost(uint32, tag = "1")]
    pub tag: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

impl OpenApiOperationEnvelope {
    pub fn encode_op(tag: u32, payload: Vec<u8>) -> Vec<u8> {
        let env = Self { tag, payload };
        env.encode_to_vec()
    }
}

// ── Inner op structs (prost-encoded) ─────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpPushDocument {
    #[prost(bytes = "vec", tag = "1")]
    pub source: Vec<u8>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddPath {
    #[prost(string, tag = "1")]
    pub path_pattern: String,
    #[prost(string, tag = "2")]
    pub summary: String,
    #[prost(string, tag = "3")]
    pub description: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemovePath {
    #[prost(string, tag = "1")]
    pub path_pattern: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddOperation {
    #[prost(string, tag = "1")]
    pub path_pattern: String,
    #[prost(string, tag = "2")]
    pub method: String,
    #[prost(string, tag = "3")]
    pub operation_id: String,
    #[prost(string, tag = "4")]
    pub summary: String,
    #[prost(string, tag = "5")]
    pub description: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveOperation {
    #[prost(string, tag = "1")]
    pub path_pattern: String,
    #[prost(string, tag = "2")]
    pub method: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpAddComponentSchema {
    #[prost(string, tag = "1")]
    pub schema_name: String,
    #[prost(string, tag = "2")]
    pub schema_type: String,
    #[prost(string, tag = "3")]
    pub description: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpRemoveComponentSchema {
    #[prost(string, tag = "1")]
    pub schema_name: String,
}

// ── OpenApiOp enum ────────────────────────────────────────────────────────────

/// Decoded operation ready for the plugin's apply_mutation.
pub enum OpenApiOp {
    PushDocument(OpPushDocument),
    AddPath(OpAddPath),
    RemovePath(OpRemovePath),
    AddOperation(OpAddOperation),
    RemoveOperation(OpRemoveOperation),
    AddComponentSchema(OpAddComponentSchema),
    RemoveComponentSchema(OpRemoveComponentSchema),
}

// ── Legacy type alias for the PushDocument op (kept for existing unit tests) ──

pub struct PushDocumentOp {
    pub source: Vec<u8>,
}

pub fn encode_push_document(op: &PushDocumentOp) -> Vec<u8> {
    let inner = OpPushDocument { source: op.source.clone() };
    OpenApiOperationEnvelope::encode_op(op_tag::PUSH_DOCUMENT, inner.encode_to_vec())
}

// ── Decode ────────────────────────────────────────────────────────────────────

pub fn decode_operation(data: &[u8]) -> Result<OpenApiOp, MutationError> {
    let env = OpenApiOperationEnvelope::decode(data)
        .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;

    match env.tag {
        op_tag::PUSH_DOCUMENT => {
            let inner = OpPushDocument::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            if inner.source.is_empty() {
                return Err(MutationError::InvalidOperation(
                    "PushDocument operation has empty source".into(),
                ));
            }
            Ok(OpenApiOp::PushDocument(inner))
        }
        op_tag::ADD_PATH => {
            let inner = OpAddPath::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            Ok(OpenApiOp::AddPath(inner))
        }
        op_tag::REMOVE_PATH => {
            let inner = OpRemovePath::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            Ok(OpenApiOp::RemovePath(inner))
        }
        op_tag::ADD_OPERATION => {
            let inner = OpAddOperation::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            Ok(OpenApiOp::AddOperation(inner))
        }
        op_tag::REMOVE_OPERATION => {
            let inner = OpRemoveOperation::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            Ok(OpenApiOp::RemoveOperation(inner))
        }
        op_tag::ADD_COMPONENT_SCHEMA => {
            let inner = OpAddComponentSchema::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            Ok(OpenApiOp::AddComponentSchema(inner))
        }
        op_tag::REMOVE_COMPONENT_SCHEMA => {
            let inner = OpRemoveComponentSchema::decode(env.payload.as_slice())
                .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
            Ok(OpenApiOp::RemoveComponentSchema(inner))
        }
        other => Err(MutationError::InvalidOperation(
            format!("unknown OpenAPI operation tag: {other}"),
        )),
    }
}
