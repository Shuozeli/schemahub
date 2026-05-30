//! Compatibility orchestration: protected-bookmark gating (design.md §7).
//!
//! On a *protected* bookmark and without `--force`, every changed declaration is
//! checked old-vs-new against the repo's [`CompatibilityRules`]. The old blob is
//! the one currently loaded for the schema; the new blob comes from the
//! mutation effect's upserts. Removals are checked old-vs-absent only when the
//! compiler models that as a violation (it receives the old blob and decides);
//! here we surface every violation the compiler reports.

use schemahub_types::{
    CompatibilityRules, CompatibilityViolation, Compiler, MutationEffect, SchemaObjects,
};

use crate::error::{CoreError, CoreResult};

/// Run the compatibility gate for a single schema's mutation effect.
///
/// `old` is the schema as currently stored; `effect` is the proposed change.
/// Returns `Err(CoreError::Incompatible(..))` collecting *all* violations across
/// changed declarations, so the caller can report them together.
pub(crate) fn gate(
    compiler: &dyn Compiler,
    rules: &CompatibilityRules,
    old: &SchemaObjects,
    effect: &MutationEffect,
) -> CoreResult<()> {
    if rules.disabled {
        return Ok(());
    }

    let mut violations: Vec<CompatibilityViolation> = Vec::new();

    // Upserts: check each changed/added declaration against its prior version.
    for (name, new_blob) in &effect.upserts {
        // A pure addition (no prior decl) is always compatible — skip it; the
        // compiler's check is for old→new transitions only.
        let Some(old_blob) = old.decls.get(name) else {
            continue;
        };
        if let Err(mut vs) = compiler.check_compatibility(old_blob, new_blob, rules) {
            violations.append(&mut vs);
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(CoreError::Incompatible(violations))
    }
}
