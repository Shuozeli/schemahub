# schemahub — OpenAPI AST Specification

> This document specifies the internal AST model for OpenAPI schemas in schemahub. It exists because `design.md` (OQ-18) requires the OpenAPI AST to be fully specified before implementation — even though granular OpenAPI mutations are deferred to v2. The AST produced by v1's `parse` must be structurally identical to what v2 granular mutations would produce, to avoid a blob migration.

---

## 1. Background and Constraints

### Why this document exists

The `FormatPlugin::parse(source: &str) -> Blob` method ingests an OpenAPI document and produces a blob stored content-addressed in the object store. In v2, granular mutations (`AddEndpoint`, `RemoveParameter`, etc.) will produce blobs through a different code path. If the v1 and v2 AST structures differ, every existing OpenAPI blob requires migration at v2 launch.

**Constraint:** The v1 AST must be designed assuming v2 granular mutations exist. Every element must be individually addressable by a stable path. The v1 `parse` path and the v2 mutation path must produce identical byte sequences for identical semantic content.

### OpenAPI version support

This specification targets **OpenAPI 3.1.x**. OpenAPI 2.x (Swagger) is not supported in v1. The parser rejects documents with `openapi:` fields that don't start with `3.`.

### What schemahub stores vs. what it does not

schemahub stores the **semantic content** of an OpenAPI document as a structured AST. It does NOT store:
- YAML/JSON formatting (indentation, key ordering, comment placement)
- `x-` extension fields (preserved as opaque bytes, not interpreted)
- Example values that do not affect schema validation behavior

The `print` method produces canonical YAML from the AST — round-tripping through parse→print may change formatting but must preserve all semantic content.

---

## 2. Blob Granularity

One blob per top-level addressable declaration. For OpenAPI, the top-level declarations are:

| Declaration kind | One blob per... | Example |
|-----------------|-----------------|---------|
| `PathItem` | Path pattern | `/users`, `/users/{id}` |
| `ComponentSchema` | Named schema in `components/schemas` | `User`, `Error` |
| `ComponentParameter` | Named parameter in `components/parameters` | `PageSize`, `Authorization` |
| `ComponentResponse` | Named response in `components/responses` | `NotFound`, `Unauthorized` |
| `ComponentRequestBody` | Named requestBody in `components/requestBodies` | `CreateUserRequest` |
| `DocumentMetadata` | The whole document (exactly one per schema file) | `(document root)` |

A single OpenAPI file therefore produces multiple blobs: one `DocumentMetadata` blob and one blob per path item and per component object. These are all listed in the schema-level tree (the second level of the two-level tree structure).

### DocumentMetadata blob

A special blob that stores document-level fields that are not declarations: the OpenAPI version string, `info`, and `servers`. Every OpenAPI schema file produces exactly one `DocumentMetadata` blob. It is stored in the schema tree under the reserved key `__metadata__`.

```
schema_tree["user-api.yaml"] → {
    "__metadata__":       metadata_blob_hash,
    "path:/users":        path_item_blob_hash_A,
    "path:/users/{id}":   path_item_blob_hash_B,
    "schema:User":        schema_blob_hash_C,
    "schema:Error":       schema_blob_hash_D,
    "param:PageSize":     param_blob_hash_E,
    "response:NotFound":  response_blob_hash_F,
}
```

The schema tree key format encodes the declaration kind as a prefix (`path:`, `schema:`, `param:`, `response:`, `requestBody:`) to avoid collisions between a path named `User` and a component schema named `User`.

### Inline schemas

OpenAPI allows schemas to be defined inline (not in `components/schemas`). Inline schemas are stored **within their containing blob** — they are not extracted into separate top-level blobs. A `$ref` to a component schema is stored symbolically (see Section 5). This means:

- The PathItem blob for `/users` contains the full inline schema of its response body, if that body is not a `$ref`.
- Only schemas listed under `components/schemas` get their own top-level blob.

This matches how OpenAPI authors think about their documents: `components/schemas` entries are reusable named types; inline schemas are local to one operation.

---

## 3. Stable Path Model

Every element in the OpenAPI AST has a stable string path. These paths define:
1. The future v2 granular mutation operation identifiers (`AddParameter { path: "path:/users/GET/parameters" }`)
2. The `get_declaration` sub-element address within a blob
3. The format for search and index entries

