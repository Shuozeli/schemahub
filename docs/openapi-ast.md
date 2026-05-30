# schemahub — OpenAPI AST (v2: per-declaration, in-tree compiler)

> This document specifies the internal AST model for OpenAPI schemas in schemahub, **as implemented** in `crates/schemahub-compiler-openapi/`. OpenAPI has no sibling compiler (unlike Protobuf/FlatBuffers), so the AST, parser, and printer are in-tree (`design.md` §3.3, `crate-structure.md` §3.5).
>
> It supersedes the v1 spec (preserved in git history). Two things changed structurally in v2:
>
> 1. **No single envelope.** `parse` returns the `Compiler` trait's `ParsedSchema { meta, decls }` — one self-describing `DeclBlob` per top-level declaration, plus one document-level `MetaBlob`. There is no `OpenApiParseResult` wrapper and no `__metadata__` tree key.
> 2. **serde_json, not prost.** Each AST node is a plain `serde` Rust type, encoded with `serde_json` (`blob.rs`). The v1 `prost`/`#[prost(...)]` framing is gone.
>
> **Source-of-truth note.** Where this doc and `design.md` disagree, the code wins (it is the implementation). Divergences are flagged **AS-BUILT**.

---

## 1. Background and Constraints

### OpenAPI version support

Targets **OpenAPI 3.1.x**. The parser requires an `openapi:` field starting with `3.`; otherwise it returns `ParseError::UnsupportedVersion`. (`parser.rs::parse_openapi`.)

### What schemahub stores vs. what it does not

schemahub stores the **semantic content** of an OpenAPI document as a structured AST. It does NOT store:
- YAML/JSON formatting (indentation, key ordering, comment placement)
- `x-` extension fields (preserved as opaque JSON bytes in an `Extensions` struct, not interpreted)
- example values that don't affect schema validation

`print` produces canonical YAML from the AST; round-tripping through parse→print may change formatting but preserves semantic content.

### Why per-declaration

Per the v2 requirement (`requirements.md` §1), a schema is stored as **one object per top-level declaration** plus a file-level metadata object — not as one opaque blob. For OpenAPI this is realized by splitting the document at `parse` time into named `DeclBlob`s and one `MetaBlob`.

---

## 2. Declaration Granularity and the Per-Declaration Key Scheme

`parse` splits an OpenAPI document into:

| Declaration kind | One decl per… | Decl key (in `ParsedSchema.decls`) |
|------------------|---------------|------------------------------------|
| Path item | path pattern | `path:<pattern>` — e.g. `path:/users`, `path:/users/{id}` |
| Component schema | named `components/schemas` entry | `schema:<name>` — e.g. `schema:User` |
| Component parameter | named `components/parameters` entry | `param:<name>` — e.g. `param:PageSize` |
| Component response | named `components/responses` entry | `response:<name>` — e.g. `response:NotFound` |
| Component requestBody | named `components/requestBodies` entry | `requestBody:<name>` — e.g. `requestBody:CreateUserRequest` |
| Document metadata | the whole document (exactly one) | *(not a decl)* — stored as `ParsedSchema.meta`, a `MetaBlob` |

The kind-prefix on each key (`path:`, `schema:`, `param:`, `response:`, `requestBody:`) avoids collisions between, say, a path named `User` and a component schema named `User`. The VCS layer keys each `DeclBlob` by this string in the schema-file subtree. (`parser.rs`, lines that `push((format!("path:{path_str}"), …))` etc.)

> **AS-BUILT — metadata is the `MetaBlob`, not a `__metadata__` tree entry.** The v1 doc and `design.md` §4.2 describe a reserved tree key (`__metadata__` / `__meta__`) holding the document metadata. The implementation instead returns it as `ParsedSchema.meta` (a `DocumentMetadataBlob` encoded into a `schemahub_types::MetaBlob`). It is **not** one of the named `decls` and has no `path:`/`schema:`-style key. The `DECL_KIND_DOCUMENT_METADATA` enum value exists for summaries but no decl blob carries it.

### Inline schemas

Schemas defined inline (not under `components/schemas`) are stored **within their containing decl blob**, not extracted into separate top-level decls. A `$ref` to a component is stored symbolically (§5). Only `components/schemas` entries get their own `schema:<name>` blob.

