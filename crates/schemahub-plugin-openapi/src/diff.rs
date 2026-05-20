use schemahub_types::SchemaChange;
use bytes::Bytes;

use crate::ast::OpenApiParseResult;

/// Compare two OpenApiParseResult envelopes and return a list of SchemaChange entries.
pub fn diff_envelopes(
    old: &OpenApiParseResult,
    new: &OpenApiParseResult,
) -> Vec<SchemaChange> {
    let mut changes = Vec::new();

    // Build maps from tree_key → blob_bytes
    let old_map: std::collections::HashMap<&str, &[u8]> = old
        .declarations
        .iter()
        .map(|d| (d.tree_key.as_str(), d.blob_bytes.as_slice()))
        .collect();

    let new_map: std::collections::HashMap<&str, &[u8]> = new
        .declarations
        .iter()
        .map(|d| (d.tree_key.as_str(), d.blob_bytes.as_slice()))
        .collect();

    // Added in new
    for (key, _) in &new_map {
        if !old_map.contains_key(key) {
            changes.push(SchemaChange::DeclarationAdded { name: key.to_string() });
        }
    }

    // Removed in new
    for (key, _) in &old_map {
        if !new_map.contains_key(key) {
            changes.push(SchemaChange::DeclarationRemoved { name: key.to_string() });
        }
    }

    // Modified
    for (key, new_bytes) in &new_map {
        if let Some(old_bytes) = old_map.get(key) {
            if old_bytes != new_bytes {
                changes.push(SchemaChange::DeclarationModified {
                    name: key.to_string(),
                    detail: Bytes::from_static(b"content changed"),
                });
            }
        }
    }

    changes
}