### Path grammar

```
document_path ::= "path:" path_pattern
                | "schema:" component_name
                | "param:" component_name
                | "response:" component_name
                | "requestBody:" component_name
                | "__metadata__"

element_path  ::= document_path                              # top-level blob
                | document_path "/" element_segment+         # sub-element within blob

element_segment ::= http_method                              # GET, POST, PUT, DELETE, ...
                  | "parameters" "/" "{" param_name "}"
                  | "requestBody"
                  | "responses" "/" "{" status_code "}"
                  | "content" "/" "{" media_type "}"
                  | "schema"
                  | "properties" "/" "{" property_name "}"
                  | "items"
                  | "allOf" "/" "[" index "]"
                  | "anyOf" "/" "[" index "]"
                  | "oneOf" "/" "[" index "]"
```

### Examples

```
# Top-level blobs (schema tree keys)
path:/users
path:/users/{id}
schema:User
param:PageSize
response:NotFound
__metadata__

# Sub-elements within blobs (for future granular mutations)
path:/users/GET
path:/users/GET/parameters/{limit}
path:/users/GET/parameters/{limit}/schema
path:/users/GET/responses/{200}
path:/users/GET/responses/{200}/content/{application/json}/schema
path:/users/GET/responses/{200}/content/{application/json}/schema/properties/{id}
path:/users/POST/requestBody/content/{application/json}/schema
schema:User/properties/{email}
schema:User/properties/{address}/properties/{street}
schema:User/allOf/[0]
```

---

## 4. Rust AST Type Definitions

All types below are serialized to bytes via `prost` (Protocol Buffers). Every blob type carries `blob_version: u32` as field 1. The prost-encoded bytes of the root blob struct are stored in `objects/<hash>`.

### 4.1 Shared primitive types

```rust
/// An HTTP method.
#[derive(Clone, PartialEq, prost::Message)]
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

/// Where a parameter appears.
#[derive(Clone, PartialEq, prost::Message)]
pub enum ParameterLocation {
    Query  = 0,
    Header = 1,
    Path   = 2,
    Cookie = 3,
}

/// A JSON Schema type keyword value.
#[derive(Clone, PartialEq, prost::Message)]
pub enum JsonSchemaType {
    String  = 0,
    Integer = 1,
    Number  = 2,
    Boolean = 3,
    Array   = 4,
    Object  = 5,
    Null    = 6,
}

/// A reference to another schema — either local (within the same schemahub schema file)
/// or external (another schemahub schema, tracked in the deps/ graph).
#[derive(Clone, PartialEq, prost::Message)]
pub struct SchemaRef {
    /// For local refs: the component name, e.g. "User".
    /// Corresponds to #/components/schemas/User in the source document.
    #[prost(string, tag = "1")]
    pub local_name: String,

    /// For external refs: the import that declares where this type lives.
    /// Empty for local refs.
    #[prost(message, optional, tag = "2")]
    pub external_import: Option<Import>,
}

/// An import of an external schemahub schema (external $ref).
#[derive(Clone, PartialEq, prost::Message)]
pub struct Import {
    /// Logical path: "project/repo/schema-file-name"
    #[prost(string, tag = "1")]
    pub path: String,

    /// Pinned commit hash at the time this import was added or last updated.
    #[prost(string, tag = "2")]
    pub resolved_commit: String,

    /// The declaration name within the imported schema.
    #[prost(string, tag = "3")]
    pub decl_name: String,
}

/// An opaque map of extension fields (x- prefixed keys).
/// Stored as raw JSON bytes; not interpreted by schemahub.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Extensions {
    #[prost(bytes, tag = "1")]
    pub json_bytes: Vec<u8>,
}
```

### 4.2 JSON Schema definition

