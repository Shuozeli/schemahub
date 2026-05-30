//! Stable, versioned (de)serialization of FlatBuffers AST nodes into the
//! opaque [`DeclBlob`]/[`MetaBlob`] bytes the VCS layer stores (design.md §2.1).
//!
//! `flatc-rs-schema` types implement `serde::Serialize`/`Deserialize` but NOT
//! `prost::Message`, so the blob encoding is `serde_json` over those types,
//! wrapped with a `blob_version: u32` prefix for migration. Each top-level
//! declaration (`Object`, `Enum`, `Service`) is one [`DeclBlob`]; file-level
//! metadata is one [`MetaBlob`].

use serde::{Deserialize, Serialize};

use flatc_rs_schema::{Enum, Object, Service};
use schemahub_types::{DeclBlob, MetaBlob};

/// Current blob format version. Bump on any incompatible encoding change.
pub const BLOB_VERSION: u32 = 1;

/// Array-typed AST fields that `flatc-rs-schema` serializes with
/// `skip_serializing_if = "Vec::is_empty"` but *without* `#[serde(default)]`.
///
/// On serialize they vanish when empty; on deserialize they are then required,
/// which breaks the round-trip (e.g. an `Object` with no fields, or the stub
/// request/response `Object`s on an `RpcCall`). We cannot edit the upstream
/// types, so before deserializing we re-inject these keys as `[]` wherever a
/// JSON object is missing them. The names are array-only across the entire AST,
/// so this injection is unambiguous.
const SKIPPED_ARRAY_KEYS: &[&str] = &[
    "objects",
    "enums",
    "services",
    "fbs_files",
    "fields",
    "calls",
    "values",
    "entries",
    "included_filenames",
];

/// Recursively ensure that any of [`SKIPPED_ARRAY_KEYS`] present anywhere in the
/// JSON tree default to an empty array when omitted. This makes the upstream
/// AST types deserializable even when an empty collection was skipped.
fn normalize_skipped_arrays(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for child in map.values_mut() {
                normalize_skipped_arrays(child);
            }
            for key in SKIPPED_ARRAY_KEYS {
                map.entry(*key)
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                normalize_skipped_arrays(item);
            }
        }
        _ => {}
    }
}

/// The discriminated payload carried by one [`DeclBlob`].
///
/// FlatBuffers tables and structs are both `Object` (distinguished by
/// `Object::is_struct`); enums and unions are both `Enum` (distinguished by
/// `Enum::is_union`). `Service` is its own AST node.
#[derive(Clone, Debug, PartialEq)]
pub enum DeclPayload {
    Object(Box<Object>),
    Enum(Box<Enum>),
    Service(Box<Service>),
}

impl DeclPayload {
    /// The declaration's name (used as the VCS tree key).
    pub fn name(&self) -> Option<&str> {
        match self {
            DeclPayload::Object(o) => o.name.as_deref(),
            DeclPayload::Enum(e) => e.name.as_deref(),
            DeclPayload::Service(s) => s.name.as_deref(),
        }
    }
}

/// Discriminant for the declaration kind stored in the envelope.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum DeclKindTag {
    Object,
    Enum,
    Service,
}

/// A versioned envelope wrapping one declaration node.
///
/// A fixed-key wrapper (`blob_version`, `kind`, `object`/`enum_`/`service`) is
/// used instead of an externally-tagged enum so the skipped-array normalizer
/// (see [`normalize_skipped_arrays`]) can re-inject defaults into the nested
/// node without disturbing the wrapper's structure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct DeclEnvelope {
    blob_version: u32,
    kind: DeclKindTag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    object: Option<Box<Object>>,
    #[serde(default, rename = "enum_", skip_serializing_if = "Option::is_none")]
    enum_: Option<Box<Enum>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service: Option<Box<Service>>,
}

impl DeclEnvelope {
    fn from_payload(payload: DeclPayload) -> Self {
        let mut env = DeclEnvelope {
            blob_version: BLOB_VERSION,
            kind: DeclKindTag::Object,
            object: None,
            enum_: None,
            service: None,
        };
        match payload {
            DeclPayload::Object(o) => {
                env.kind = DeclKindTag::Object;
                env.object = Some(o);
            }
            DeclPayload::Enum(e) => {
                env.kind = DeclKindTag::Enum;
                env.enum_ = Some(e);
            }
            DeclPayload::Service(s) => {
                env.kind = DeclKindTag::Service;
                env.service = Some(s);
            }
        }
        env
    }

    fn into_payload(self) -> Result<DeclPayload, BlobError> {
        match self.kind {
            DeclKindTag::Object => self
                .object
                .map(DeclPayload::Object)
                .ok_or_else(|| BlobError::Decode("envelope missing 'object' node".into())),
            DeclKindTag::Enum => self
                .enum_
                .map(DeclPayload::Enum)
                .ok_or_else(|| BlobError::Decode("envelope missing 'enum_' node".into())),
            DeclKindTag::Service => self
                .service
                .map(DeclPayload::Service)
                .ok_or_else(|| BlobError::Decode("envelope missing 'service' node".into())),
        }
    }
}

