//! `diff_decl`: compare two declaration blobs of the same name and report a
//! semantic [`DeclChange`] (design.md §2).

use schemahub_types::{DeclBlob, DeclChange, DiffError};

use crate::blob::decode_decl;
use crate::printer::print_decl;

/// Diff two versions of one declaration.
///
/// Both blobs are expected to name the same declaration; the returned change is
/// always `DeclarationModified` carrying a human-readable diff detail. (Add /
/// remove of whole declarations is detected by the VCS layer at tree level, not
/// here, since `diff_decl` always receives an old and a new blob.)
pub fn diff_decl(old: &DeclBlob, new: &DeclBlob) -> Result<DeclChange, DiffError> {
    let old_payload = decode_decl(old).map_err(|e| DiffError::MalformedBlob(e.to_string()))?;
    let new_payload = decode_decl(new).map_err(|e| DiffError::MalformedBlob(e.to_string()))?;

    let name = new_payload
        .name()
        .or_else(|| old_payload.name())
        .unwrap_or("<anonymous>")
        .to_string();

    let old_src = print_decl(&old_payload);
    let new_src = print_decl(&new_payload);
    let detail = format!("--- old\n{old_src}+++ new\n{new_src}");

    Ok(DeclChange::DeclarationModified {
        name,
        detail: detail.into_bytes().into(),
    })
}