The core recursive type. Used inside ParameterDef, RequestBodyDef, ResponseDef, and as the body of ComponentSchemaBlob.

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct JsonSchemaDef {
    // ── Type and format ──────────────────────────────────────────────────────
    /// The JSON Schema type keyword. Multiple types (OpenAPI 3.1 nullable arrays)
    /// are expressed as anyOf with a null type.
    #[prost(enumeration = "JsonSchemaType", repeated, tag = "1")]
    pub types: Vec<i32>,

    /// The format keyword (e.g. "date-time", "uuid", "int64").
    #[prost(string, optional, tag = "2")]
    pub format: Option<String>,

    // ── String constraints ───────────────────────────────────────────────────
    #[prost(uint64, optional, tag = "3")]
    pub min_length: Option<u64>,
    #[prost(uint64, optional, tag = "4")]
    pub max_length: Option<u64>,
    #[prost(string, optional, tag = "5")]
    pub pattern: Option<String>,

    // ── Numeric constraints ──────────────────────────────────────────────────
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

    // ── Array constraints ────────────────────────────────────────────────────
    #[prost(message, optional, boxed, tag = "11")]
    pub items: Option<Box<SchemaOrRef>>,
    #[prost(uint64, optional, tag = "12")]
    pub min_items: Option<u64>,
    #[prost(uint64, optional, tag = "13")]
    pub max_items: Option<u64>,
    #[prost(bool, optional, tag = "14")]
    pub unique_items: Option<bool>,

    // ── Object constraints ───────────────────────────────────────────────────
    /// BTreeMap for deterministic serialization order.
    #[prost(message, repeated, tag = "15")]
    pub properties: Vec<PropertyDef>,
    #[prost(string, repeated, tag = "16")]
    pub required: Vec<String>,
    /// additionalProperties: false is stored as additional_properties_allowed = false.
    /// additionalProperties: <schema> is stored in additional_properties_schema.
    #[prost(bool, optional, tag = "17")]
    pub additional_properties_allowed: Option<bool>,
    #[prost(message, optional, boxed, tag = "18")]
    pub additional_properties_schema: Option<Box<SchemaOrRef>>,
    #[prost(uint64, optional, tag = "19")]
    pub min_properties: Option<u64>,
    #[prost(uint64, optional, tag = "20")]
    pub max_properties: Option<u64>,

    // ── Composition keywords ─────────────────────────────────────────────────
    #[prost(message, repeated, tag = "21")]
    pub all_of: Vec<SchemaOrRef>,
    #[prost(message, repeated, tag = "22")]
    pub any_of: Vec<SchemaOrRef>,
    #[prost(message, repeated, tag = "23")]
    pub one_of: Vec<SchemaOrRef>,
    #[prost(message, optional, boxed, tag = "24")]
    pub not: Option<Box<SchemaOrRef>>,

    // ── Enum and const ───────────────────────────────────────────────────────
    /// JSON-encoded enum values (each entry is a JSON literal: `"active"`, `1`, `null`).
    #[prost(string, repeated, tag = "25")]
    pub enum_values: Vec<String>,
    /// JSON-encoded const value.
    #[prost(string, optional, tag = "26")]
    pub const_value: Option<String>,

    // ── Metadata ─────────────────────────────────────────────────────────────
    #[prost(string, optional, tag = "27")]
    pub title: Option<String>,
    #[prost(string, optional, tag = "28")]
    pub description: Option<String>,
    /// JSON-encoded default value.
    #[prost(string, optional, tag = "29")]
    pub default: Option<String>,
    #[prost(bool, optional, tag = "30")]
    pub deprecated: Option<bool>,
    #[prost(bool, optional, tag = "31")]
    pub read_only: Option<bool>,
    #[prost(bool, optional, tag = "32")]
    pub write_only: Option<bool>,

    // ── Extensions ───────────────────────────────────────────────────────────
    #[prost(message, optional, tag = "33")]
    pub extensions: Option<Extensions>,
}

/// A schema that is either inline or a $ref.
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

/// A named property entry in an object schema.
/// Stored as a Vec<PropertyDef> (not a HashMap) to preserve declaration order.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PropertyDef {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, optional, tag = "2")]
    pub schema: Option<SchemaOrRef>,
}
```

### 4.3 Parameter definition

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct ParameterDef {
    #[prost(string, tag = "1")]
    pub name: String,

    #[prost(enumeration = "ParameterLocation", tag = "2")]
    pub location: i32,

    #[prost(string, optional, tag = "3")]
    pub description: Option<String>,

    /// Path parameters are always required. For other locations, this is explicit.
    #[prost(bool, tag = "4")]
    pub required: bool,

    #[prost(bool, optional, tag = "5")]
    pub deprecated: Option<bool>,

    /// The schema for this parameter's value.
    #[prost(message, optional, tag = "6")]
    pub schema: Option<SchemaOrRef>,

    #[prost(message, optional, tag = "7")]
    pub extensions: Option<Extensions>,
}
```

