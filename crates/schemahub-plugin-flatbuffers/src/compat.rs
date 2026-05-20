use schemahub_types::{CompatibilityDirection, CompatibilityViolation};

use crate::ast::{EnumBlob, TableBlob};

/// Check compatibility of a table change.
///
/// Per design:
///   Add field at end of table: BACKWARD ✓, FORWARD ✓, FULL ✓
///   Deprecate a field:         BACKWARD ✓, FORWARD ✓, FULL ✓
///   Reorder fields:            rejected at mutation layer
///   Remove field:              rejected at mutation layer
pub fn check_table_compat(
    old: &TableBlob,
    new: &TableBlob,
    direction: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    if direction == CompatibilityDirection::Disabled {
        return vec![];
    }

    let mut violations = Vec::new();
    let decl = &old.name;

    // Index old fields by slot_index
    let old_by_slot: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        old.fields.iter().map(|f| (f.slot_index, f)).collect();
    let new_by_slot: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        new.fields.iter().map(|f| (f.slot_index, f)).collect();

    // Fields in old but not in new (by slot_index) — effectively removed
    for f in &old.fields {
        if !new_by_slot.contains_key(&f.slot_index) {
            // Removing a field is rejected at mutation layer; but if we see this in compat check,
            // it's always a violation for all directions (field slot gone means wire breakage).
            violations.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: Some(f.name.clone()),
                message: format!(
                    "field '{}' (slot {}) removed: always breaking for FlatBuffers (slot identity is wire identity)",
                    f.name, f.slot_index
                ),
            });
        }
    }

    // Fields in new but not in old — additions at end are ok; check ordering invariant
    for f in &new.fields {
        if !old_by_slot.contains_key(&f.slot_index) {
            // New field added — check that slot_index is max_old + 1 or higher
            let max_old_slot = old.fields.iter().map(|f| f.slot_index).max().unwrap_or(0);
            if !old.fields.is_empty() && f.slot_index <= max_old_slot {
                // Field inserted in the middle of existing slots — this is a wire breakage
                violations.push(CompatibilityViolation {
                    declaration_name: decl.clone(),
                    field_name: Some(f.name.clone()),
                    message: format!(
                        "field '{}' has slot_index {} which is not appended after existing max slot {}: FlatBuffers slot order must not be changed",
                        f.name, f.slot_index, max_old_slot
                    ),
                });
            }
            // Adding at end: compatible for all directions
        }
    }

    // Type changes for same slots
    for new_f in &new.fields {
        if let Some(old_f) = old_by_slot.get(&new_f.slot_index) {
            if old_f.field_type != new_f.field_type {
                // Any type change in FlatBuffers is breaking (no wire-type remapping like proto)
                violations.push(CompatibilityViolation {
                    declaration_name: decl.clone(),
                    field_name: Some(new_f.name.clone()),
                    message: format!(
                        "field '{}' (slot {}) type changed from '{}' to '{}': always breaking in FlatBuffers",
                        new_f.name, new_f.slot_index, old_f.field_type, new_f.field_type
                    ),
                });
            }
        }
    }

    violations
}

/// Check compatibility of an enum change.
///
/// Per design:
///   Add enum value: BACKWARD ✓, FORWARD ✗, FULL ✗
pub fn check_enum_compat(
    old: &EnumBlob,
    new: &EnumBlob,
    direction: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    if direction == CompatibilityDirection::Disabled {
        return vec![];
    }

    let mut violations = Vec::new();
    let decl = &old.name;

    let old_values: std::collections::HashMap<i64, &str> =
        old.values.iter().map(|v| (v.value, v.name.as_str())).collect();
    let new_values: std::collections::HashMap<i64, &str> =
        new.values.iter().map(|v| (v.value, v.name.as_str())).collect();

    // Values removed: breaks backward and full
    for (num, name) in &old_values {
        if !new_values.contains_key(num) {
            match direction {
                CompatibilityDirection::Backward | CompatibilityDirection::Full => {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: None,
                        message: format!(
                            "enum value '{name}' ({num}) removed: breaks {:?} compatibility",
                            direction
                        ),
                    });
                }
                _ => {}
            }
        }
    }

    // Values added: breaks forward and full
    for (num, name) in &new_values {
        if !old_values.contains_key(num) {
            match direction {
                CompatibilityDirection::Forward | CompatibilityDirection::Full => {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: None,
                        message: format!(
                            "enum value '{name}' ({num}) added: breaks {:?} compatibility",
                            direction
                        ),
                    });
                }
                _ => {}
            }
        }
    }

    violations
}

