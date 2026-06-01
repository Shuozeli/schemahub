//! OpenAPI mutation operations.
//!
//! Ported from v1's `operations.rs`, switched from `prost` to `serde_json`
//! encoding (the rest of this crate already uses serde_json for blobs).
//!
//! v1 design: OpenAPI mutations are primarily whole-document pushes
//! (`PushDocument`, used by `UpdateSchema`). The v1 plugin also implemented a
//! handful of granular ops (T71–T83) — those are ported here. Anything beyond
//! this set is deferred to v2 and rejected with `UnsupportedInV1`.

use serde::{Deserialize, Serialize};

use schemahub_types::errors::MutationError;

/// The wire envelope for an OpenAPI mutation, serialized into
/// [`schemahub_types::Mutation::operation`] as JSON bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OpenApiOp {
    /// Whole-document replace: re-parse `source` and emit the full decl set.
    PushDocument { source: String },
    /// Add an empty path item.
    AddPath {
        path_pattern: String,
        summary: String,
        description: String,
    },
    /// Remove a path item.
    RemovePath { path_pattern: String },
    /// Add a (bare) operation to an existing path item.
    AddOperation {
        path_pattern: String,
        method: String,
        operation_id: String,
        summary: String,
        description: String,
    },
    /// Remove an operation (by method) from a path item.
    RemoveOperation {
        path_pattern: String,
        method: String,
    },
    /// Add a (bare typed) component schema.
    AddComponentSchema {
        schema_name: String,
        schema_type: String,
        description: String,
    },
    /// Remove a component schema.
    RemoveComponentSchema { schema_name: String },
}

impl OpenApiOp {
    /// Encode an op to operation bytes.
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("OpenApiOp serialization is infallible")
    }

    /// Decode operation bytes into an [`OpenApiOp`].
    pub fn decode(data: &[u8]) -> Result<Self, MutationError> {
        let op: OpenApiOp = serde_json::from_slice(data)
            .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))?;
        if let OpenApiOp::PushDocument { source } = &op {
            if source.is_empty() {
                return Err(MutationError::InvalidOperation(
                    "PushDocument operation has empty source".into(),
                ));
            }
        }
        Ok(op)
    }
}