---

## 3. Self-Describing Decl Blobs

Because each decl now stands alone in storage keyed only by its name string, the blob must carry its own **kind tag** — single-blob methods (`summarize_decl`, `decl_detail`, `diff_decl`, `validate_resolution`) must work without re-deriving the kind from a tree-key prefix. This is the v2 wrapper (`ast.rs`):

```rust
/// The current blob format version for every OpenAPI decl/meta blob.
pub const BLOB_VERSION: u32 = 1;

/// The self-describing payload of one DeclBlob.
pub struct OpenApiDecl {
    pub blob_version: u32,     // rides on the wrapper, for migration
    pub kind: DeclPayload,     // kind-tagged body
}

pub enum DeclPayload {
    PathItem(PathItemBlob),
    ComponentSchema(ComponentSchemaBlob),
    ComponentParameter(ComponentParameterBlob),
    ComponentResponse(ComponentResponseBlob),
    ComponentRequestBody(ComponentRequestBodyBlob),
}
```

`OpenApiDecl::new(payload)` stamps `blob_version = BLOB_VERSION`.

> **AS-BUILT — where `blob_version` lives.** v1 put `blob_version` as field 1 of *every* blob struct (`PathItemBlob`, `ComponentSchemaBlob`, …). In v2 the per-blob `blob_version` fields are **gone**; the version rides once on the `OpenApiDecl` wrapper (decls) and once on `DocumentMetadataBlob` (meta). The inner blob structs (`PathItemBlob` etc.) no longer carry it.

---

## 4. Blob Encoding (`blob.rs`)

**Encoding: `serde_json`.** Rationale (from `blob.rs`): the in-tree AST is small and human-debuggable, `serde_json` is already a dependency (no `prost` build step), and it is deterministic — serde serializes struct fields in declaration order and `serde_json` does not reorder, so identical ASTs produce identical bytes.

- `encode_decl(&OpenApiDecl) -> DeclBlob` / `decode_decl(&DeclBlob) -> Result<OpenApiDecl, BlobError>`
- `encode_meta(&DocumentMetadataBlob) -> MetaBlob` / `decode_meta(&MetaBlob) -> Result<DocumentMetadataBlob, BlobError>`

`decode_*` reject any blob whose `blob_version > BLOB_VERSION`. An empty `MetaBlob` (the default before any parse) decodes to `DocumentMetadataBlob::default()` (treated as "no metadata yet"). `BlobError` converts into `ReadError::MalformedBlob` / `PrintError::MalformedBlob` / `DiffError::MalformedBlob`.

> **AS-BUILT — encoding changed from prost to serde_json.** The v1 doc said "All types below are serialized to bytes via `prost`." That is no longer true. The AST types derive `serde::{Serialize, Deserialize}` and are encoded with `serde_json`. There are no `#[prost(...)]` tags, no prost enum-discriminant integers, and no `.proto` for the AST. This also matches `design.md` §2.1's "`prost`/`serde` … whichever the sibling crate supports" — here, serde.

---

## 5. Rust AST Type Definitions (`ast.rs`)

All types derive `Clone, Debug, PartialEq, Serialize, Deserialize` (most also `Default`). Optional/empty fields use `#[serde(default, skip_serializing_if = …)]` so omitted values don't bloat the JSON.

### 5.1 Enums (plain Rust enums)

`HttpMethod` (`Get`/`Post`/`Put`/`Delete`/`Patch`/`Head`/`Options`/`Trace`), `ParameterLocation` (`Query`/`Header`/`Path`/`Cookie`), `JsonSchemaType` (`String`/`Integer`/`Number`/`Boolean`/`Array`/`Object`/`Null`). Each has `from_str` / `to_str` helpers. These serialize as their variant names (not prost integer discriminants).

### 5.2 Shared primitives

```rust
pub struct SchemaRef {            // a $ref target
    pub local_name: String,                       // component name for a local #/components/schemas/<name>
    pub external_import: Option<ExternalImport>,  // populated for cross-file refs (v2-modeled)
}
pub struct ExternalImport { pub path: String, pub resolved_commit: String, pub decl_name: String }
pub struct Extensions { pub json_bytes: Vec<u8> }  // raw JSON of x- fields, uninterpreted
```