### 4.4 Request body definition

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct RequestBodyDef {
    #[prost(string, optional, tag = "1")]
    pub description: Option<String>,

    #[prost(bool, tag = "2")]
    pub required: bool,

    /// Media type → content definition. Stored as Vec for ordering stability.
    #[prost(message, repeated, tag = "3")]
    pub content: Vec<MediaTypeEntry>,

    #[prost(message, optional, tag = "4")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct MediaTypeEntry {
    /// e.g. "application/json", "multipart/form-data"
    #[prost(string, tag = "1")]
    pub media_type: String,

    #[prost(message, optional, tag = "2")]
    pub schema: Option<SchemaOrRef>,

    #[prost(message, optional, tag = "3")]
    pub extensions: Option<Extensions>,
}
```

### 4.5 Response definition

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct ResponseDef {
    #[prost(string, tag = "1")]
    pub description: String,  // required in OpenAPI 3.x

    #[prost(message, repeated, tag = "2")]
    pub content: Vec<MediaTypeEntry>,

    /// Response headers.
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

/// A response that is either inline or a $ref to components/responses.
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
        /// Component name in components/responses.
        #[prost(string, tag = "2")]
        Ref(String),
    }
}
```

### 4.6 Operation definition

```rust
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

    /// Parameters defined at the operation level.
    /// These are merged with path-level parameters; operation parameters override.
    #[prost(message, repeated, tag = "6")]
    pub parameters: Vec<ParameterOrRef>,

    #[prost(message, optional, tag = "7")]
    pub request_body: Option<RequestBodyOrRef>,

    /// Status code (or "default") → response. Stored as Vec for ordering stability.
    #[prost(message, repeated, tag = "8")]
    pub responses: Vec<ResponseEntry>,

    #[prost(bool, optional, tag = "9")]
    pub deprecated: Option<bool>,

    #[prost(message, optional, tag = "10")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ResponseEntry {
    /// HTTP status code as string, or "default".
    #[prost(string, tag = "1")]
    pub status_code: String,
    #[prost(message, optional, tag = "2")]
    pub response: Option<ResponseOrRef>,
}

/// A parameter that is either inline or a $ref to components/parameters.
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
        /// Component name in components/parameters.
        #[prost(string, tag = "2")]
        Ref(String),
    }
}

/// A requestBody that is either inline or a $ref to components/requestBodies.
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
        /// Component name in components/requestBodies.
        #[prost(string, tag = "2")]
        Ref(String),
    }
}
```

### 4.7 Blob types (the stored objects)

#### DocumentMetadataBlob

Stored under `__metadata__` in the schema tree.

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct DocumentMetadataBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,

    /// e.g. "3.1.0"
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
```

#### PathItemBlob

Stored under `path:<path_pattern>` in the schema tree.

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct PathItemBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,

    /// The path pattern, e.g. "/users/{id}". Stored in the blob (not just the tree key)
    /// so the blob is self-describing after lookup.
    #[prost(string, tag = "2")]
    pub path_pattern: String,

    #[prost(string, optional, tag = "3")]
    pub summary: Option<String>,

    #[prost(string, optional, tag = "4")]
    pub description: Option<String>,

    /// Path-level parameters (inherited by all operations unless overridden).
    #[prost(message, repeated, tag = "5")]
    pub parameters: Vec<ParameterOrRef>,

    /// The operations on this path. At most one per HTTP method.
    #[prost(message, repeated, tag = "6")]
    pub operations: Vec<OperationDef>,

    #[prost(message, optional, tag = "7")]
    pub extensions: Option<Extensions>,
}
```

#### ComponentSchemaBlob

Stored under `schema:<name>` in the schema tree.

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentSchemaBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,

    /// The component name, e.g. "User".
    #[prost(string, tag = "2")]
    pub name: String,

    #[prost(message, optional, tag = "3")]
    pub schema: Option<JsonSchemaDef>,

    #[prost(message, optional, tag = "4")]
    pub extensions: Option<Extensions>,
}
```

#### ComponentParameterBlob

Stored under `param:<name>` in the schema tree.

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentParameterBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,

    #[prost(string, tag = "2")]
    pub name: String,

    #[prost(message, optional, tag = "3")]
    pub parameter: Option<ParameterDef>,
}
```

