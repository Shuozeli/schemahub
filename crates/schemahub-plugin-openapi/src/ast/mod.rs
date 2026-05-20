use prost::Message;

// ── Enums ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum HttpMethod {
    Get     = 0,
    Post    = 1,
    Put     = 2,
    Delete  = 3,
    Patch   = 4,
    Head    = 5,
    Options = 6,
    Trace   = 7,
}

impl HttpMethod {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "get"     => Some(Self::Get),
            "post"    => Some(Self::Post),
            "put"     => Some(Self::Put),
            "delete"  => Some(Self::Delete),
            "patch"   => Some(Self::Patch),
            "head"    => Some(Self::Head),
            "options" => Some(Self::Options),
            "trace"   => Some(Self::Trace),
            _         => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Get     => "get",
            Self::Post    => "post",
            Self::Put     => "put",
            Self::Delete  => "delete",
            Self::Patch   => "patch",
            Self::Head    => "head",
            Self::Options => "options",
            Self::Trace   => "trace",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum ParameterLocation {
    Query  = 0,
    Header = 1,
    Path   = 2,
    Cookie = 3,
}

impl ParameterLocation {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "query"  => Some(Self::Query),
            "header" => Some(Self::Header),
            "path"   => Some(Self::Path),
            "cookie" => Some(Self::Cookie),
            _        => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Query  => "query",
            Self::Header => "header",
            Self::Path   => "path",
            Self::Cookie => "cookie",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum JsonSchemaType {
    String  = 0,
    Integer = 1,
    Number  = 2,
    Boolean = 3,
    Array   = 4,
    Object  = 5,
    Null    = 6,
}

impl JsonSchemaType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "string"  => Some(Self::String),
            "integer" => Some(Self::Integer),
            "number"  => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            "array"   => Some(Self::Array),
            "object"  => Some(Self::Object),
            "null"    => Some(Self::Null),
            _         => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::String  => "string",
            Self::Integer => "integer",
            Self::Number  => "number",
            Self::Boolean => "boolean",
            Self::Array   => "array",
            Self::Object  => "object",
            Self::Null    => "null",
        }
    }
}

// ── Shared primitive types ────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct SchemaRef {
    #[prost(string, tag = "1")]
    pub local_name: String,
    #[prost(message, optional, tag = "2")]
    pub external_import: Option<Import>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Import {
    #[prost(string, tag = "1")]
    pub path: String,
    #[prost(string, tag = "2")]
    pub resolved_commit: String,
    #[prost(string, tag = "3")]
    pub decl_name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Extensions {
    #[prost(bytes = "vec", tag = "1")]
    pub json_bytes: Vec<u8>,
}

// ── JSON Schema ───────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct JsonSchemaDef {
    #[prost(enumeration = "JsonSchemaType", repeated, tag = "1")]
    pub types: Vec<i32>,
    #[prost(string, optional, tag = "2")]
    pub format: Option<String>,
    #[prost(uint64, optional, tag = "3")]
    pub min_length: Option<u64>,
    #[prost(uint64, optional, tag = "4")]
    pub max_length: Option<u64>,
    #[prost(string, optional, tag = "5")]
    pub pattern: Option<String>,
    #[prost(double, optional, tag = "6")]
    pub minimum: Option<f64>,
    #[prost(double, optional, tag = "7")]
    pub maximum: Option<f64>,
    #[prost(bool, optional, tag = "8")]
    pub exclusive_minimum: Option<bool>,
    #[prost(bool, optional, tag = "9")]
    pub exclusive_maximum: Option<bool>,
    #[prost(double, optional, tag = "10")]
    pub multiple_of: Option<f64>,
    #[prost(message, optional, boxed, tag = "11")]
    pub items: Option<Box<SchemaOrRef>>,
    #[prost(uint64, optional, tag = "12")]
    pub min_items: Option<u64>,
    #[prost(uint64, optional, tag = "13")]
    pub max_items: Option<u64>,
    #[prost(bool, optional, tag = "14")]
    pub unique_items: Option<bool>,
    #[prost(message, repeated, tag = "15")]
    pub properties: Vec<PropertyDef>,
    #[prost(string, repeated, tag = "16")]
    pub required: Vec<String>,
    #[prost(bool, optional, tag = "17")]
    pub additional_properties_allowed: Option<bool>,
    #[prost(message, optional, boxed, tag = "18")]
    pub additional_properties_schema: Option<Box<SchemaOrRef>>,
    #[prost(uint64, optional, tag = "19")]
    pub min_properties: Option<u64>,
    #[prost(uint64, optional, tag = "20")]
    pub max_properties: Option<u64>,
    #[prost(message, repeated, tag = "21")]
    pub all_of: Vec<SchemaOrRef>,
    #[prost(message, repeated, tag = "22")]
    pub any_of: Vec<SchemaOrRef>,
    #[prost(message, repeated, tag = "23")]
    pub one_of: Vec<SchemaOrRef>,
    #[prost(message, optional, boxed, tag = "24")]
    pub not: Option<Box<SchemaOrRef>>,
    #[prost(string, repeated, tag = "25")]
    pub enum_values: Vec<String>,
    #[prost(string, optional, tag = "26")]
    pub const_value: Option<String>,
    #[prost(string, optional, tag = "27")]
    pub title: Option<String>,
    #[prost(string, optional, tag = "28")]
    pub description: Option<String>,
    #[prost(string, optional, tag = "29")]
    pub default: Option<String>,
    #[prost(bool, optional, tag = "30")]
    pub deprecated: Option<bool>,
    #[prost(bool, optional, tag = "31")]
    pub read_only: Option<bool>,
    #[prost(bool, optional, tag = "32")]
    pub write_only: Option<bool>,
    #[prost(message, optional, tag = "33")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SchemaOrRef {
    #[prost(oneof = "schema_or_ref::Value", tags = "1, 2")]
    pub value: Option<schema_or_ref::Value>,
}