> **AS-BUILT** — the v1 type was `Import`; in the OpenAPI AST it is named `ExternalImport`, and the schemahub-wide `Import { path, resolved_commit }` lives in `schemahub-types`. External imports are modeled in the AST but, in v1, are not surfaced as document-level imports (see §6.5).

### 5.3 JSON Schema (`JsonSchemaDef`)

The core recursive type. Field set is unchanged from v1 in *content* (types, format, string/numeric/array/object constraints, `allOf`/`anyOf`/`oneOf`/`not`, `enum`/`const`, metadata, extensions) — only the encoding and `Option`/`Vec` skip attributes changed. `properties: Vec<PropertyDef>` preserves declaration order; `items`/`additional_properties_schema`/`not` are `Option<Box<SchemaOrRef>>`. `enum_values` and `const_value`/`default` are JSON-encoded strings.

```rust
pub enum SchemaOrRef { Inline(JsonSchemaDef), Ref(SchemaRef) }   // default = Inline(default)
pub struct PropertyDef { pub name: String, pub schema: Option<SchemaOrRef> }
```

> **AS-BUILT** — `SchemaOrRef` (and the other `*OrRef` types below) are now ordinary Rust `enum`s, not the v1 prost `oneof` helper modules (`schema_or_ref::Value`, etc.).

### 5.4 Parameter / RequestBody / Response / Operation

- `ParameterDef { name, location, description?, required, deprecated?, schema?, extensions? }`
- `ParameterOrRef = Inline(ParameterDef) | Ref(String)` (Ref is the component name)
- `RequestBodyDef { description?, required, content: Vec<MediaTypeEntry>, extensions? }`; `MediaTypeEntry { media_type, schema?, extensions? }`; `RequestBodyOrRef = Inline | Ref(String)`
- `ResponseDef { description, content, headers: Vec<HeaderDef>, extensions? }`; `HeaderDef { name, description?, required?, schema? }`; `ResponseOrRef = Inline | Ref(String)`
- `OperationDef { method, operation_id?, summary?, description?, tags, parameters: Vec<ParameterOrRef>, request_body?: RequestBodyOrRef, responses: Vec<ResponseEntry>, deprecated?, extensions? }`; `ResponseEntry { status_code: String, response?: ResponseOrRef }`. `OperationDef::empty(method)` builds a bare op.

### 5.5 Blob types (the stored payloads)

```rust
pub struct DocumentMetadataBlob {     // → MetaBlob (NOT a decl)
    pub blob_version: u32,
    pub openapi_version: String,      // "3.1.0"
    pub info: Option<InfoObject>,     // { title, description?, version, terms_of_service? }
    pub servers: Vec<ServerObject>,   // { url, description? }
    pub extensions: Option<Extensions>,
}

pub struct PathItemBlob          { path_pattern, summary?, description?, parameters: Vec<ParameterOrRef>, operations: Vec<OperationDef>, extensions? }
pub struct ComponentSchemaBlob   { name, schema: Option<JsonSchemaDef>, extensions? }
pub struct ComponentParameterBlob{ name, parameter: Option<ParameterDef> }
pub struct ComponentResponseBlob { name, response: Option<ResponseDef> }
pub struct ComponentRequestBodyBlob { name, request_body: Option<RequestBodyDef> }
```

Each `*Blob` (except `DocumentMetadataBlob`) is wrapped in `OpenApiDecl { blob_version, kind: DeclPayload::<Kind>(blob) }` before encoding (§3). `path_pattern` / `name` stay inside the blob so it is self-describing after lookup.

---

## 6. `Compiler` Method Behaviors for OpenAPI (`lib.rs`)

The OpenAPI compiler implements `schemahub_types::Compiler`. Methods take/return the trait's per-declaration types (`ParsedSchema`, `SchemaObjects`, `DeclBlob`, `MetaBlob`, `MutationEffect`, …) — not a single envelope.

### 6.1 `parse(&self, source) -> Result<ParsedSchema, ParseError>`