/// File-level metadata: namespace context, includes, root-type designation,
/// `file_identifier`, and `file_extension` (design.md §3.2).
///
/// The parser keeps `root_type` in side-channel `ParserState`, not in `Schema`,
/// so the compiler captures it here explicitly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FbsMeta {
    pub blob_version: u32,
    /// The active `namespace` for the file (e.g. `MyGame.Sample`), if any.
    pub namespace: Option<String>,
    /// `include "..."` filenames, in source order.
    pub includes: Vec<String>,
    /// The `root_type` designation, if present.
    pub root_type: Option<String>,
    /// `file_identifier "ABCD"`, if present.
    pub file_ident: Option<String>,
    /// `file_extension "ext"`, if present.
    pub file_ext: Option<String>,
    /// Top-level declaration names in original SOURCE order (tables, structs,
    /// enums, unions, services interleaved). `SchemaObjects::decls` is a
    /// `BTreeMap` (alphabetical) and the parser groups objects/enums/services
    /// into separate vectors, both of which lose source order; this list lets
    /// the printer restore it. Empty for blobs written before this field
    /// existed (the printer then falls back to BTreeMap order).
    #[serde(default)]
    pub decl_order: Vec<String>,
    /// File-level `attribute "name";` declarations, in source order. These
    /// register custom attribute names so their *usages* on fields/decls are
    /// not flagged as unknown. Empty for blobs written before this field
    /// existed.
    #[serde(default)]
    pub declared_attributes: Vec<String>,
}

impl FbsMeta {
    pub fn new() -> Self {
        Self {
            blob_version: BLOB_VERSION,
            ..Default::default()
        }
    }
}

/// Errors raised while (de)serializing a blob.
#[derive(Debug)]
pub enum BlobError {
    /// The bytes did not deserialize as the expected type.
    Decode(String),
    /// A recognized format but an unsupported `blob_version`.
    UnsupportedVersion(u32),
}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::Decode(m) => write!(f, "decode error: {m}"),
            BlobError::UnsupportedVersion(v) => write!(f, "unsupported blob_version: {v}"),
        }
    }
}

// ── Declaration blobs ───────────────────────────────────────────────────────

/// Encode one declaration payload into a versioned [`DeclBlob`].
pub fn encode_decl(payload: DeclPayload) -> DeclBlob {
    let env = DeclEnvelope::from_payload(payload);
    // serde_json over local types never fails to serialize.
    let bytes = serde_json::to_vec(&env).expect("DeclEnvelope serializes");
    DeclBlob::new(bytes)
}

/// Decode a [`DeclBlob`] back into its declaration payload.
pub fn decode_decl(blob: &DeclBlob) -> Result<DeclPayload, BlobError> {
    let mut value: serde_json::Value =
        serde_json::from_slice(blob.as_bytes()).map_err(|e| BlobError::Decode(e.to_string()))?;
    // Reject an unsupported version before re-injecting skipped arrays so the
    // version check is independent of the payload's structural validity.
    if let Some(v) = value.get("blob_version").and_then(|v| v.as_u64()) {
        if v != BLOB_VERSION as u64 {
            return Err(BlobError::UnsupportedVersion(v as u32));
        }
    }
    normalize_skipped_arrays(&mut value);
    let env: DeclEnvelope =
        serde_json::from_value(value).map_err(|e| BlobError::Decode(e.to_string()))?;
    env.into_payload()
}

// ── Meta blob ───────────────────────────────────────────────────────────────

/// Encode file metadata into a versioned [`MetaBlob`].
pub fn encode_meta(mut meta: FbsMeta) -> MetaBlob {
    meta.blob_version = BLOB_VERSION;
    let bytes = serde_json::to_vec(&meta).expect("FbsMeta serializes");
    MetaBlob::new(bytes)
}

/// Decode a [`MetaBlob`] back into [`FbsMeta`]. An empty blob yields a default
/// (empty) meta, so freshly-created schemas without metadata round-trip.
pub fn decode_meta(blob: &MetaBlob) -> Result<FbsMeta, BlobError> {
    if blob.is_empty() {
        return Ok(FbsMeta::new());
    }
    let meta: FbsMeta =
        serde_json::from_slice(blob.as_bytes()).map_err(|e| BlobError::Decode(e.to_string()))?;
    if meta.blob_version != BLOB_VERSION {
        return Err(BlobError::UnsupportedVersion(meta.blob_version));
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flatc_rs_schema::Object;

    #[test]
    fn decl_blob_round_trips_an_object() {
        // Arrange
        let obj = Object {
            name: Some("Order".to_string()),
            ..Default::default()
        };
        let payload = DeclPayload::Object(Box::new(obj.clone()));

        // Act
        let blob = encode_decl(payload);
        let decoded = decode_decl(&blob).expect("decodes");

        // Assert
        match decoded {
            DeclPayload::Object(o) => assert_eq!(o.name.as_deref(), Some("Order")),
            other => panic!("expected Object, got {other:?}"),
        }
    }

    #[test]
    fn meta_blob_round_trips() {
        // Arrange
        let meta = FbsMeta {
            blob_version: BLOB_VERSION,
            namespace: Some("MyGame.Sample".to_string()),
            includes: vec!["other.fbs".to_string()],
            root_type: Some("Monster".to_string()),
            file_ident: Some("MONS".to_string()),
            file_ext: Some("mon".to_string()),
            decl_order: vec!["Monster".to_string()],
            declared_attributes: vec!["priority".to_string()],
        };

        // Act
        let blob = encode_meta(meta.clone());
        let decoded = decode_meta(&blob).expect("decodes");

        // Assert
        assert_eq!(decoded, meta);
    }

    #[test]
    fn empty_meta_blob_decodes_to_default() {
        // Arrange
        let blob = MetaBlob::default();

        // Act
        let decoded = decode_meta(&blob).expect("decodes");

        // Assert
        assert_eq!(decoded.namespace, None);
        assert!(decoded.includes.is_empty());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        // Arrange: hand-craft an envelope with a bad version.
        let bad = serde_json::json!({
            "blob_version": 9999,
            "kind": "Object",
            "object": { "name": "X" }
        });
        let blob = DeclBlob::new(serde_json::to_vec(&bad).unwrap());

        // Act
        let result = decode_decl(&blob);

        // Assert
        assert!(matches!(result, Err(BlobError::UnsupportedVersion(9999))));
    }
}
