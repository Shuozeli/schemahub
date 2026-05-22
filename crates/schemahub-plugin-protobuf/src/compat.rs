use schemahub_types::{CompatibilityDirection, CompatibilityViolation};

use crate::ast::{EnumBlob, MessageBlob, ServiceBlob};
use crate::mutations::check_type_change_compat;

// ── Message compatibility ─────────────────────────────────────────────────────

pub fn check_message_compat(
    old: &MessageBlob,
    new: &MessageBlob,
    direction: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    if direction == CompatibilityDirection::Disabled {
        return vec![];
    }

    let mut violations = Vec::new();
    let decl = &old.name;

    // Build maps: field name → field, field number → field
    let old_by_name: std::collections::HashMap<&str, &crate::ast::FieldDef> =
        old.fields.iter().map(|f| (f.name.as_str(), f)).collect();
    let new_by_name: std::collections::HashMap<&str, &crate::ast::FieldDef> =
        new.fields.iter().map(|f| (f.name.as_str(), f)).collect();
    let old_by_number: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        old.fields.iter().map(|f| (f.number, f)).collect();
    let new_by_number: std::collections::HashMap<u32, &crate::ast::FieldDef> =
        new.fields.iter().map(|f| (f.number, f)).collect();

    // Fields present in old but not in new → RemoveField
    for f in &old.fields {
        if !new_by_name.contains_key(f.name.as_str()) && !new_by_number.contains_key(&f.number) {
            // Field was fully removed
            // BACKWARD: new schema reads old data → old field not in new = old producers sent
            //   something new consumers won't parse → BACKWARD ok (unknown fields ignored in proto3)
            // FORWARD: old schema reads new data → old consumers may not handle absence of field
            //   Actually: removing a field means new producers don't send it,
            //   old consumers reading new data won't see it. Old consumers that require it break.
            // Design table: Remove field → BACKWARD ✓, FORWARD ✗, FULL ✗
            match direction {
                CompatibilityDirection::Forward => {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: Some(f.name.clone()),
                        message: format!(
                            "field '{}' (number {}) removed: breaks FORWARD compatibility",
                            f.name, f.number
                        ),
                    });
                }
                CompatibilityDirection::Full => {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: Some(f.name.clone()),
                        message: format!(
                            "field '{}' (number {}) removed: breaks FULL compatibility",
                            f.name, f.number
                        ),
                    });
                }
                _ => {}
            }
        }
    }

    // Fields present in new but not in old → AddField
    for f in &new.fields {
        if !old_by_name.contains_key(f.name.as_str()) && !old_by_number.contains_key(&f.number) {
            // Adding a field is always compatible (BACKWARD ✓, FORWARD ✓, FULL ✓)
            // No violations.
        }
    }

    // Fields present in both — check for changes
    for new_field in &new.fields {
        if let Some(old_field) = old_by_name.get(new_field.name.as_str()) {
            // Same name, check number
            if old_field.number != new_field.number {
                // Renumbering is always breaking
                violations.push(CompatibilityViolation {
                    declaration_name: decl.clone(),
                    field_name: Some(new_field.name.clone()),
                    message: format!(
                        "field '{}' number changed from {} to {}: always breaking",
                        new_field.name, old_field.number, new_field.number
                    ),
                });
            }

            // Check type change
            if old_field.field_type != new_field.field_type {
                check_type_compat_violation(
                    decl,
                    &new_field.name,
                    &old_field.field_type,
                    &new_field.field_type,
                    direction,
                    &mut violations,
                );
            }

            // Check label (repeated) change
            if old_field.repeated != new_field.repeated {
                // Changing repeated <-> singular is generally breaking
                match direction {
                    CompatibilityDirection::Backward
                    | CompatibilityDirection::Forward
                    | CompatibilityDirection::Full => {
                        violations.push(CompatibilityViolation {
                            declaration_name: decl.clone(),
                            field_name: Some(new_field.name.clone()),
                            message: format!(
                                "field '{}' label changed (repeated={}→{}): breaking",
                                new_field.name, old_field.repeated, new_field.repeated
                            ),
                        });
                    }
                    CompatibilityDirection::Disabled => {}
                }
            }
        } else if let Some(old_field) = old_by_number.get(&new_field.number) {
            // Same number but different name → rename
            // Rename is compatible: BACKWARD ✓, FORWARD ✓, FULL ✓
            // No violation for rename.
            let _ = old_field;
        }
    }

    violations
}