Parses YAML/JSON (JSON is a YAML subset) via `serde_yaml`. Builds the document `DocumentMetadataBlob` (→ `meta`) and one `(key, DeclBlob)` per path item and per component (§2). Parse errors: non-`3.` version (`UnsupportedVersion`), missing `info.title` / `info.version`, non-mapping root.

> **AS-BUILT — parse returns `ParsedSchema`, not a blob/envelope.** The v1 `OpenApiParseResult` "root envelope" (with `ParsedDeclaration { tree_key, blob_bytes }`) and the "OpenAPI-specific carve-out to the parse-returns-one-blob convention" are **gone**. `parse` returns `ParsedSchema { meta: MetaBlob, decls: Vec<(String, DeclBlob)> }` directly, exactly like the other compilers — the core's per-declaration write path needs no special case.

### 6.2 `print(&self, schema: &SchemaObjects) -> Result<String, PrintError>` (`printer.rs`)

Reassembles the whole schema file from `SchemaObjects` (a `MetaBlob` + `BTreeMap<key, DeclBlob>`) into canonical OpenAPI 3.1 YAML. `BTreeMap` iteration is key-sorted, so the `path:`/`schema:`/`param:`/`response:`/`requestBody:` prefixes keep kinds grouped and sorted within each kind. Top-level document key order follows OpenAPI structure (`openapi`, `info`, `servers`, `paths`, `components`); within a decl, declaration order (e.g. `properties`) is preserved.

> **AS-BUILT — `print` operates on the whole `SchemaObjects`, not per-blob.** v1 said `print` is called per decl blob and a separate `generate_descriptors` reassembles the document. In v2, `print(SchemaObjects)` *is* the reassembly path; `decl_detail` handles single-decl rendering for display (§6.3).

### 6.3 Read / exploration

- `summarize_decl(&DeclBlob) -> DeclSummary` — decodes the `OpenApiDecl`, returns `{ name, kind, doc_comment }` with `name` re-prefixed (`path:<pattern>`, `schema:<name>`, …) and `kind` the matching `DeclKind`.
- `decl_detail(&DeclBlob) -> DeclDetail` — human/agent-readable rendering of one declaration (`print_decl_detail`).
- `imports(&MetaBlob) -> Vec<Import>` — **AS-BUILT: returns empty.** External cross-file `$ref` imports are modeled in the AST (`SchemaRef.external_import`) but live inside decl blobs, not the meta blob; the trait scopes `imports` to the meta blob, and OpenAPI v1 has no document-level imports.
- `type_refs(&DeclBlob) -> Vec<TypeRef>` — collects the local component refs a decl references, deduplicated, as `schema:<name>` / `param:<name>` / `response:<name>` / `requestBody:<name>` keys (used for `FollowType` and the dependency index).

### 6.4 `diff_decl` / `check_compatibility`

`diff_decl(old, new) -> DeclChange` (`diff.rs`) and `check_compatibility(old, new, rules) -> Result<(), Vec<CompatibilityViolation>>` (`compat.rs`) operate on a **pair of single decl blobs**, not on envelopes.

> **AS-BUILT — diff/compat are per-declaration, not whole-document.** v1 described `diff`/`check_compatibility` comparing two `OpenApiParseResult` envelopes and producing add/remove/modify across the document. In v2 the VCS layer already knows which decls changed (per-declaration tree), so these methods compare one decl against its counterpart; whole-document add/remove falls out of the tree diff at the core layer. The compatibility rule intent (operation/parameter/response/property/enum rules) is unchanged.

### 6.5 `$ref` handling

- **Local** `#/components/schemas/User` → `SchemaOrRef::Ref(SchemaRef { local_name: "User", external_import: None })`; the `ComponentSchemaBlob` for `User` is a separate `schema:User` decl in the same file. Refs stay symbolic. Parameter/response/requestBody refs strip their `#/components/<kind>/` prefix to the bare component name.
- **External** (cross-file) → `SchemaRef { local_name: "", external_import: Some(ExternalImport { path, resolved_commit, decl_name }) }`. Modeled but v2-resolved (not surfaced via `imports`).

### 6.6 Mutations (`operations.rs`, `lib.rs::apply_one`)

`apply_mutation(schema, op)` decodes an `OpenApiOp` and applies it; `apply_mutations(schema, ops)` folds an ordered batch over a working copy and returns the **net** `MutationEffect` (only the final state validated).