#### ComponentResponseBlob

Stored under `response:<name>` in the schema tree.

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentResponseBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,

    #[prost(string, tag = "2")]
    pub name: String,

    #[prost(message, optional, tag = "3")]
    pub response: Option<ResponseDef>,
}
```

#### ComponentRequestBodyBlob

Stored under `requestBody:<name>` in the schema tree.

```rust
#[derive(Clone, PartialEq, prost::Message)]
pub struct ComponentRequestBodyBlob {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,

    #[prost(string, tag = "2")]
    pub name: String,

    #[prost(message, optional, tag = "3")]
    pub request_body: Option<RequestBodyDef>,
}
```

---

## 5. `$ref` Handling

### Local `$ref` (within the same schema file)

A `$ref` of the form `#/components/schemas/User` is stored as a `SchemaOrRef::Ref` with `local_name = "User"` and no `external_import`. The core does not resolve local refs at storage time — they remain symbolic.

When the core needs to follow a local ref (e.g., for `FollowType` or `generate_descriptors`), it looks up `schema:User` in the same schema tree.

### External `$ref` (cross-schema import)

A `$ref` pointing to another file (e.g., `./common.yaml#/components/schemas/Address` or a full schemahub path `schemahub://payments/common-types/address.yaml#/components/schemas/Address`) is stored as a `SchemaOrRef::Ref` with a populated `external_import` field:

```rust
SchemaOrRef::Ref(SchemaRef {
    local_name: "",
    external_import: Some(Import {
        path:             "payments/common-types/address.yaml",
        resolved_commit:  "a3f9c2d...",
        decl_name:        "Address",
    }),
})
```

External imports are tracked in the `deps/` index. The `imports()` method on the plugin extracts all `Import` structs from a blob for BFS traversal.

### `$ref` in the source that points to a component within the same file

OpenAPI documents commonly reference their own components:
```yaml
responses:
  '200':
    content:
      application/json:
        schema:
          $ref: '#/components/schemas/User'
```

This becomes `SchemaOrRef::Ref { local_name: "User", external_import: None }` in the PathItemBlob. The `ComponentSchemaBlob` for `User` is a separate blob in the same schema tree.

---

## 6. `FormatPlugin` Method Behaviors for OpenAPI

### 6.1 `parse(source: &str) -> Result<Blob, ParseError>`

**Input:** Raw YAML or JSON string of a complete OpenAPI 3.1 document.

**Output:** This method is unusual for OpenAPI — it produces not one blob but a set of blobs. However, the `FormatPlugin` interface returns a single `Blob`. The resolution:

`parse` for OpenAPI returns a **root envelope blob** that contains the list of (schema-tree-key, blob-bytes) pairs for all declarations in the document. The core unwraps the envelope and stores each declaration blob separately, building the schema tree from the envelope contents.

```rust
/// Returned by parse() for OpenAPI documents.
/// The core unwraps this to populate the schema tree.
#[derive(Clone, PartialEq, prost::Message)]
pub struct OpenApiParseResult {
    #[prost(uint32, tag = "1")]
    pub blob_version: u32,

    /// One entry per top-level declaration.
    #[prost(message, repeated, tag = "2")]
    pub declarations: Vec<ParsedDeclaration>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ParsedDeclaration {
    /// The schema tree key, e.g. "path:/users", "schema:User", "__metadata__"
    #[prost(string, tag = "1")]
    pub tree_key: String,

    /// The serialized blob bytes for this declaration.
    #[prost(bytes, tag = "2")]
    pub blob_bytes: Vec<u8>,
}
```

The core recognizes this envelope type by format_id and handles the multi-blob write path. This is an OpenAPI-specific carve-out to the "parse returns one blob" convention, justified by the fact that OpenAPI documents are inherently multi-declaration.

**Parse errors:** The parser returns `ParseError` for:
- Documents with `openapi:` version not starting with `3.`
- Missing required fields (`info.title`, `info.version`)
- Invalid `$ref` syntax
- Duplicate operation IDs across the document
- Path parameters declared in the path pattern but not in any parameter definition (and vice versa)

