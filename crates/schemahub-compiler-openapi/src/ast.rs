//! The in-tree OpenAPI 3.1 AST (`docs/openapi-ast.md`).
//!
//! Ported from the v1 `schemahub-plugin-openapi` AST. The struct *shapes* are
//! unchanged so existing semantic content round-trips identically; only the
//! encoding moved from `prost` to `serde` (serialized via `serde_json` in
//! [`crate::blob`]) and the `oneof` helper modules became plain Rust enums.
//!
//! Every blob carries `blob_version` for migration (see `openapi-ast.md` §8).

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The current blob format version for every OpenAPI decl/meta blob.
pub const BLOB_VERSION: u32 = 1;

/// Error returned by the `FromStr` impls on this module's enums when the
/// input string is not a recognised variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownVariant {
    pub kind: &'static str,
    pub value: String,
}

impl std::fmt::Display for UnknownVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown {} variant: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for UnknownVariant {}

// ── Enums ─────────────────────────────────────────────────────────────────────

/// An HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
    Trace,
}

impl HttpMethod {
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::Patch => "patch",
            Self::Head => "head",
            Self::Options => "options",
            Self::Trace => "trace",
        }
    }
}

impl FromStr for HttpMethod {
    type Err = UnknownVariant;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "get" => Ok(Self::Get),
            "post" => Ok(Self::Post),
            "put" => Ok(Self::Put),
            "delete" => Ok(Self::Delete),
            "patch" => Ok(Self::Patch),
            "head" => Ok(Self::Head),
            "options" => Ok(Self::Options),
            "trace" => Ok(Self::Trace),
            _ => Err(UnknownVariant {
                kind: "HttpMethod",
                value: s.to_string(),
            }),
        }
    }
}

/// Where a parameter appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParameterLocation {
    Query,
    Header,
    Path,
    Cookie,
}

impl ParameterLocation {
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Header => "header",
            Self::Path => "path",
            Self::Cookie => "cookie",
        }
    }
}

impl FromStr for ParameterLocation {
    type Err = UnknownVariant;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "query" => Ok(Self::Query),
            "header" => Ok(Self::Header),
            "path" => Ok(Self::Path),
            "cookie" => Ok(Self::Cookie),
            _ => Err(UnknownVariant {
                kind: "ParameterLocation",
                value: s.to_string(),
            }),
        }
    }
}

/// A JSON Schema `type` keyword value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JsonSchemaType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
    Null,
}

impl JsonSchemaType {
    pub fn to_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Array => "array",
            Self::Object => "object",
            Self::Null => "null",
        }
    }
}

impl FromStr for JsonSchemaType {
    type Err = UnknownVariant;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "string" => Ok(Self::String),
            "integer" => Ok(Self::Integer),
            "number" => Ok(Self::Number),
            "boolean" => Ok(Self::Boolean),
            "array" => Ok(Self::Array),
            "object" => Ok(Self::Object),
            "null" => Ok(Self::Null),
            _ => Err(UnknownVariant {
                kind: "JsonSchemaType",
                value: s.to_string(),
            }),
        }
    }
}

// ── Shared primitive types ────────────────────────────────────────────────────

/// A reference to another schema — local (component name) or external (import).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaRef {
    /// For local refs: the component name, e.g. "User".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local_name: String,
    /// For external (cross-schema) refs. Empty for local refs. v2-modeled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_import: Option<ExternalImport>,
}

/// An import of an external schemahub schema (an external `$ref`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExternalImport {
    /// Logical path: "project/repo/schema-file-name".
    pub path: String,
    /// Pinned commit hash, or empty for a live source-text `$ref`.
    pub resolved_commit: String,
    /// The declaration name within the imported schema.
    pub decl_name: String,
}

/// An opaque map of `x-`-prefixed extension fields, stored as raw JSON bytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Extensions {
    pub json_bytes: Vec<u8>,
}

// ── JSON Schema ───────────────────────────────────────────────────────────────

