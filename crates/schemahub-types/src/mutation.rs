use crate::SchemaPath;
use bytes::Bytes;

/// An opaque mutation envelope passed from the gRPC handler to the core,
/// and from the core to the FormatPlugin. The core never inspects `operation`
/// — it only uses `format_id` to select the right plugin.
#[derive(Clone, Debug)]
pub struct Mutation {
    /// Which schema file to mutate.
    pub schema_path: SchemaPath,
    /// Which plugin handles this mutation, e.g. "protobuf", "flatbuffers", "openapi".
    pub format_id: String,
    /// Format-specific operation bytes. Deserialized only by the plugin.
    pub operation: Bytes,
}
