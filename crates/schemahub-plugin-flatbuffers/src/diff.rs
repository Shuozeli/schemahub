use schemahub_types::errors::DiffError;

use crate::ast::{
    DeclBlob, EnumBlob, TableBlob, KIND_ENUM, KIND_METADATA, KIND_STRUCT, KIND_TABLE, KIND_UNION,
    decode_decl_blob,
};

/// Compare two DeclBlob-encoded declaration blobs.
/// Returns a JSON-encoded list of changes, or None if identical.
pub fn diff_decl_blobs(old_bytes: &[u8], new_bytes: &[u8]) -> Result<Option<Vec<u8>>, DiffError> {
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
        return Ok(Some(detail.to_string().into_bytes()));
    }

    match old_decl.kind {
        KIND_TABLE => diff_tables_inner(&old_decl, &new_decl),
        KIND_ENUM => diff_enums_inner(&old_decl, &new_decl),
        KIND_STRUCT | KIND_UNION => {
            if old_decl.data != new_decl.data {
                let detail = serde_json::json!({ "changed": true, "kind": old_decl.kind });
                Ok(Some(detail.to_string().into_bytes()))
            } else {
                Ok(None)
            }
        }
        KIND_METADATA => {
            if old_decl.data != new_decl.data {
                let detail = serde_json::json!({ "changed": true, "kind": "metadata" });
                Ok(Some(detail.to_string().into_bytes()))
            } else {
                Ok(None)
            }
        }
        _ => {
            if old_decl.data != new_decl.data {
                let detail = serde_json::json!({ "changed": true });
                Ok(Some(detail.to_string().into_bytes()))
            } else {
                Ok(None)
            }
        }
    }
}

fn diff_tables_inner(old_decl: &DeclBlob, new_decl: &DeclBlob) -> Result<Option<Vec<u8>>, DiffError> {
    use prost::Message;
    let old = TableBlob::decode(old_decl.data.as_slice())
        .map_err(|e| DiffError::MalformedBlob(format!("TableBlob: {e}")))?;
    let new = TableBlob::decode(new_decl.data.as_slice())
        .map_err(|e| DiffError::MalformedBlob(format!("TableBlob: {e}")))?;
    diff_tables(&old, &new)
}

fn diff_enums_inner(old_decl: &DeclBlob, new_decl: &DeclBlob) -> Result<Option<Vec<u8>>, DiffError> {
    use prost::Message;
    let old = EnumBlob::decode(old_decl.data.as_slice())
        .map_err(|e| DiffError::MalformedBlob(format!("EnumBlob: {e}")))?;
    let new = EnumBlob::decode(new_decl.data.as_slice())
        .map_err(|e| DiffError::MalformedBlob(format!("EnumBlob: {e}")))?;
    diff_enums(&old, &new)
}

pub fn diff_tables(old: &TableBlob, new: &TableBlob) -> Result<Option<Vec<u8>>, DiffError> {
    if old == new {
        return Ok(None);
    }

    let mut changes: Vec<serde_json::Value> = Vec::new();

    let old_by_slot: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        old.fields.iter().map(|f| (f.slot_index, f)).collect();
    let new_by_slot: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        new.fields.iter().map(|f| (f.slot_index, f)).collect();

    // Fields added
    for f in &new.fields {
        if !old_by_slot.contains_key(&f.slot_index) {
            changes.push(serde_json::json!({
                "kind": "FieldAdded",
                "table": new.name,
                "field": f.name,
                "type": f.field_type,
                "slot_index": f.slot_index,
            }));
        }
    }

    // Fields removed
    for f in &old.fields {
        if !new_by_slot.contains_key(&f.slot_index) {
            changes.push(serde_json::json!({
                "kind": "FieldRemoved",
                "table": old.name,
                "field": f.name,
                "slot_index": f.slot_index,
            }));
        }
    }

    // Fields modified
    for new_f in &new.fields {
        if let Some(old_f) = old_by_slot.get(&new_f.slot_index) {
            if old_f.name != new_f.name {
                changes.push(serde_json::json!({
                    "kind": "FieldRenamed",
                    "table": new.name,
                    "old_name": old_f.name,
                    "new_name": new_f.name,
                    "slot_index": new_f.slot_index,
                }));
            }
            if old_f.field_type != new_f.field_type {
                changes.push(serde_json::json!({
                    "kind": "FieldTypeChanged",
                    "table": new.name,
                    "field": new_f.name,
                    "old_type": old_f.field_type,
                    "new_type": new_f.field_type,
                }));
            }
            if !old_f.deprecated && new_f.deprecated {
                changes.push(serde_json::json!({
                    "kind": "FieldDeprecated",
                    "table": new.name,
                    "field": new_f.name,
                }));
            }
        }
    }

    if changes.is_empty() {
        // Something else changed (e.g. doc comment)
        changes.push(serde_json::json!({ "kind": "Other" }));
    }

    let detail = serde_json::json!({ "changes": changes });
    Ok(Some(detail.to_string().into_bytes()))
}