/// The core recursive JSON Schema type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JsonSchemaDef {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<JsonSchemaType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<SchemaOrRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<PropertyDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_properties_allowed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_properties_schema: Option<Box<SchemaOrRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_properties: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_properties: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all_of: Vec<SchemaOrRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any_of: Vec<SchemaOrRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<SchemaOrRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<SchemaOrRef>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub const_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

/// A schema that is either inline or a `$ref`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SchemaOrRef {
    Inline(Box<JsonSchemaDef>),
    Ref(SchemaRef),
}

impl Default for SchemaOrRef {
    fn default() -> Self {
        Self::Inline(Box::default())
    }
}

/// A named property in an object schema. `Vec<PropertyDef>` preserves order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PropertyDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaOrRef>,
}

// ── Parameter ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParameterDef {
    pub name: String,
    pub location: ParameterLocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaOrRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

impl Default for ParameterDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            location: ParameterLocation::Query,
            description: None,
            required: false,
            deprecated: None,
            schema: None,
            extensions: None,
        }
    }
}

/// A parameter that is either inline or a `$ref` to `components/parameters`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParameterOrRef {
    Inline(Box<ParameterDef>),
    Ref(String),
}

// ── Request body ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RequestBodyDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<MediaTypeEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaTypeEntry {
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaOrRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

/// A requestBody that is either inline or a `$ref` to `components/requestBodies`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RequestBodyOrRef {
    Inline(RequestBodyDef),
    Ref(String),
}

// ── Response ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseDef {
    /// Required in OpenAPI 3.x.
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<MediaTypeEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeaderDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaOrRef>,
}

/// A response that is either inline or a `$ref` to `components/responses`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResponseOrRef {
    Inline(ResponseDef),
    Ref(String),
}

// ── Operation ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationDef {
    pub method: HttpMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterOrRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBodyOrRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<ResponseEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

impl OperationDef {
    /// A bare operation with only its method set (matches v1 `Default` semantics
    /// where `method` is the first field).
    pub fn empty(method: HttpMethod) -> Self {
        Self {
            method,
            operation_id: None,
            summary: None,
            description: None,
            tags: Vec::new(),
            parameters: Vec::new(),
            request_body: None,
            responses: Vec::new(),
            deprecated: None,
            extensions: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseEntry {
    /// HTTP status code as string, or "default".
    pub status_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseOrRef>,
}

// ── Blob types (the stored objects) ─────────────────────────────────────────────

/// Document-level metadata. Serialized into the [`schemahub_types::MetaBlob`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocumentMetadataBlob {
    pub blob_version: u32,
    /// e.g. "3.1.0".
    pub openapi_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<InfoObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InfoObject {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_of_service: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerObject {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathItemBlob {
    pub path_pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterOrRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentSchemaBlob {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<JsonSchemaDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentParameterBlob {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter: Option<ParameterDef>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentResponseBlob {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseDef>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentRequestBodyBlob {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBodyDef>,
}

/// The self-describing payload of one [`schemahub_types::DeclBlob`].
///
/// Unlike the v1 envelope (where the tree-key prefix told you the kind), each
/// decl blob now stands alone in storage keyed by its decl name. The blob must
/// therefore carry its own kind so `summarize_decl` / `decl_detail` /
/// `diff_decl` work without the name. The `blob_version` rides on the wrapper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenApiDecl {
    pub blob_version: u32,
    pub kind: DeclPayload,
}

/// The kind-tagged body of an [`OpenApiDecl`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeclPayload {
    PathItem(PathItemBlob),
    ComponentSchema(Box<ComponentSchemaBlob>),
    ComponentParameter(ComponentParameterBlob),
    ComponentResponse(ComponentResponseBlob),
    ComponentRequestBody(ComponentRequestBodyBlob),
}

impl OpenApiDecl {
    pub fn new(kind: DeclPayload) -> Self {
        Self {
            blob_version: BLOB_VERSION,
            kind,
        }
    }
}