### 6.2 `print(blob: &Blob) -> Result<String, PrintError>`

`print` is called **per declaration blob**, not on the envelope. Each blob type renders to its canonical YAML fragment.

For assembling a complete OpenAPI document from a schema tree (e.g., for `UpdateSchema` source round-trip or `GetDescriptors`), the core calls `generate_descriptors` instead, which assembles the full document from all blobs in the schema tree.

The canonical YAML output from `print` is deterministic:
- Keys are sorted alphabetically within objects, except for the top-level OpenAPI structure which follows canonical OpenAPI key order (`openapi`, `info`, `servers`, `paths`, `components`).
- `properties` within schemas preserve the declaration order from the AST (BTreeMap → sorted, but the order stored in `Vec<PropertyDef>` is the original declaration order).
- Enum values preserve their original order.

### 6.3 `list_declarations(blob: &Blob) -> Result<Vec<DeclSummary>, ReadError>`

Since the OpenAPI plugin stores multiple blobs per schema file, `list_declarations` is called on the `OpenApiParseResult` envelope blob. It returns one `DeclSummary` per declaration:

```
DeclSummary { name: "path:/users",        kind: PathItem,           doc: "User management endpoints" }
DeclSummary { name: "path:/users/{id}",   kind: PathItem,           doc: None }
DeclSummary { name: "schema:User",        kind: ComponentSchema,    doc: "A registered user" }
DeclSummary { name: "schema:Error",       kind: ComponentSchema,    doc: "Standard error response" }
DeclSummary { name: "param:PageSize",     kind: ComponentParameter, doc: "Number of results per page" }
DeclSummary { name: "response:NotFound",  kind: ComponentResponse,  doc: "Resource not found" }
DeclSummary { name: "__metadata__",       kind: DocumentMetadata,   doc: "Payments API v2.1" }
```

The `kind` field uses the `DeclKind` enum defined in the core:

```rust
pub enum DeclKind {
    // Protobuf
    Message, Enum, Service,
    // FlatBuffers
    Table, Struct, FbsEnum, Union,
    // OpenAPI
    PathItem, ComponentSchema, ComponentParameter, ComponentResponse, ComponentRequestBody, DocumentMetadata,
}
```

### 6.4 `get_declaration(blob: &Blob, name: &str) -> Result<DeclDetail, ReadError>`

`name` is the schema tree key (`"path:/users"`, `"schema:User"`, etc.). The method locates the corresponding blob in the envelope, deserializes it, and returns a `DeclDetail` — a JSON or YAML rendering of the full declaration suitable for display in the CLI or agent context.

```
DeclDetail for "path:/users":
  PathItem "/users"
    GET  listUsers
      Parameters: limit (query, integer, optional), offset (query, integer, optional)
      Response 200: content: application/json → $ref User
    POST createUser
      RequestBody: required, application/json → $ref CreateUserRequest
      Response 201: content: application/json → $ref User
      Response 422: content: application/json → $ref Error
```

### 6.5 `imports(blob: &Blob) -> Result<Vec<Import>, ReadError>`

Scans all `SchemaOrRef::Ref` values in the envelope blob, collecting all `external_import` entries. Returns the deduplicated list. Called by the core during BFS transitive closure for `generate_descriptors`.

### 6.6 `diff(old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError>`

Compares two `OpenApiParseResult` envelope blobs. Produces `SchemaChange` entries:

- A declaration present in `new` but not `old` → `DeclarationAdded { name }`
- A declaration present in `old` but not `new` → `DeclarationRemoved { name }`
- A declaration present in both with differing blob hashes → `DeclarationModified { name, detail }`

The `detail` bytes for `DeclarationModified` contain a format-specific diff structure (e.g., which operations were added/removed, which parameters changed). The core does not interpret `detail`.

### 6.7 `check_compatibility(old, new, rules) -> Result<(), Vec<CompatibilityViolation>>`

Calls `diff` internally, then evaluates each `SchemaChange` against the compatibility table from `design.md` Section 4.4.

For `DeclarationModified` entries, the compatibility checker recursively inspects the old and new blobs to determine the nature of the modification:

