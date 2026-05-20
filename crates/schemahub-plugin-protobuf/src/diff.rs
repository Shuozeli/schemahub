use bytes::Bytes;
use schemahub_types::errors::DiffError;

use crate::ast::{
    DeclBlob, EnumBlob, MessageBlob, ServiceBlob, KIND_ENUM, KIND_MESSAGE, KIND_SERVICE,
    decode_decl_blob,
};

/// Compare two DeclBlob-encoded declaration blobs.
/// Returns None if identical, Some(detail_bytes) if different.
/// The detail bytes are a JSON description of the changes.
pub fn diff_decl_blobs(old_bytes: &[u8], new_bytes: &[u8]) -> Result<Option<Bytes>, DiffError> {
    let old_decl = decode_decl_blob(old_bytes)
        .map_err(|e| DiffError::MalformedBlob(format!("old blob: {e}")))?;
    let new_decl = decode_decl_blob(new_bytes)
        .map_err(|e| DiffError::MalformedBlob(format!("new blob: {e}")))?;

    if old_decl.kind != new_decl.kind {
        let detail = serde_json::json!({
            "kind_changed": true,
            "old_kind": old_decl.kind,
            "new_kind": new_decl.kind,
        });
        return Ok(Some(Bytes::from(detail.to_string())));
    }

    match old_decl.kind {
        KIND_MESSAGE => diff_messages_inner(&old_decl, &new_decl),
        KIND_ENUM => diff_enums_inner(&old_decl, &new_decl),
        KIND_SERVICE => diff_services_inner(&old_decl, &new_decl),
        _ => {
            // metadata or unknown — byte-level comparison
            if old_decl.data != new_decl.data {
                let detail = serde_json::json!({"changed": true, "kind": old_decl.kind});
                Ok(Some(Bytes::from(detail.to_string())))
            } else {
                Ok(None)
            }
        }
    }
}

fn diff_messages_inner(old_decl: &DeclBlob, new_decl: &DeclBlob) -> Result<Option<Bytes>, DiffError> {
    let old = decode_message_or_err(&old_decl.data)?;
    let new = decode_message_or_err(&new_decl.data)?;
    diff_messages(&old, &new)
}

fn diff_enums_inner(old_decl: &DeclBlob, new_decl: &DeclBlob) -> Result<Option<Bytes>, DiffError> {
    let old = decode_enum_or_err(&old_decl.data)?;
    let new = decode_enum_or_err(&new_decl.data)?;
    diff_enums(&old, &new)
}

fn diff_services_inner(old_decl: &DeclBlob, new_decl: &DeclBlob) -> Result<Option<Bytes>, DiffError> {
    let old = decode_service_or_err(&old_decl.data)?;
    let new = decode_service_or_err(&new_decl.data)?;
    diff_services(&old, &new)
}

pub fn diff_messages(old: &MessageBlob, new: &MessageBlob) -> Result<Option<Bytes>, DiffError> {
    if old == new {
        return Ok(None);
    }

    let mut changes: Vec<serde_json::Value> = Vec::new();

    let old_by_name: std::collections::HashMap<&str, &crate::ast::FieldDef> =
        old.fields.iter().map(|f| (f.name.as_str(), f)).collect();
    let new_by_name: std::collections::HashMap<&str, &crate::ast::FieldDef> =
        new.fields.iter().map(|f| (f.name.as_str(), f)).collect();
    let old_by_num: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        old.fields.iter().map(|f| (f.number, f)).collect();
    let new_by_num: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        new.fields.iter().map(|f| (f.number, f)).collect();

    // Added fields
    for f in &new.fields {
        if !old_by_name.contains_key(f.name.as_str()) && !old_by_num.contains_key(&f.number) {
            changes.push(serde_json::json!({
                "change": "field_added",
                "field_name": f.name,
                "field_number": f.number,
                "field_type": f.field_type,
            }));
        }
    }

    // Removed fields
    for f in &old.fields {
        if !new_by_name.contains_key(f.name.as_str()) && !new_by_num.contains_key(&f.number) {
            changes.push(serde_json::json!({
                "change": "field_removed",
                "field_name": f.name,
                "field_number": f.number,
            }));
        }
    }

    // Modified fields
    for f in &new.fields {
        if let Some(old_f) = old_by_name.get(f.name.as_str()) {
            if old_f.field_type != f.field_type {
                changes.push(serde_json::json!({
                    "change": "field_type_changed",
                    "field_name": f.name,
                    "old_type": old_f.field_type,
                    "new_type": f.field_type,
                }));
            }
            if old_f.repeated != f.repeated {
                changes.push(serde_json::json!({
                    "change": "field_label_changed",
                    "field_name": f.name,
                    "old_repeated": old_f.repeated,
                    "new_repeated": f.repeated,
                }));
            }
            if old_f.number != f.number {
                changes.push(serde_json::json!({
                    "change": "field_renumbered",
                    "field_name": f.name,
                    "old_number": old_f.number,
                    "new_number": f.number,
                }));
            }
        } else if let Some(old_f) = old_by_num.get(&f.number) {
            if old_f.name != f.name {
                changes.push(serde_json::json!({
                    "change": "field_renamed",
                    "old_name": old_f.name,
                    "new_name": f.name,
                    "field_number": f.number,
                }));
            }
        }
    }

    // Check field order changed (if everything else is the same)
    if changes.is_empty() && old.fields != new.fields {
        changes.push(serde_json::json!({ "change": "fields_reordered" }));
    }

    if changes.is_empty() && old == new {
        return Ok(None);
    }

    if changes.is_empty() {
        // Something else changed (reserved sets, doc comment, etc.)
        changes.push(serde_json::json!({ "change": "other" }));
    }

    let detail = serde_json::json!({
        "declaration": old.name,
        "kind": "message",
        "changes": changes,
    });
    Ok(Some(Bytes::from(detail.to_string())))
}

pub fn diff_enums(old: &EnumBlob, new: &EnumBlob) -> Result<Option<Bytes>, DiffError> {
    if old == new {
        return Ok(None);
    }
    let detail = serde_json::json!({
        "declaration": old.name,
        "kind": "enum",
        "changed": true,
    });
    Ok(Some(Bytes::from(detail.to_string())))
}

pub fn diff_services(old: &ServiceBlob, new: &ServiceBlob) -> Result<Option<Bytes>, DiffError> {
    if old == new {
        return Ok(None);
    }
    let detail = serde_json::json!({
        "declaration": old.name,
        "kind": "service",
        "changed": true,
    });
    Ok(Some(Bytes::from(detail.to_string())))
}

// ── Decode helpers ─────────────────────────────────────────────────────────────

fn decode_message_or_err(data: &[u8]) -> Result<MessageBlob, DiffError> {
    use prost::Message;
    MessageBlob::decode(data)
        .map_err(|e| DiffError::MalformedBlob(format!("MessageBlob: {e}")))
}

fn decode_enum_or_err(data: &[u8]) -> Result<EnumBlob, DiffError> {
    use prost::Message;
    EnumBlob::decode(data)
        .map_err(|e| DiffError::MalformedBlob(format!("EnumBlob: {e}")))
}

fn decode_service_or_err(data: &[u8]) -> Result<ServiceBlob, DiffError> {
    use prost::Message;
    ServiceBlob::decode(data)
        .map_err(|e| DiffError::MalformedBlob(format!("ServiceBlob: {e}")))
}