fn check_type_compat_violation(
    decl: &str,
    field_name: &str,
    from: &str,
    to: &str,
    direction: CompatibilityDirection,
    violations: &mut Vec<CompatibilityViolation>,
) {
    // Cross-wire-type → always breaking (all directions)
    use crate::mutations::is_cross_wire_type_pub;
    if is_cross_wire_type_pub(from, to) {
        violations.push(CompatibilityViolation {
            declaration_name: decl.to_owned(),
            field_name: Some(field_name.to_owned()),
            message: format!(
                "field '{field_name}' type changed from '{from}' to '{to}': \
                 crosses wire-type boundary (always breaking)"
            ),
        });
        return;
    }

    // Same wire type — check allowlist
    // Per design.md:
    //   int32 → int64: BACKWARD ✓, FORWARD ✓, FULL ✓
    //   int64 → int32: BACKWARD ✓, FORWARD ✗, FULL ✗
    //   uint32 → uint64 / sint32 → sint64 / string → bytes: BACKWARD ✓, FORWARD ✓, FULL ✓
    //   All other same-wire-type: ✗ ✗ ✗
    let compat = check_type_change_compat(from, to);
    match compat {
        TypeChangeCompat::FullyCompat => {}
        TypeChangeCompat::BackwardOnly => {
            match direction {
                CompatibilityDirection::Forward | CompatibilityDirection::Full => {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.to_owned(),
                        field_name: Some(field_name.to_owned()),
                        message: format!(
                            "field '{field_name}' type change '{from}'→'{to}' \
                             is BACKWARD-only, not {:?}",
                            direction
                        ),
                    });
                }
                _ => {}
            }
        }
        TypeChangeCompat::Breaking => {
            violations.push(CompatibilityViolation {
                declaration_name: decl.to_owned(),
                field_name: Some(field_name.to_owned()),
                message: format!(
                    "field '{field_name}' type change '{from}'→'{to}' is breaking"
                ),
            });
        }
    }
}

// Exposed to compat module
pub enum TypeChangeCompat {
    FullyCompat,
    BackwardOnly,
    Breaking,
}