// ── Tests ─────────────────────────────────────────────────────────────────────

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

    fn make_enum(values: Vec<(&str, i64)>) -> EnumBlob {
        EnumBlob {
            name: "Status".into(),
            base_type: "int32".into(),
            values: values
                .into_iter()
                .map(|(name, value)| EnumValueDef {
                    name: name.into(),
                    value,
                    doc_comment: String::new(),
                })
                .collect(),
            doc_comment: String::new(),
        }
    }

    #[test]
    fn add_field_at_end_is_full_compatible() {
        let old = make_table(vec![field("id", "string", 0)]);
        let new = make_table(vec![field("id", "string", 0), field("amount", "int32", 1)]);
        let v = check_table_compat(&old, &new, CompatibilityDirection::Full);
        assert!(v.is_empty(), "adding field at end should be FULL compatible: {v:?}");
    }

    #[test]
    fn add_field_at_end_is_backward_compatible() {
        let old = make_table(vec![field("id", "string", 0)]);
        let new = make_table(vec![field("id", "string", 0), field("amount", "int32", 1)]);
        let v = check_table_compat(&old, &new, CompatibilityDirection::Backward);
        assert!(v.is_empty(), "adding field at end: {v:?}");
    }

    #[test]
    fn add_field_at_end_is_forward_compatible() {
        let old = make_table(vec![field("id", "string", 0)]);
        let new = make_table(vec![field("id", "string", 0), field("amount", "int32", 1)]);
        let v = check_table_compat(&old, &new, CompatibilityDirection::Forward);
        assert!(v.is_empty(), "adding field at end: {v:?}");
    }

    #[test]
    fn deprecate_field_is_full_compatible() {
        let old = make_table(vec![field("id", "string", 0), field("old_f", "string", 1)]);
        let mut deprecated = field("old_f", "string", 1);
        deprecated.deprecated = true;
        let new = make_table(vec![field("id", "string", 0), deprecated]);
        let v = check_table_compat(&old, &new, CompatibilityDirection::Full);
        assert!(v.is_empty(), "deprecation should be fully compatible: {v:?}");
    }

    #[test]
    fn type_change_is_always_breaking() {
        let old = make_table(vec![field("amount", "int32", 0)]);
        let new = make_table(vec![field("amount", "int64", 0)]);
        // All directions
        for dir in [
            CompatibilityDirection::Backward,
            CompatibilityDirection::Forward,
            CompatibilityDirection::Full,
        ] {
            let v = check_table_compat(&old, &new, dir);
            assert!(!v.is_empty(), "type change should be breaking for {dir:?}: {v:?}");
        }
    }

    #[test]
    fn add_enum_value_backward_compatible() {
        let old = make_enum(vec![("UNKNOWN", 0)]);
        let new = make_enum(vec![("UNKNOWN", 0), ("ACTIVE", 1)]);
        let v = check_enum_compat(&old, &new, CompatibilityDirection::Backward);
        assert!(v.is_empty(), "adding enum value should be BACKWARD ok: {v:?}");
    }

    #[test]
    fn add_enum_value_breaks_forward() {
        let old = make_enum(vec![("UNKNOWN", 0)]);
        let new = make_enum(vec![("UNKNOWN", 0), ("ACTIVE", 1)]);
        let v = check_enum_compat(&old, &new, CompatibilityDirection::Forward);
        assert!(!v.is_empty(), "adding enum value should break FORWARD");
    }

    #[test]
    fn add_enum_value_breaks_full() {
        let old = make_enum(vec![("UNKNOWN", 0)]);
        let new = make_enum(vec![("UNKNOWN", 0), ("ACTIVE", 1)]);
        let v = check_enum_compat(&old, &new, CompatibilityDirection::Full);
        assert!(!v.is_empty(), "adding enum value should break FULL");
    }

    #[test]
    fn remove_enum_value_breaks_backward() {
        let old = make_enum(vec![("UNKNOWN", 0), ("ACTIVE", 1)]);
        let new = make_enum(vec![("UNKNOWN", 0)]);
        let v = check_enum_compat(&old, &new, CompatibilityDirection::Backward);
        assert!(!v.is_empty(), "removing enum value should break BACKWARD");
    }

    #[test]
    fn disabled_no_violations() {
        let old = make_table(vec![field("id", "string", 0)]);
        let new = make_table(vec![]);
        let v = check_table_compat(&old, &new, CompatibilityDirection::Disabled);
        assert!(v.is_empty());
    }
}