pub fn diff_enums(old: &EnumBlob, new: &EnumBlob) -> Result<Option<Vec<u8>>, DiffError> {
    if old == new {
        return Ok(None);
    }

    let mut changes: Vec<serde_json::Value> = Vec::new();

    let old_by_val: std::collections::HashMap<i64, &str> =
        old.values.iter().map(|v| (v.value, v.name.as_str())).collect();
    let new_by_val: std::collections::HashMap<i64, &str> =
        new.values.iter().map(|v| (v.value, v.name.as_str())).collect();

    for (num, name) in &new_by_val {
        if !old_by_val.contains_key(num) {
            changes.push(serde_json::json!({
                "kind": "EnumValueAdded",
                "enum": new.name,
                "value_name": name,
                "value": num,
            }));
        }
    }

    for (num, name) in &old_by_val {
        if !new_by_val.contains_key(num) {
            changes.push(serde_json::json!({
                "kind": "EnumValueRemoved",
                "enum": old.name,
                "value_name": name,
                "value": num,
            }));
        }
    }

    if changes.is_empty() {
        changes.push(serde_json::json!({ "kind": "Other" }));
    }

    let detail = serde_json::json!({ "changes": changes });
    Ok(Some(detail.to_string().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{EnumValueDef, FieldDef};

    fn field(name: &str, ft: &str, slot: u32) -> FieldDef {
        FieldDef {
            name: name.into(),
            field_type: ft.into(),
            default_value: String::new(),
            deprecated: false,
            slot_index: slot,
            doc_comment: String::new(),
        }
    }

    fn make_table(fields: Vec<FieldDef>) -> TableBlob {
        TableBlob { name: "Order".into(), fields, doc_comment: String::new() }
    }

    #[test]
    fn diff_identical_tables_is_none() {
        let t = make_table(vec![field("id", "string", 0)]);
        let result = diff_tables(&t, &t).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn diff_tables_detects_added_field() {
        let old = make_table(vec![field("id", "string", 0)]);
        let new = make_table(vec![field("id", "string", 0), field("amount", "int32", 1)]);
        let result = diff_tables(&old, &new).unwrap().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&result).unwrap();
        let changes = json["changes"].as_array().unwrap();
        assert!(changes.iter().any(|c| c["kind"] == "FieldAdded"), "got: {json}");
    }

    #[test]
    fn diff_tables_detects_deprecated() {
        let old = make_table(vec![field("id", "string", 0)]);
        let mut dep = field("id", "string", 0);
        dep.deprecated = true;
        let new = make_table(vec![dep]);
        let result = diff_tables(&old, &new).unwrap().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&result).unwrap();
        let changes = json["changes"].as_array().unwrap();
        assert!(changes.iter().any(|c| c["kind"] == "FieldDeprecated"), "got: {json}");
    }

    #[test]
    fn diff_identical_enums_is_none() {
        let e = EnumBlob {
            name: "S".into(),
            base_type: "int32".into(),
            values: vec![EnumValueDef { name: "A".into(), value: 0, doc_comment: String::new() }],
            doc_comment: String::new(),
        };
        let result = diff_enums(&e, &e).unwrap();
        assert!(result.is_none());
    }
}
