//! First-class conflict rendering / resolution validation (design.md §6).

use schemahub_types::{ConflictError, ConflictSides, DeclBlob};

use crate::blob::{decode_decl, DeclPayload};

/// Render the competing sides of a conflicted declaration for display.
pub fn render(sides: &ConflictSides) -> Result<String, ConflictError> {
    if sides.sides.is_empty() {
        return Err(ConflictError::EmptyConflict);
    }
    let mut out = String::new();
    out.push_str("<<<<<<< conflict\n");
    if let Some(base) = &sides.base {
        out.push_str("======= base\n");
        out.push_str(&render_one(base)?);
        out.push('\n');
    }
    for (i, side) in sides.sides.iter().enumerate() {
        out.push_str(&format!("======= side {}\n", i + 1));
        out.push_str(&render_one(side)?);
        out.push('\n');
    }
    out.push_str(">>>>>>> end\n");
    Ok(out)
}

fn render_one(blob: &DeclBlob) -> Result<String, ConflictError> {
    let detail =
        crate::read::detail(blob).map_err(|e| ConflictError::MalformedBlob(e.to_string()))?;
    Ok(String::from_utf8_lossy(detail.as_bytes()).into_owned())
}

/// Validate that `resolved` decodes to a single, well-formed declaration.
pub fn validate_resolution(resolved: &DeclBlob) -> Result<(), ConflictError> {
    let p = decode_decl(resolved.as_bytes())
        .map_err(|e| ConflictError::InvalidResolution(e.to_string()))?;
    let named = match &p {
        DeclPayload::Message(m) => m.name.is_some(),
        DeclPayload::Enum(e) => e.name.is_some(),
        DeclPayload::Service(s) => s.name.is_some(),
    };
    if !named {
        return Err(ConflictError::InvalidResolution(
            "resolved declaration has no name".into(),
        ));
    }
    Ok(())
}
