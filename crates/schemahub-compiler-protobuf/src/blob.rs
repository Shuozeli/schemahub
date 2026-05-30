//! Versioned encode/decode for `DeclBlob` and `MetaBlob` (crate-structure.md §3.5).
//!
//! Each blob begins with a one-byte `BLOB_VERSION` followed by a one-byte
//! discriminant (`DeclKindTag`) and then the body produced by [`crate::codec`].
//! Prefixing the version lets us migrate the encoding without ambiguity.

use protoc_rs_schema::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileOptions,
    ServiceDescriptorProto, SourceCodeInfo,
};

use crate::codec::{Codec, Reader, Writer};

/// Bump when the on-disk blob layout changes incompatibly.
pub const BLOB_VERSION: u8 = 1;

// ─────────────────────────── DeclBlob payload ──────────────────────────────

/// The decoded payload of a `DeclBlob`: exactly one top-level declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum DeclPayload {
    Message(DescriptorProto),
    Enum(EnumDescriptorProto),
    Service(ServiceDescriptorProto),
}

/// One-byte discriminant for the decl kind.
mod tag {
    pub const MESSAGE: u8 = 1;
    pub const ENUM: u8 = 2;
    pub const SERVICE: u8 = 3;
}

#[derive(Debug)]
pub enum BlobError {
    UnsupportedVersion(u8),
    UnknownTag(u8),
    Malformed(String),
}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::UnsupportedVersion(v) => write!(f, "unsupported blob_version: {v}"),
            BlobError::UnknownTag(t) => write!(f, "unknown decl tag: {t}"),
            BlobError::Malformed(m) => write!(f, "malformed blob: {m}"),
        }
    }
}

impl std::error::Error for BlobError {}

/// Serialize one declaration to versioned bytes.
pub fn encode_decl(payload: &DeclPayload) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u8(BLOB_VERSION);
    match payload {
        DeclPayload::Message(m) => {
            w.write_u8(tag::MESSAGE);
            m.encode(&mut w);
        }
        DeclPayload::Enum(e) => {
            w.write_u8(tag::ENUM);
            e.encode(&mut w);
        }
        DeclPayload::Service(s) => {
            w.write_u8(tag::SERVICE);
            s.encode(&mut w);
        }
    }
    w.into_bytes()
}

/// Deserialize one declaration from versioned bytes.
pub fn decode_decl(bytes: &[u8]) -> Result<DeclPayload, BlobError> {
    let mut r = Reader::new(bytes);
    let version = r
        .read_u8()
        .map_err(|e| BlobError::Malformed(e.to_string()))?;
    if version != BLOB_VERSION {
        return Err(BlobError::UnsupportedVersion(version));
    }
    let tag = r
        .read_u8()
        .map_err(|e| BlobError::Malformed(e.to_string()))?;
    match tag {
        tag::MESSAGE => Ok(DeclPayload::Message(
            DescriptorProto::decode(&mut r).map_err(|e| BlobError::Malformed(e.to_string()))?,
        )),
        tag::ENUM => Ok(DeclPayload::Enum(
            EnumDescriptorProto::decode(&mut r).map_err(|e| BlobError::Malformed(e.to_string()))?,
        )),
        tag::SERVICE => Ok(DeclPayload::Service(
            ServiceDescriptorProto::decode(&mut r)
                .map_err(|e| BlobError::Malformed(e.to_string()))?,
        )),
        other => Err(BlobError::UnknownTag(other)),
    }
}

// ─────────────────────────── MetaBlob payload ──────────────────────────────

/// File-level metadata held by a `MetaBlob`: everything in a
/// `FileDescriptorProto` that is **not** a top-level message/enum/service
/// declaration (design.md §3.1 decl-split).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetaPayload {
    pub name: Option<String>,
    pub package: Option<String>,
    pub dependency: Vec<String>,
    pub public_dependency: Vec<i32>,
    pub weak_dependency: Vec<i32>,
    pub option_dependency: Vec<String>,
    pub options: Option<FileOptions>,
    pub syntax: Option<String>,
    pub edition: Option<String>,
    /// `source_code_info` for the whole file (declaration + file-level
    /// locations), kept here so the printer can recover comments.
    pub source_code_info: Option<SourceCodeInfo>,
    /// Original source order of top-level message names, used to re-map
    /// `source_code_info` paths after the decls are re-sorted by the VCS layer's
    /// `BTreeMap`.
    pub message_order: Vec<String>,
    /// Original source order of top-level enum names.
    pub enum_order: Vec<String>,
    /// Original source order of top-level service names.
    pub service_order: Vec<String>,
    /// File-level extension fields (proto2 `extend` blocks at file scope). These
    /// are not top-level message/enum/service declarations, so they live in the
    /// file meta rather than a `DeclBlob`.
    pub extension: Vec<FieldDescriptorProto>,
}

/// Serialize the file metadata to versioned bytes.
pub fn encode_meta(meta: &MetaPayload) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u8(BLOB_VERSION);
    w.write_opt_str(&meta.name);
    w.write_opt_str(&meta.package);
    write_str_vec(&mut w, &meta.dependency);
    write_i32_vec(&mut w, &meta.public_dependency);
    write_i32_vec(&mut w, &meta.weak_dependency);
    write_str_vec(&mut w, &meta.option_dependency);
    match &meta.options {
        Some(o) => {
            w.write_u8(1);
            o.encode(&mut w);
        }
        None => w.write_u8(0),
    }
    w.write_opt_str(&meta.syntax);
    w.write_opt_str(&meta.edition);
    match &meta.source_code_info {
        Some(s) => {
            w.write_u8(1);
            s.encode(&mut w);
        }
        None => w.write_u8(0),
    }
    write_str_vec(&mut w, &meta.message_order);
    write_str_vec(&mut w, &meta.enum_order);
    write_str_vec(&mut w, &meta.service_order);
    write_field_vec(&mut w, &meta.extension);
    w.into_bytes()
}

