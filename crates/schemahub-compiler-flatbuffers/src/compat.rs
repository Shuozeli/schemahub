//! Per-declaration compatibility checking (design.md §3.2 / v1 §4.3), ported
//! onto the real `flatc-rs-schema` AST.
//!
//! Rule tables:
//!   - Add field at end of table:  BACKWARD ok, FORWARD ok, FULL ok
//!   - Deprecate a field:          BACKWARD ok, FORWARD ok, FULL ok
//!   - Reorder fields:             rejected (slot identity is wire identity)
//!   - Remove field:               rejected (always breaking; use deprecate)
//!   - Type change (same slot):    always breaking (no wire-type remapping)
//!   - Any struct mutation:        rejected (structs are fixed layout)
//!   - Add enum/union value:       BACKWARD ok, FORWARD breaks, FULL breaks
//!   - Remove enum/union value:    BACKWARD breaks, FORWARD ok, FULL breaks

use std::collections::HashMap;

use flatc_rs_schema::{Enum, Field, Object};
use schemahub_types::{
    CompatibilityDirection, CompatibilityRules, CompatibilityViolation, DeclBlob,
};

use crate::blob::{decode_decl, DeclPayload};
use crate::printer::print_type;

/// Check compatibility between an old and new version of one declaration.
pub fn check_compatibility(
    old: &DeclBlob,
    new: &DeclBlob,
    rules: &CompatibilityRules,
) -> Result<(), Vec<CompatibilityViolation>> {
    if rules.disabled || rules.direction == CompatibilityDirection::Disabled {
        return Ok(());
    }

    let old_payload = decode_decl(old).map_err(|e| {
        vec![CompatibilityViolation {
            declaration_name: String::new(),
            field_name: None,
            message: format!("old blob malformed: {e}"),
        }]
    })?;
    let new_payload = decode_decl(new).map_err(|e| {
        vec![CompatibilityViolation {
            declaration_name: String::new(),
            field_name: None,
            message: format!("new blob malformed: {e}"),
        }]
    })?;

    let violations = match (&old_payload, &new_payload) {
        (DeclPayload::Object(o), DeclPayload::Object(n)) => check_object_compat(o, n),
        (DeclPayload::Enum(o), DeclPayload::Enum(n)) => check_enum_compat(o, n, rules.direction),
        // Service changes and kind-changes are out of scope for v1 wire checks.
        _ => Vec::new(),
    };

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// The wire slot identity of a field: its explicit `id` attribute, else its
/// positional index in the table.
fn slot_of(field: &Field, positional: usize) -> u32 {
    field.id.unwrap_or(positional as u32)
}

/// Build a slot → field map for a table.
fn fields_by_slot(obj: &Object) -> HashMap<u32, &Field> {
    obj.fields
        .iter()
        .enumerate()
        .map(|(i, f)| (slot_of(f, i), f))
        .collect()
}

/// Object (table/struct) compatibility. These rules — slot removal, type
/// change, struct mutation, and non-appended additions — are breaking in every
/// enabled direction, so the direction is not consulted here.
fn check_object_compat(old: &Object, new: &Object) -> Vec<CompatibilityViolation> {
    let mut violations = Vec::new();
    let decl = old.name.clone().unwrap_or_default();

    // Any change to a struct is breaking (fixed layout).
    if old.is_struct || new.is_struct {
        if old.fields != new.fields || old.is_struct != new.is_struct {
            violations.push(CompatibilityViolation {
                declaration_name: decl,
                field_name: None,
                message: "struct layout changed: any struct mutation is breaking in FlatBuffers"
                    .to_string(),
            });
        }
        return violations;
    }

    let old_by_slot = fields_by_slot(old);
    let new_by_slot = fields_by_slot(new);
    let max_old_slot = old_by_slot.keys().copied().max();

    // Removed slots: always breaking.
    for (&slot, f) in &old_by_slot {
        if !new_by_slot.contains_key(&slot) {
            violations.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: f.name.clone(),
                message: format!(
                    "field '{}' (slot {slot}) removed: always breaking (slot is wire identity); deprecate instead",
                    f.name.as_deref().unwrap_or("?")
                ),
            });
        }
    }

    // Added slots: only allowed strictly after the previous max slot.
    for (&slot, f) in &new_by_slot {
        if !old_by_slot.contains_key(&slot) {
            if let Some(max_old) = max_old_slot {
                if slot <= max_old {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: f.name.clone(),
                        message: format!(
                            "field '{}' added at slot {slot} which is not after max existing slot {max_old}: fields must be appended",
                            f.name.as_deref().unwrap_or("?")
                        ),
                    });
                }
            }
        }
    }

    // Type changes on retained slots: always breaking.
    for (&slot, new_f) in &new_by_slot {
        if let Some(old_f) = old_by_slot.get(&slot) {
            let old_ty = old_f.type_.as_ref().map(print_type).unwrap_or_default();
            let new_ty = new_f.type_.as_ref().map(print_type).unwrap_or_default();
            if old_ty != new_ty {
                violations.push(CompatibilityViolation {
                    declaration_name: decl.clone(),
                    field_name: new_f.name.clone(),
                    message: format!(
                        "field '{}' (slot {slot}) type changed '{old_ty}' -> '{new_ty}': always breaking in FlatBuffers",
                        new_f.name.as_deref().unwrap_or("?")
                    ),
                });
            }
        }
    }

    violations
}

fn check_enum_compat(
    old: &Enum,
    new: &Enum,
    direction: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    let mut violations = Vec::new();
    let decl = old.name.clone().unwrap_or_default();

    let old_values: HashMap<i64, String> = old
        .values
        .iter()
        .filter_map(|v| Some((v.value?, v.name.clone().unwrap_or_default())))
        .collect();
    let new_values: HashMap<i64, String> = new
        .values
        .iter()
        .filter_map(|v| Some((v.value?, v.name.clone().unwrap_or_default())))
        .collect();

    // Removed values: break BACKWARD and FULL.
    let removal_breaks = matches!(
        direction,
        CompatibilityDirection::Backward | CompatibilityDirection::Full
    );
    for (num, name) in &old_values {
        if removal_breaks && !new_values.contains_key(num) {
            violations.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: None,
                message: format!(
                    "enum value '{name}' ({num}) removed: breaks {direction:?} compatibility"
                ),
            });
        }
    }

    // Added values: break FORWARD and FULL.
    let addition_breaks = matches!(
        direction,
        CompatibilityDirection::Forward | CompatibilityDirection::Full
    );
    for (num, name) in &new_values {
        if addition_breaks && !old_values.contains_key(num) {
            violations.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: None,
                message: format!(
                    "enum value '{name}' ({num}) added: breaks {direction:?} compatibility"
                ),
            });
        }
    }

    violations
}
