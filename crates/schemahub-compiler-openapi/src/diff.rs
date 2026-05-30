//! Per-declaration diff.
//!
//! The trait's `diff_decl(old, new)` is called by the VCS layer only for a
//! declaration present in *both* sides (adds/removes are detected by the VCS
//! from name presence). It therefore always yields a `DeclarationModified` when
//! the bytes differ. The decl name is recovered from the blob payload (a path
//! item carries its `path_pattern`, components carry their `name`) so the result
//! is self-describing.

use bytes::Bytes;
use schemahub_types::blob::DeclBlob;
use schemahub_types::change::DeclChange;
use schemahub_types::errors::DiffError;

use crate::ast::DeclPayload;
use crate::blob::decode_decl;

/// The stable tree-key name of a decl payload (matches the parser's keys).
pub fn decl_key(payload: &DeclPayload) -> String {
    match payload {
        DeclPayload::PathItem(b) => format!("path:{}", b.path_pattern),
        DeclPayload::ComponentSchema(b) => format!("schema:{}", b.name),
        DeclPayload::ComponentParameter(b) => format!("param:{}", b.name),
        DeclPayload::ComponentResponse(b) => format!("response:{}", b.name),
        DeclPayload::ComponentRequestBody(b) => format!("requestBody:{}", b.name),
    }
}

/// Diff two versions of the same declaration.
pub fn diff_decl(old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError> {
    let old_decl = decode_decl(old)?;
    let new_decl = decode_decl(new)?;

    let name = decl_key(&new_decl.kind);

    // Byte-equal blobs => no change. We still return a Modified entry per the
    // trait contract (the VCS layer only calls this when it believes a change
    // exists); the detail describes whether content actually differs.
    let detail: Bytes = if old.as_bytes() == new.as_bytes() {
        Bytes::from_static(b"no content change")
    } else {
        Bytes::from(describe_change(&old_decl.kind, &new_decl.kind))
    };

    Ok(DeclChange::DeclarationModified { name, detail })
}

/// A short human-readable summary of what changed between two payloads.
fn describe_change(old: &DeclPayload, new: &DeclPayload) -> Vec<u8> {
    let summary = match (old, new) {
        (DeclPayload::PathItem(o), DeclPayload::PathItem(n)) => {
            let old_methods: Vec<&str> = o.operations.iter().map(|op| op.method.to_str()).collect();
            let new_methods: Vec<&str> = n.operations.iter().map(|op| op.method.to_str()).collect();
            format!("path item modified; methods {old_methods:?} -> {new_methods:?}")
        }
        (DeclPayload::ComponentSchema(o), DeclPayload::ComponentSchema(n)) => {
            let old_props = o
                .schema
                .as_ref()
                .map(|s| s.properties.len())
                .unwrap_or(0);
            let new_props = n
                .schema
                .as_ref()
                .map(|s| s.properties.len())
                .unwrap_or(0);
            format!("schema modified; properties {old_props} -> {new_props}")
        }
        _ => "declaration content changed".to_string(),
    };
    summary.into_bytes()
}
