//! Compatibility orchestration: protected-bookmark gating (design.md §7).
//!
//! On a *protected* bookmark and without `--force`, every changed declaration is
//! checked old-vs-new against the repo's [`CompatibilityRules`]. The old blob is
//! the one currently loaded for the schema; the new blob comes from the
//! mutation effect's upserts. Removing a top-level declaration has no `new`
//! blob to pass through the compiler trait, so the core records that transition
//! as a declaration-level incompatibility itself.

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

    // A top-level declaration disappearing is an API break in every enforced
    // direction. Field/value/RPC removals inside a surviving declaration are
    // represented by an upsert and remain format-compiler decisions above.
    for name in &effect.removes {
        violations.push(CompatibilityViolation {
            declaration_name: name.clone(),
            field_name: None,
            message: "top-level declaration removed".to_string(),
        });
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(CoreError::Incompatible(violations))
    }
}
