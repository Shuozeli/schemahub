//! First-class conflict rendering and resolution validation (design.md §6).

use schemahub_types::{ConflictError, ConflictSides, DeclBlob};

use crate::blob::decode_decl;
use crate::printer::print_decl;

/// Render a conflicted declaration (a merge of N sides) as a human/agent
/// readable view: the base (if any) followed by each divergent side, each in a
/// labeled block of reconstructed `.fbs` source.
pub fn render_conflict(sides: &ConflictSides) -> Result<String, ConflictError> {
    if sides.sides.is_empty() {
        return Err(ConflictError::EmptyConflict);
    }

    let mut out = String::new();
    if let Some(base) = &sides.base {
        let payload = decode_decl(base).map_err(|e| ConflictError::MalformedBlob(e.to_string()))?;
        out.push_str("<<<<<<< base\n");
        out.push_str(&print_decl(&payload));
    }
    for (i, side) in sides.sides.iter().enumerate() {
        let payload = decode_decl(side).map_err(|e| ConflictError::MalformedBlob(e.to_string()))?;
        out.push_str(&format!("======= side {}\n", i + 1));
        out.push_str(&print_decl(&payload));
    }
    out.push_str(">>>>>>>\n");
    Ok(out)
}

/// Validate that a proposed resolution blob is a single valid declaration.
pub fn validate_resolution(resolved: &DeclBlob) -> Result<(), ConflictError> {
    let payload =
        decode_decl(resolved).map_err(|e| ConflictError::MalformedBlob(e.to_string()))?;
    if payload.name().is_none() {
        return Err(ConflictError::InvalidResolution(
            "resolved declaration has no name".to_string(),
        ));
    }
    // Re-render and re-parse to confirm the resolution is syntactically valid.
    let src = print_decl(&payload);
    flatc_rs_parser::FbsParser::new(&src)
        .parse()
        .map_err(|e| ConflictError::InvalidResolution(e.to_string()))?;
    Ok(())
}