// ── Enum compatibility ────────────────────────────────────────────────────────

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

    let old_values: std::collections::HashMap<i32, &str> =
        old.values.iter().map(|v| (v.number, v.name.as_str())).collect();
    let new_values: std::collections::HashMap<i32, &str> =
        new.values.iter().map(|v| (v.number, v.name.as_str())).collect();

    // Values in old but not in new → removed
    for (num, name) in &old_values {
        if !new_values.contains_key(num) {
            // Remove enum value: BACKWARD ✗, FORWARD ✓, FULL ✗
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

    // Values in new but not in old → added
    for (num, name) in &new_values {
        if !old_values.contains_key(num) {
            // Add enum value: BACKWARD ✓, FORWARD ✗, FULL ✗
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

// ── Service compatibility ─────────────────────────────────────────────────────

pub fn check_service_compat(
    old: &ServiceBlob,
    new: &ServiceBlob,
    direction: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    if direction == CompatibilityDirection::Disabled {
        return vec![];
    }

    let mut violations = Vec::new();
    let decl = &old.name;

    let old_rpcs: std::collections::HashMap<&str, &crate::ast::RpcDef> =
        old.rpcs.iter().map(|r| (r.name.as_str(), r)).collect();
    let new_rpcs: std::collections::HashMap<&str, &crate::ast::RpcDef> =
        new.rpcs.iter().map(|r| (r.name.as_str(), r)).collect();

    // RPCs removed → FORWARD ✓, BACKWARD ✗, FULL ✗
    for (name, _) in &old_rpcs {
        if !new_rpcs.contains_key(name) {
            match direction {
                CompatibilityDirection::Backward | CompatibilityDirection::Full => {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: None,
                        message: format!(
                            "RPC '{name}' removed: breaks {:?} compatibility",
                            direction
                        ),
                    });
                }
                _ => {}
            }
        }
    }

    // RPCs added → BACKWARD ✓, FORWARD ✗, FULL ✗
    for (name, _) in &new_rpcs {
        if !old_rpcs.contains_key(name) {
            match direction {
                CompatibilityDirection::Forward | CompatibilityDirection::Full => {
                    violations.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: None,
                        message: format!(
                            "RPC '{name}' added: breaks {:?} compatibility",
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

    fn field(name: &str, ft: &str, number: u32) -> FieldDef {
        FieldDef {
            name: name.into(),
            field_type: ft.into(),
            number,
            repeated: false,
            doc_comment: String::new(),
            ..Default::default()
        }
    }

    fn make_msg(fields: Vec<FieldDef>) -> MessageBlob {
        MessageBlob {
            blob_version: 1,
            name: "TestMsg".into(),
            fields,
            reserved_numbers: vec![],
            reserved_names: vec![],
            doc_comment: String::new(),
        }
    }

    #[test]
    fn add_optional_field_is_full_compatible() {
        let old = make_msg(vec![field("user_id", "string", 1)]);
        let new = make_msg(vec![
            field("user_id", "string", 1),
            field("amount", "int64", 2),
        ]);
        let v = check_message_compat(&old, &new, CompatibilityDirection::Full);
        assert!(v.is_empty(), "expected no violations, got: {v:?}");
    }

    #[test]
    fn remove_field_is_backward_compatible_only() {
        let old = make_msg(vec![
            field("user_id", "string", 1),
            field("amount", "int64", 2),
        ]);
        let new = make_msg(vec![field("user_id", "string", 1)]);

        // BACKWARD: ok
        let v = check_message_compat(&old, &new, CompatibilityDirection::Backward);
        assert!(v.is_empty(), "BACKWARD should be ok: {v:?}");

        // FORWARD: violation
        let v = check_message_compat(&old, &new, CompatibilityDirection::Forward);
        assert!(!v.is_empty(), "FORWARD should have violation");

        // FULL: violation
        let v = check_message_compat(&old, &new, CompatibilityDirection::Full);
        assert!(!v.is_empty(), "FULL should have violation");
    }

    #[test]
    fn rename_field_is_full_compatible() {
        let old = make_msg(vec![field("user_id", "string", 1)]);
        let new = make_msg(vec![field("account_id", "string", 1)]);
        let v = check_message_compat(&old, &new, CompatibilityDirection::Full);
        assert!(v.is_empty(), "rename should be fully compatible: {v:?}");
    }

    #[test]
    fn add_enum_value_backward_compatible() {
        let old = EnumBlob {
            blob_version: 1,
            name: "Status".into(),
            values: vec![
                EnumValueDef { name: "UNKNOWN".into(), number: 0, doc_comment: String::new() },
            ],
            doc_comment: String::new(),
        };
        let new = EnumBlob {
            blob_version: 1,
            name: "Status".into(),
            values: vec![
                EnumValueDef { name: "UNKNOWN".into(), number: 0, doc_comment: String::new() },
                EnumValueDef { name: "ACTIVE".into(), number: 1, doc_comment: String::new() },
            ],
            doc_comment: String::new(),
        };

        // BACKWARD: ok
        let v = check_enum_compat(&old, &new, CompatibilityDirection::Backward);
        assert!(v.is_empty(), "adding enum value should be BACKWARD ok: {v:?}");

        // FORWARD: violation
        let v = check_enum_compat(&old, &new, CompatibilityDirection::Forward);
        assert!(!v.is_empty(), "adding enum value breaks FORWARD");

        // FULL: violation
        let v = check_enum_compat(&old, &new, CompatibilityDirection::Full);
        assert!(!v.is_empty(), "adding enum value breaks FULL");
    }

    #[test]
    fn disabled_compat_no_violations() {
        let old = make_msg(vec![field("a", "string", 1)]);
        let new = make_msg(vec![]);
        let v = check_message_compat(&old, &new, CompatibilityDirection::Disabled);
        assert!(v.is_empty());
    }
}
