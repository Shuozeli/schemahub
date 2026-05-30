//! `diff_decl(old, new)` → a `DeclChange` (design.md §2).

use bytes::Bytes;
use schemahub_types::{DeclBlob, DeclChange, DiffError};

use crate::blob::{decode_decl, DeclPayload};

/// Diff two declaration blobs of the *same* declaration name.
pub fn diff(old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError> {
    let old_p = decode_decl(old.as_bytes()).map_err(|e| DiffError::MalformedBlob(e.to_string()))?;
    let new_p = decode_decl(new.as_bytes()).map_err(|e| DiffError::MalformedBlob(e.to_string()))?;

    let name = payload_name(&new_p);

    // Same bytes → no semantic change, but the trait only models add/remove/
    // modified. Identical content still reports Modified with an empty detail so
    // callers can decide; callers comparing equal blobs usually skip diffing.
    let detail = summarize_change(&old_p, &new_p);
    Ok(DeclChange::DeclarationModified {
        name,
        detail: Bytes::from(detail.into_bytes()),
    })
}

fn payload_name(p: &DeclPayload) -> String {
    match p {
        DeclPayload::Message(m) => m.name.clone().unwrap_or_default(),
        DeclPayload::Enum(e) => e.name.clone().unwrap_or_default(),
        DeclPayload::Service(s) => s.name.clone().unwrap_or_default(),
    }
}

/// A short human-readable change summary (the opaque `detail` bytes).
fn summarize_change(old: &DeclPayload, new: &DeclPayload) -> String {
    match (old, new) {
        (DeclPayload::Message(o), DeclPayload::Message(n)) => {
            let mut lines = Vec::new();
            let on: std::collections::HashSet<_> =
                o.field.iter().filter_map(|f| f.number).collect();
            let nn: std::collections::HashSet<_> =
                n.field.iter().filter_map(|f| f.number).collect();
            for f in &n.field {
                if let Some(num) = f.number {
                    if !on.contains(&num) {
                        lines.push(format!("+ field {} = {}", f.name.as_deref().unwrap_or(""), num));
                    }
                }
            }
            for f in &o.field {
                if let Some(num) = f.number {
                    if !nn.contains(&num) {
                        lines.push(format!("- field {} = {}", f.name.as_deref().unwrap_or(""), num));
                    }
                }
            }
            if lines.is_empty() {
                "message modified".to_string()
            } else {
                lines.join("\n")
            }
        }
        (DeclPayload::Enum(o), DeclPayload::Enum(n)) => {
            let on: std::collections::HashSet<_> =
                o.value.iter().filter_map(|v| v.number).collect();
            let nn: std::collections::HashSet<_> =
                n.value.iter().filter_map(|v| v.number).collect();
            let mut lines = Vec::new();
            for v in &n.value {
                if let Some(num) = v.number {
                    if !on.contains(&num) {
                        lines.push(format!("+ value {} = {}", v.name.as_deref().unwrap_or(""), num));
                    }
                }
            }
            for v in &o.value {
                if let Some(num) = v.number {
                    if !nn.contains(&num) {
                        lines.push(format!("- value {} = {}", v.name.as_deref().unwrap_or(""), num));
                    }
                }
            }
            if lines.is_empty() {
                "enum modified".to_string()
            } else {
                lines.join("\n")
            }
        }
        (DeclPayload::Service(_), DeclPayload::Service(_)) => "service modified".to_string(),
        _ => "declaration kind changed".to_string(),
    }
}
