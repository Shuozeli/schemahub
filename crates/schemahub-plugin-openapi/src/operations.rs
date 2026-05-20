use prost::Message;
use schemahub_types::errors::MutationError;

/// The only OpenAPI v1 mutation operation.
pub struct PushDocumentOp {
    /// Raw YAML/JSON bytes of the new OpenAPI document.
    pub source: Vec<u8>,
}

/// Prost-encoded envelope for OpenAPI operations.
#[derive(Clone, PartialEq, prost::Message)]
pub struct OpenApiOperationEnvelope {
    #[prost(bytes = "vec", tag = "1")]
    pub push_document_source: Vec<u8>,
}

pub fn encode_push_document(op: &PushDocumentOp) -> Vec<u8> {
    let envelope = OpenApiOperationEnvelope {
        push_document_source: op.source.clone(),
    };
    envelope.encode_to_vec()
}

pub fn decode_operation(data: &[u8]) -> Result<PushDocumentOp, MutationError> {
    let envelope = OpenApiOperationEnvelope::decode(data)
        .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;

    if envelope.push_document_source.is_empty() {
        return Err(MutationError::InvalidOperation(
            "PushDocument operation has empty source".into(),
        ));
    }

    Ok(PushDocumentOp { source: envelope.push_document_source })
}
