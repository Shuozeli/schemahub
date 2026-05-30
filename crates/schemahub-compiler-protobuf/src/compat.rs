//! Per-declaration compatibility checking against the real descriptor
//! (design.md §3.1 / v1 §4.2 rule tables).

use std::collections::HashMap;

use protoc_rs_schema::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, ServiceDescriptorProto,
};
use schemahub_types::{
    CompatibilityDirection, CompatibilityRules, CompatibilityViolation, DeclBlob,
};

use crate::blob::{decode_decl, DeclPayload};
use crate::typecompat::{classify_type_change_typed, TypeChangeCompat};

/// Check whether `new` is compatible with `old` under `rules`.
pub fn check(
    old: &DeclBlob,
    new: &DeclBlob,
    rules: &CompatibilityRules,
) -> Result<(), Vec<CompatibilityViolation>> {
    if rules.disabled || rules.direction == CompatibilityDirection::Disabled {
        return Ok(());
    }
    let old_p = match decode_decl(old.as_bytes()) {
        Ok(p) => p,
        Err(e) => {
            return Err(vec![CompatibilityViolation {
                declaration_name: String::new(),
                field_name: None,
                message: format!("old blob malformed: {e}"),
            }])
        }
    };
    let new_p = match decode_decl(new.as_bytes()) {
        Ok(p) => p,
        Err(e) => {
            return Err(vec![CompatibilityViolation {
                declaration_name: String::new(),
                field_name: None,
                message: format!("new blob malformed: {e}"),
            }])
        }
    };

    let violations = match (old_p, new_p) {
        (DeclPayload::Message(o), DeclPayload::Message(n)) => {
            check_message(&o, &n, rules.direction)
        }
        (DeclPayload::Enum(o), DeclPayload::Enum(n)) => check_enum(&o, &n, rules.direction),
        (DeclPayload::Service(o), DeclPayload::Service(n)) => {
            check_service(&o, &n, rules.direction)
        }
        (o, n) => vec![CompatibilityViolation {
            declaration_name: decl_name(&n),
            field_name: None,
            message: format!(
                "declaration kind changed from {} to {} (always breaking)",
                kind_str(&o),
                kind_str(&n)
            ),
        }],
    };

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn decl_name(p: &DeclPayload) -> String {
    match p {
        DeclPayload::Message(m) => m.name.clone().unwrap_or_default(),
        DeclPayload::Enum(e) => e.name.clone().unwrap_or_default(),
        DeclPayload::Service(s) => s.name.clone().unwrap_or_default(),
    }
}

fn kind_str(p: &DeclPayload) -> &'static str {
    match p {
        DeclPayload::Message(_) => "message",
        DeclPayload::Enum(_) => "enum",
        DeclPayload::Service(_) => "service",
    }
}

// ── Message ──────────────────────────────────────────────────────────────────

fn check_message(
    old: &DescriptorProto,
    new: &DescriptorProto,
    dir: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    let decl = old.name.clone().unwrap_or_default();
    let mut v = Vec::new();

    let old_by_num: HashMap<i32, &FieldDescriptorProto> =
        old.field.iter().filter_map(|f| f.number.map(|n| (n, f))).collect();
    let new_by_num: HashMap<i32, &FieldDescriptorProto> =
        new.field.iter().filter_map(|f| f.number.map(|n| (n, f))).collect();

    // Removed fields (by number) → BACKWARD ✓, FORWARD ✗, FULL ✗.
    for (num, f) in &old_by_num {
        if !new_by_num.contains_key(num)
            && matches!(dir, CompatibilityDirection::Forward | CompatibilityDirection::Full)
        {
            v.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: f.name.clone(),
                message: format!("field number {num} removed: breaks {dir:?} compatibility"),
            });
        }
    }

    // Added fields → always compatible (no violation). Same-number fields:
    // check type/cardinality changes.
    for (num, nf) in &new_by_num {
        let Some(of) = old_by_num.get(num) else {
            continue; // added; compatible
        };
        let from = type_name_of(of);
        let to = type_name_of(nf);
        if from != to {
            match classify_type_change_typed(&from, of.r#type, &to, nf.r#type) {
                TypeChangeCompat::FullyCompat => {}
                TypeChangeCompat::BackwardOnly => {
                    if matches!(dir, CompatibilityDirection::Forward | CompatibilityDirection::Full)
                    {
                        v.push(CompatibilityViolation {
                            declaration_name: decl.clone(),
                            field_name: nf.name.clone(),
                            message: format!(
                                "field {num} type '{from}'→'{to}' is BACKWARD-only, not {dir:?}"
                            ),
                        });
                    }
                }
                TypeChangeCompat::Breaking => {
                    v.push(CompatibilityViolation {
                        declaration_name: decl.clone(),
                        field_name: nf.name.clone(),
                        message: format!(
                            "field {num} type '{from}'→'{to}' is breaking"
                        ),
                    });
                }
            }
        }
        // Cardinality / label change is breaking in all enforced directions.
        if of.label != nf.label || of.proto3_optional != nf.proto3_optional {
            v.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: nf.name.clone(),
                message: format!(
                    "field {num} cardinality changed ({:?}→{:?}): breaking",
                    of.label, nf.label
                ),
            });
        }
    }

    v
}