pub mod schema_or_ref {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Value {
        #[prost(message, tag = "1")]
        Inline(super::JsonSchemaDef),
        #[prost(message, tag = "2")]
        Ref(super::SchemaRef),
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PropertyDef {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, optional, tag = "2")]
    pub schema: Option<SchemaOrRef>,
}

// ── Parameter ─────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct ParameterDef {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(enumeration = "ParameterLocation", tag = "2")]
    pub location: i32,
    #[prost(string, optional, tag = "3")]
    pub description: Option<String>,
    #[prost(bool, tag = "4")]
    pub required: bool,
    #[prost(bool, optional, tag = "5")]
    pub deprecated: Option<bool>,
    #[prost(message, optional, tag = "6")]
    pub schema: Option<SchemaOrRef>,
    #[prost(message, optional, tag = "7")]
    pub extensions: Option<Extensions>,
}

// ── Request body ──────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct RequestBodyDef {
    #[prost(string, optional, tag = "1")]
    pub description: Option<String>,
    #[prost(bool, tag = "2")]
    pub required: bool,
    #[prost(message, repeated, tag = "3")]
    pub content: Vec<MediaTypeEntry>,
    #[prost(message, optional, tag = "4")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct MediaTypeEntry {
    #[prost(string, tag = "1")]
    pub media_type: String,
    #[prost(message, optional, tag = "2")]
    pub schema: Option<SchemaOrRef>,
    #[prost(message, optional, tag = "3")]
    pub extensions: Option<Extensions>,
}

// ── Response ──────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct ResponseDef {
    #[prost(string, tag = "1")]
    pub description: String,
    #[prost(message, repeated, tag = "2")]
    pub content: Vec<MediaTypeEntry>,
    #[prost(message, repeated, tag = "3")]
    pub headers: Vec<HeaderDef>,
    #[prost(message, optional, tag = "4")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct HeaderDef {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, optional, tag = "2")]
    pub description: Option<String>,
    #[prost(bool, optional, tag = "3")]
    pub required: Option<bool>,
    #[prost(message, optional, tag = "4")]
    pub schema: Option<SchemaOrRef>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ResponseOrRef {
    #[prost(oneof = "response_or_ref::Value", tags = "1, 2")]
    pub value: Option<response_or_ref::Value>,
}

pub mod response_or_ref {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Value {
        #[prost(message, tag = "1")]
        Inline(super::ResponseDef),
        #[prost(string, tag = "2")]
        Ref(String),
    }
}

// ── Operation ─────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct OperationDef {
    #[prost(enumeration = "HttpMethod", tag = "1")]
    pub method: i32,
    #[prost(string, optional, tag = "2")]
    pub operation_id: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub summary: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub description: Option<String>,
    #[prost(string, repeated, tag = "5")]
    pub tags: Vec<String>,
    #[prost(message, repeated, tag = "6")]
    pub parameters: Vec<ParameterOrRef>,
    #[prost(message, optional, tag = "7")]
    pub request_body: Option<RequestBodyOrRef>,
    #[prost(message, repeated, tag = "8")]
    pub responses: Vec<ResponseEntry>,
    #[prost(bool, optional, tag = "9")]
    pub deprecated: Option<bool>,
    #[prost(message, optional, tag = "10")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ResponseEntry {
    #[prost(string, tag = "1")]
    pub status_code: String,
    #[prost(message, optional, tag = "2")]
    pub response: Option<ResponseOrRef>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ParameterOrRef {
    #[prost(oneof = "parameter_or_ref::Value", tags = "1, 2")]
    pub value: Option<parameter_or_ref::Value>,
}