/// Deserialize the file metadata from versioned bytes.
pub fn decode_meta(bytes: &[u8]) -> Result<MetaPayload, BlobError> {
    // An empty MetaBlob (the `Default`) decodes to default metadata; this keeps
    // `SchemaObjects::default()` usable in tests and pass-throughs.
    if bytes.is_empty() {
        return Ok(MetaPayload::default());
    }
    let mut r = Reader::new(bytes);
    let version = r
        .read_u8()
        .map_err(|e| BlobError::Malformed(e.to_string()))?;
    if version != BLOB_VERSION {
        return Err(BlobError::UnsupportedVersion(version));
    }
    let m = (|| -> Result<MetaPayload, crate::codec::CodecError> {
        Ok(MetaPayload {
            name: r.read_opt_str()?,
            package: r.read_opt_str()?,
            dependency: read_str_vec(&mut r)?,
            public_dependency: read_i32_vec(&mut r)?,
            weak_dependency: read_i32_vec(&mut r)?,
            option_dependency: read_str_vec(&mut r)?,
            options: if r.read_u8()? == 1 {
                Some(FileOptions::decode(&mut r)?)
            } else {
                None
            },
            syntax: r.read_opt_str()?,
            edition: r.read_opt_str()?,
            source_code_info: if r.read_u8()? == 1 {
                Some(SourceCodeInfo::decode(&mut r)?)
            } else {
                None
            },
            message_order: read_str_vec(&mut r)?,
            enum_order: read_str_vec(&mut r)?,
            service_order: read_str_vec(&mut r)?,
            extension: read_field_vec(&mut r)?,
        })
    })()
    .map_err(|e| BlobError::Malformed(e.to_string()))?;
    Ok(m)
}

// Local helpers (the codec module keeps its vec helpers private).
fn write_str_vec(w: &mut Writer, items: &[String]) {
    w.write_uvarint(items.len() as u64);
    for it in items {
        w.write_str(it);
    }
}

fn read_str_vec(r: &mut Reader) -> Result<Vec<String>, crate::codec::CodecError> {
    let len = r.read_uvarint()? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(r.read_str()?);
    }
    Ok(out)
}

fn write_i32_vec(w: &mut Writer, items: &[i32]) {
    w.write_uvarint(items.len() as u64);
    for it in items {
        w.write_ivarint(*it as i64);
    }
}

fn read_i32_vec(r: &mut Reader) -> Result<Vec<i32>, crate::codec::CodecError> {
    let len = r.read_uvarint()? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(r.read_ivarint()? as i32);
    }
    Ok(out)
}

fn write_field_vec(w: &mut Writer, items: &[FieldDescriptorProto]) {
    w.write_uvarint(items.len() as u64);
    for it in items {
        it.encode(w);
    }
}

fn read_field_vec(r: &mut Reader) -> Result<Vec<FieldDescriptorProto>, crate::codec::CodecError> {
    let len = r.read_uvarint()? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(FieldDescriptorProto::decode(r)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protoc_rs_schema::{
        DescriptorProto, FieldDescriptorProto, FieldLabel, FieldType, FileOptions,
    };

    #[test]
    fn decl_blob_round_trips_a_message() {
        // Arrange
        let msg = DescriptorProto {
            name: Some("M".into()),
            field: vec![FieldDescriptorProto {
                name: Some("a".into()),
                number: Some(1),
                label: Some(FieldLabel::Repeated),
                r#type: Some(FieldType::Int64),
                json_name: Some("a".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let payload = DeclPayload::Message(msg.clone());

        // Act
        let bytes = encode_decl(&payload);
        let decoded = decode_decl(&bytes).expect("decode ok");

        // Assert
        assert_eq!(decoded, DeclPayload::Message(msg));
        assert_eq!(bytes[0], BLOB_VERSION);
    }

    #[test]
    fn meta_blob_round_trips() {
        // Arrange
        let meta = MetaPayload {
            package: Some("a.b".into()),
            dependency: vec!["x.proto".into()],
            options: Some(FileOptions {
                go_package: Some("ex/m".into()),
                ..Default::default()
            }),
            syntax: Some("proto3".into()),
            message_order: vec!["M".into()],
            ..Default::default()
        };

        // Act
        let bytes = encode_meta(&meta);
        let decoded = decode_meta(&bytes).expect("decode ok");

        // Assert
        assert_eq!(decoded, meta);
    }

    #[test]
    fn decode_rejects_bad_version() {
        // Arrange
        let bytes = vec![99u8, 1u8];

        // Act
        let result = decode_decl(&bytes);

        // Assert
        assert!(matches!(result, Err(BlobError::UnsupportedVersion(99))));
    }

    #[test]
    fn empty_meta_decodes_to_default() {
        // Arrange
        let bytes: Vec<u8> = Vec::new();

        // Act
        let decoded = decode_meta(&bytes).expect("empty meta ok");

        // Assert
        assert_eq!(decoded, MetaPayload::default());
    }
}