fn type_name_of(f: &FieldDescriptorProto) -> String {
    use protoc_rs_schema::FieldType;
    match f.r#type {
        Some(FieldType::Message) | Some(FieldType::Enum) | Some(FieldType::Group) => f
            .type_name
            .clone()
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_string(),
        Some(t) => t.proto_name().to_string(),
        None => f.type_name.clone().unwrap_or_default(),
    }
}

// ── Enum ─────────────────────────────────────────────────────────────────────

fn check_enum(
    old: &EnumDescriptorProto,
    new: &EnumDescriptorProto,
    dir: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    let decl = old.name.clone().unwrap_or_default();
    let mut v = Vec::new();
    let old_nums: HashMap<i32, &str> = old
        .value
        .iter()
        .filter_map(|x| Some((x.number?, x.name.as_deref().unwrap_or(""))))
        .collect();
    let new_nums: HashMap<i32, &str> = new
        .value
        .iter()
        .filter_map(|x| Some((x.number?, x.name.as_deref().unwrap_or(""))))
        .collect();

    // Remove enum value → BACKWARD ✗, FORWARD ✓, FULL ✗.
    for (num, name) in &old_nums {
        if !new_nums.contains_key(num)
            && matches!(dir, CompatibilityDirection::Backward | CompatibilityDirection::Full)
        {
            v.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: None,
                message: format!("enum value '{name}' ({num}) removed: breaks {dir:?}"),
            });
        }
    }
    // Add enum value → BACKWARD ✓, FORWARD ✗, FULL ✗.
    for (num, name) in &new_nums {
        if !old_nums.contains_key(num)
            && matches!(dir, CompatibilityDirection::Forward | CompatibilityDirection::Full)
        {
            v.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: None,
                message: format!("enum value '{name}' ({num}) added: breaks {dir:?}"),
            });
        }
    }
    v
}

// ── Service ──────────────────────────────────────────────────────────────────

fn check_service(
    old: &ServiceDescriptorProto,
    new: &ServiceDescriptorProto,
    dir: CompatibilityDirection,
) -> Vec<CompatibilityViolation> {
    let decl = old.name.clone().unwrap_or_default();
    let mut v = Vec::new();
    let old_rpcs: HashMap<&str, &protoc_rs_schema::MethodDescriptorProto> =
        old.method.iter().filter_map(|m| m.name.as_deref().map(|n| (n, m))).collect();
    let new_rpcs: HashMap<&str, &protoc_rs_schema::MethodDescriptorProto> =
        new.method.iter().filter_map(|m| m.name.as_deref().map(|n| (n, m))).collect();

    // Remove RPC → BACKWARD ✗, FORWARD ✓, FULL ✗.
    for name in old_rpcs.keys() {
        if !new_rpcs.contains_key(name)
            && matches!(dir, CompatibilityDirection::Backward | CompatibilityDirection::Full)
        {
            v.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: None,
                message: format!("RPC '{name}' removed: breaks {dir:?}"),
            });
        }
    }
    // Add RPC → BACKWARD ✓, FORWARD ✗, FULL ✗.
    for name in new_rpcs.keys() {
        if !old_rpcs.contains_key(name)
            && matches!(dir, CompatibilityDirection::Forward | CompatibilityDirection::Full)
        {
            v.push(CompatibilityViolation {
                declaration_name: decl.clone(),
                field_name: None,
                message: format!("RPC '{name}' added: breaks {dir:?}"),
            });
        }
    }
    // Changed request/response types or streaming → always breaking.
    for (name, nm) in &new_rpcs {
        if let Some(om) = old_rpcs.get(name) {
            if om.input_type != nm.input_type || om.output_type != nm.output_type {
                v.push(CompatibilityViolation {
                    declaration_name: decl.clone(),
                    field_name: None,
                    message: format!("RPC '{name}' request/response type changed: breaking"),
                });
            }
            if om.client_streaming.unwrap_or(false) != nm.client_streaming.unwrap_or(false)
                || om.server_streaming.unwrap_or(false) != nm.server_streaming.unwrap_or(false)
            {
                v.push(CompatibilityViolation {
                    declaration_name: decl.clone(),
                    field_name: None,
                    message: format!("RPC '{name}' streaming mode changed: breaking"),
                });
            }
        }
    }
    v
}