**PathItem changes:**
- New operation added (new HTTP method): BACKWARD-compatible (new capability, old clients unaffected)
- Operation removed: FORWARD-compatible (old clients can still call it against old servers)
- Parameter added as `required: true`: FORWARD-compatible only (old clients that don't send it are broken against new server)
- Parameter added as `required: false`: FULL-compatible
- Parameter removed: BACKWARD-compatible (old clients that send it — server ignores unknown query params)
- Response field added as required: BACKWARD-compatible (old clients get extra fields; JSON is additive)
- Response field removed: FORWARD-compatible (old clients expect it; new server doesn't send it)
- Response field type changed: INCOMPATIBLE under all directions
- `operationId` changed: INCOMPATIBLE (generated client code uses operationId as function name)

**ComponentSchema changes:**
- Property added (not in `required`): BACKWARD-compatible
- Property added to `required`: FORWARD-compatible only
- Property removed: FORWARD-compatible
- Property type changed: INCOMPATIBLE
- Enum value added: BACKWARD-compatible (new servers can produce it; old clients won't understand it — actually FORWARD depending on direction)

See the full table in `design.md` Section 4.4.

### 6.8 `apply_mutation` and `apply_mutations` (v1)

In v1, OpenAPI mutations are whole-document pushes via `UpdateSchema`, not granular. The `apply_mutation` and `apply_mutations` methods on the OpenAPI plugin return `MutationError::UnsupportedInV1` for all inputs except the internal `PushDocument` mutation used by `UpdateSchema`. The `PushDocument` mutation carries the full source text and is handled by calling `parse` → `diff` → `check_compatibility` in sequence.

This is explicitly a temporary v1 design. The v2 granular mutation operations will match the path model from Section 3.

### 6.9 `generate_descriptors(blobs) -> Result<Bytes, DescriptorError>`

Assembles a complete, resolved OpenAPI 3.1 YAML document from the transitive closure of blobs:

1. Find the `__metadata__` blob → write `openapi:`, `info:`, `servers:` sections
2. Collect all `path:*` blobs → write `paths:` section
3. Collect all `schema:*`, `param:*`, `response:*`, `requestBody:*` blobs → write `components:` section
4. For external `$ref` imports in any blob: inline the referenced schema from the imported blob (since the BFS closure already includes it)

The output is a single self-contained YAML document with no unresolved `$ref` values pointing outside the document. Internal `$ref` values (local component references) are preserved as-is — they remain valid in the assembled document.

### 6.10 `generate_code` (v1)

Returns `CodegenError::UnsupportedLanguage` for all inputs. OpenAPI codegen (HTTP client/server generation) is deferred to v2.

---

## 7. v2 Granular Mutations (Design Intent)

The following operations are NOT implemented in v1 but define the mutation shape that the v1 AST is designed to support. Implemented in v2, they will produce blobs identical in structure to v1's `parse`.

```
AddOperation       { path: "path:/users", method: POST, operation: OperationDef }
RemoveOperation    { path: "path:/users", method: POST }
AddParameter       { path: "path:/users/GET", parameter: ParameterDef }
RemoveParameter    { path: "path:/users/GET/parameters/{limit}" }
UpdateParameter    { path: "path:/users/GET/parameters/{limit}", changes: ... }
AddResponseStatus  { path: "path:/users/GET", status_code: "404", response: ResponseDef }
RemoveResponseStatus { path: "path:/users/GET/responses/{404}" }
AddProperty        { path: "schema:User", property: PropertyDef }
RemoveProperty     { path: "schema:User/properties/{email}" }
MakeRequired       { path: "schema:User/properties/{email}" }
MakeOptional       { path: "schema:User/properties/{email}" }
AddPathItem        { path_pattern: "/orders", path_item: PathItemBlob }
RemovePathItem     { path_pattern: "/orders" }
```

Every operation is addressed by the stable path from Section 3. The AST types in Section 4 are designed to make all these operations expressible as field-level mutations on the blob structs.

---

## 8. Blob Version History

| `blob_version` | Change | Released |
|---------------|--------|---------|
| 1 | Initial v1 AST | v0.1.0 |

Migrations are defined in the `schemahub-openapi-plugin` crate following the migration chain model in `design.md` Section 3.5.