> **AS-BUILT — granular OpenAPI mutations are partially implemented, not deferred-entirely.** v1 said the only OpenAPI mutation is the internal whole-document `PushDocument`, with all granular ops returning `UnsupportedInV1`. The implementation ships these granular ops in addition to `PushDocument`:
>
> | Op | Effect |
> |----|--------|
> | `PushDocument { source }` | whole-document replace: re-parse, upsert all decls + meta, remove dropped decls (used by `UpdateSchema`) |
> | `AddPath { path_pattern, summary, description }` | new empty `path:<pattern>` decl (errors if it exists) |
> | `RemovePath { path_pattern }` | remove the `path:<pattern>` decl |
> | `AddOperation { path_pattern, method, operation_id, summary, description }` | add one HTTP method to a path item |
> | `RemoveOperation { path_pattern, method }` | remove one HTTP method from a path item |
> | `AddComponentSchema { schema_name, schema_type, description }` | new `schema:<name>` decl |
> | `RemoveComponentSchema { schema_name }` | remove the `schema:<name>` decl |
>
> Any other granular op returns `MutationError::UnsupportedInV1`. These map to the `OpenApiMutation` oneof in `mutations.proto` (see `grpc-api.md` §4.3) and are reachable via `ApplyMutation` but **not** `ApplyTransaction`.

### 6.7 Conflicts

- `render_conflict(&ConflictSides) -> String` — renders `base` (if present) and each competing side as YAML fragments (`# ===== base =====` / `# ===== side N =====`); `EmptyConflict` error if no sides.
- `validate_resolution(&DeclBlob) -> Result<(), ConflictError>` — a valid resolution must `decode_decl` to a well-formed `OpenApiDecl`; otherwise `InvalidResolution`.

### 6.8 Codegen

- `generate_descriptors(&SchemaClosure) -> Bytes` — assembles each schema file's reconstructed YAML via `print_schema_objects`; multiple files in a closure are concatenated as a YAML multi-document stream (`---` separators), sorted by schema name. Local `$ref`s stay symbolic (valid within each document).
- `generate_code(_, lang) -> Result<String, CodegenError>` — returns `CodegenError::UnsupportedLanguage(lang)`. OpenAPI client/server codegen is out of scope (v2).

---

## 7. Blob Version History

| `blob_version` | Change | Notes |
|---------------|--------|-------|
| 1 | Initial v2 in-tree AST | `BLOB_VERSION = 1`; serde_json encoding; self-describing `OpenApiDecl` wrapper; `DocumentMetadataBlob` as the `MetaBlob` |

`decode_decl` / `decode_meta` reject `blob_version` greater than `BLOB_VERSION`. Future migrations live in `schemahub-compiler-openapi`.

---

## 8. Relationship to the v1 Spec (what changed)

| v1 (prost / single-envelope) | v2 (as implemented) |
|------------------------------|---------------------|
| `parse` returns an `OpenApiParseResult` envelope blob; core unwraps it | `parse` returns `ParsedSchema { meta, decls }` directly |
| Document metadata stored under reserved tree key `__metadata__` | Document metadata is `ParsedSchema.meta` — a `DocumentMetadataBlob` `MetaBlob`, not a keyed decl |
| Every blob is a `prost::Message` with `blob_version` as field 1; `oneof` helper modules | serde types encoded with `serde_json`; `blob_version` only on `OpenApiDecl` + `DocumentMetadataBlob`; plain Rust enums |
| Tree-key prefix tells you the decl kind | each `DeclBlob` self-describes via `OpenApiDecl.kind` (kind tag) |
| `print` per-blob; `generate_descriptors` reassembles | `print(SchemaObjects)` reassembles; `decl_detail` renders one decl |
| `diff`/`check_compatibility` over whole-document envelopes | `diff_decl`/`check_compatibility` over single decl-blob pairs |
| OpenAPI mutations: whole-document `PushDocument` only | `PushDocument` **plus** six granular ops (add/remove path, operation, component schema) |

The path/key scheme itself (`path:`, `schema:`, `param:`, `response:`, `requestBody:`) is unchanged from v1 — that part of the design carried over intact.