pub mod parameter_or_ref {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Value {
        #[prost(message, tag = "1")]
        Inline(super::ParameterDef),
        #[prost(string, tag = "2")]
        Ref(String),
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct RequestBodyOrRef {
    #[prost(oneof = "request_body_or_ref::Value", tags = "1, 2")]
    pub value: Option<request_body_or_ref::Value>,
}

pub mod request_body_or_ref {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Value {
        #[prost(message, tag = "1")]
        Inline(super::RequestBodyDef),
        #[prost(string, tag = "2")]
        Ref(String),
    }
}

// ── Blob types ────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct DocumentMetadataBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub openapi_version: String,
    #[prost(message, optional, tag = "3")]
    pub info: Option<InfoObject>,
    #[prost(message, repeated, tag = "4")]
    pub servers: Vec<ServerObject>,
    #[prost(message, optional, tag = "5")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct InfoObject {
    #[prost(string, tag = "1")]
    pub title: String,
    #[prost(string, optional, tag = "2")]
    pub description: Option<String>,
    #[prost(string, tag = "3")]
    pub version: String,
    #[prost(string, optional, tag = "4")]
    pub terms_of_service: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ServerObject {
    #[prost(string, tag = "1")]
    pub url: String,
    #[prost(string, optional, tag = "2")]
    pub description: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PathItemBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub path_pattern: String,
    #[prost(string, optional, tag = "3")]
    pub summary: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub description: Option<String>,
    #[prost(message, repeated, tag = "5")]
    pub parameters: Vec<ParameterOrRef>,
    #[prost(message, repeated, tag = "6")]
    pub operations: Vec<OperationDef>,
    #[prost(message, optional, tag = "7")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentSchemaBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub schema: Option<JsonSchemaDef>,
    #[prost(message, optional, tag = "4")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentParameterBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub parameter: Option<ParameterDef>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentResponseBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub response: Option<ResponseDef>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentRequestBodyBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub request_body: Option<RequestBodyDef>,
}

// ── Parse result envelope ─────────────────────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct OpenApiParseResult {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,
    #[prost(message, repeated, tag = "2")]
    pub declarations: Vec<ParsedDeclaration>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ParsedDeclaration {
    #[prost(string, tag = "1")]
    pub tree_key: String,
    #[prost(bytes = "vec", tag = "2")]
    pub blob_bytes: Vec<u8>,
}

// ── Encode / decode helpers ───────────────────────────────────────────────────

pub fn encode_parse_result(r: &OpenApiParseResult) -> Vec<u8> {
    r.encode_to_vec()
}

pub fn decode_parse_result(b: &[u8]) -> Result<OpenApiParseResult, prost::DecodeError> {
    OpenApiParseResult::decode(b)
}

pub fn encode_metadata(b: &DocumentMetadataBlob) -> Vec<u8> {
    b.encode_to_vec()
}

pub fn decode_metadata(b: &[u8]) -> Result<DocumentMetadataBlob, prost::DecodeError> {
    DocumentMetadataBlob::decode(b)
}

pub fn encode_path_item(b: &PathItemBlob) -> Vec<u8> {
    b.encode_to_vec()
}

pub fn decode_path_item(b: &[u8]) -> Result<PathItemBlob, prost::DecodeError> {
    PathItemBlob::decode(b)
}

pub fn encode_component_schema(b: &ComponentSchemaBlob) -> Vec<u8> {
    b.encode_to_vec()
}

pub fn decode_component_schema(b: &[u8]) -> Result<ComponentSchemaBlob, prost::DecodeError> {
    ComponentSchemaBlob::decode(b)
}

pub fn encode_component_parameter(b: &ComponentParameterBlob) -> Vec<u8> {
    b.encode_to_vec()
}

pub fn decode_component_parameter(b: &[u8]) -> Result<ComponentParameterBlob, prost::DecodeError> {
    ComponentParameterBlob::decode(b)
}

pub fn encode_component_response(b: &ComponentResponseBlob) -> Vec<u8> {
    b.encode_to_vec()
}

pub fn decode_component_response(b: &[u8]) -> Result<ComponentResponseBlob, prost::DecodeError> {
    ComponentResponseBlob::decode(b)
}

pub fn encode_component_request_body(b: &ComponentRequestBodyBlob) -> Vec<u8> {
    b.encode_to_vec()
}

pub fn decode_component_request_body(
    b: &[u8],
) -> Result<ComponentRequestBodyBlob, prost::DecodeError> {
    ComponentRequestBodyBlob::decode(b)
}
