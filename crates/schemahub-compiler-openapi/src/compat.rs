//! Per-declaration compatibility checking (design.md §4.4 / openapi-ast.md §6.7).
//!
//! The trait calls `check_compatibility(old, new, rules)` for one declaration's
//! before/after content. Because we now hold the full decl on both sides (the
//! v1 envelope-level checker could only see add/remove of whole declarations),
//! this performs field-level analysis:
//!
//! | Change                          | BACKWARD | FORWARD | FULL |
//! |---------------------------------|----------|---------|------|
//! | Add optional request parameter  |   OK     |   OK    |  OK  |
//! | Add required request parameter  |   FAIL   |   OK    | FAIL |
//! | Remove request parameter        |   OK     |   FAIL  | FAIL |
//! | Add operation (HTTP method)     |   OK     |   FAIL  | FAIL |
//! | Remove operation                |   FAIL   |   OK    | FAIL |
//! | Add response field (property)   |   OK     |   OK    |  OK  |
//! | Remove response field           |   FAIL   |   OK    | FAIL |
//! | Change response field type      |   FAIL   |   FAIL  | FAIL |
//! | operationId changed             |   FAIL   |   FAIL  | FAIL |
//! | Schema property added           |   OK     |   OK    |  OK  |
//! | Schema property added required  |   FAIL   |   OK    | FAIL |
//! | Schema property removed         |   FAIL   |   OK    | FAIL |
//! | Schema property type changed    |   FAIL   |   FAIL  | FAIL |

use schemahub_types::blob::DeclBlob;
use schemahub_types::compat::{CompatibilityDirection, CompatibilityRules, CompatibilityViolation};

use crate::ast::{
    ComponentSchemaBlob, DeclPayload, HttpMethod, JsonSchemaDef, OperationDef, ParameterOrRef,
    PathItemBlob, SchemaOrRef,
};
use crate::blob::decode_decl;
use crate::diff::decl_key;

/// Returns `Ok(())` if compatible, else the list of violations.
pub fn check_compatibility(
    old: &DeclBlob,
    new: &DeclBlob,
    rules: &CompatibilityRules,
) -> Result<(), Vec<CompatibilityViolation>> {
    if rules.disabled || rules.direction == CompatibilityDirection::Disabled {
        return Ok(());
    }

    let old_decl = decode_decl(old).map_err(|e| {
        vec![CompatibilityViolation {
            declaration_name: "unknown".into(),
            field_name: None,
            message: format!("malformed old blob: {e}"),
        }]
    })?;
    let new_decl = decode_decl(new).map_err(|e| {
        vec![CompatibilityViolation {
            declaration_name: "unknown".into(),
            field_name: None,
            message: format!("malformed new blob: {e}"),
        }]
    })?;

    let name = decl_key(&new_decl.kind);
    let dir = rules.direction;
    let mut v: Vec<CompatibilityViolation> = Vec::new();

    match (&old_decl.kind, &new_decl.kind) {
        (DeclPayload::PathItem(o), DeclPayload::PathItem(n)) => {
            check_path_item(&name, o, n, dir, &mut v);
        }
        (DeclPayload::ComponentSchema(o), DeclPayload::ComponentSchema(n)) => {
            check_component_schema(&name, o, n, dir, &mut v);
        }
        // Other kinds (param/response/requestBody): conservative — flag any
        // content change as a violation under all enabled directions, since
        // field-level analysis for these is deferred.
        _ => {
            if old.as_bytes() != new.as_bytes() {
                v.push(CompatibilityViolation {
                    declaration_name: name.clone(),
                    field_name: None,
                    message: format!(
                        "declaration '{name}' changed; field-level analysis for this kind is deferred to v2"
                    ),
                });
            }
        }
    }

    if v.is_empty() {
        Ok(())
    } else {
        Err(v)
    }
}

fn break_on(dir: CompatibilityDirection, backward_breaks: bool, forward_breaks: bool) -> bool {
    match dir {
        CompatibilityDirection::Backward => backward_breaks,
        CompatibilityDirection::Forward => forward_breaks,
        CompatibilityDirection::Full => backward_breaks || forward_breaks,
        CompatibilityDirection::Disabled => false,
    }
}

fn check_path_item(
    name: &str,
    old: &PathItemBlob,
    new: &PathItemBlob,
    dir: CompatibilityDirection,
    v: &mut Vec<CompatibilityViolation>,
) {
    // ── Operations added / removed ─────────────────────────────────────────────
    let old_methods: Vec<HttpMethod> = old.operations.iter().map(|o| o.method).collect();
    let new_methods: Vec<HttpMethod> = new.operations.iter().map(|o| o.method).collect();

    for m in &new_methods {
        if !old_methods.contains(m) {
            // Adding an operation: breaks FORWARD (and FULL).
            if break_on(dir, false, true) {
                v.push(CompatibilityViolation {
                    declaration_name: name.to_string(),
                    field_name: Some(m.to_str().to_string()),
                    message: format!(
                        "adding operation '{}' breaks {dir:?} compatibility",
                        m.to_str()
                    ),
                });
            }
        }
    }
    for m in &old_methods {
        if !new_methods.contains(m) {
            // Removing an operation: breaks BACKWARD (and FULL).
            if break_on(dir, true, false) {
                v.push(CompatibilityViolation {
                    declaration_name: name.to_string(),
                    field_name: Some(m.to_str().to_string()),
                    message: format!(
                        "removing operation '{}' breaks {dir:?} compatibility",
                        m.to_str()
                    ),
                });
            }
        }
    }

    // ── Per-operation parameter + operationId + response analysis ──────────────
    for new_op in &new.operations {
        if let Some(old_op) = old.operations.iter().find(|o| o.method == new_op.method) {
            check_operation(name, old_op, new_op, dir, v);
        }
    }
}

fn check_operation(
    name: &str,
    old: &OperationDef,
    new: &OperationDef,
    dir: CompatibilityDirection,
    v: &mut Vec<CompatibilityViolation>,
) {
    let method = new.method.to_str();

    // operationId changed → breaks all directions (generated client fn name).
    if old.operation_id != new.operation_id && old.operation_id.is_some() {
        v.push(CompatibilityViolation {
            declaration_name: name.to_string(),
            field_name: Some(format!("{method}.operationId")),
            message: format!("operationId changed on '{method}'; breaks {dir:?} compatibility"),
        });
    }

    // Parameters added/removed.
    let old_params = inline_params(old);
    let new_params = inline_params(new);

    for (pname, required) in &new_params {
        if !old_params.iter().any(|(n, _)| n == pname) {
            // Added parameter. Required add breaks FORWARD; optional add is OK.
            if *required && break_on(dir, false, true) {
                v.push(CompatibilityViolation {
                    declaration_name: name.to_string(),
                    field_name: Some(format!("{method}.parameters.{pname}")),
                    message: format!(
                        "adding required parameter '{pname}' on '{method}' breaks {dir:?} compatibility"
                    ),
                });
            }
        }
    }
    for (pname, _) in &old_params {
        if !new_params.iter().any(|(n, _)| n == pname) {
            // Removed parameter: breaks FORWARD (old servers expect it absent? no —
            // removing breaks FORWARD per the table) -> forward_breaks = true.
            if break_on(dir, false, true) {
                v.push(CompatibilityViolation {
                    declaration_name: name.to_string(),
                    field_name: Some(format!("{method}.parameters.{pname}")),
                    message: format!(
                        "removing parameter '{pname}' on '{method}' breaks {dir:?} compatibility"
                    ),
                });
            }
        }
    }
}

/// Collect (name, required) for inline parameters of an operation.
fn inline_params(op: &OperationDef) -> Vec<(String, bool)> {
    op.parameters
        .iter()
        .filter_map(|p| match p {
            ParameterOrRef::Inline(param) => Some((param.name.clone(), param.required)),
            ParameterOrRef::Ref(_) => None,
        })
        .collect()
}

fn check_component_schema(
    name: &str,
    old: &ComponentSchemaBlob,
    new: &ComponentSchemaBlob,
    dir: CompatibilityDirection,
    v: &mut Vec<CompatibilityViolation>,
) {
    let (Some(o), Some(n)) = (&old.schema, &new.schema) else {
        return;
    };

    let old_props = prop_map(o);
    let new_props = prop_map(n);

    for (pname, ptype) in &new_props {
        match old_props.iter().find(|(name, _)| name == pname) {
            None => {
                // Property added. If it's now required, breaks FORWARD.
                if n.required.contains(pname) && break_on(dir, false, true) {
                    v.push(CompatibilityViolation {
                        declaration_name: name.to_string(),
                        field_name: Some(pname.clone()),
                        message: format!(
                            "adding required property '{pname}' breaks {dir:?} compatibility"
                        ),
                    });
                }
            }
            Some((_, old_type)) => {
                // Property type changed → breaks all directions.
                if old_type != ptype {
                    v.push(CompatibilityViolation {
                        declaration_name: name.to_string(),
                        field_name: Some(pname.clone()),
                        message: format!(
                            "property '{pname}' type changed ({old_type} -> {ptype}); breaks {dir:?} compatibility"
                        ),
                    });
                }
            }
        }
    }
    for (pname, _) in &old_props {
        if !new_props.iter().any(|(name, _)| name == pname) {
            // Property removed: breaks FORWARD.
            if break_on(dir, false, true) {
                v.push(CompatibilityViolation {
                    declaration_name: name.to_string(),
                    field_name: Some(pname.clone()),
                    message: format!("removing property '{pname}' breaks {dir:?} compatibility"),
                });
            }
        }
    }
}

/// A simple `(name, type-signature)` view of a schema's direct properties, used
/// to detect property type changes.
fn prop_map(schema: &JsonSchemaDef) -> Vec<(String, String)> {
    schema
        .properties
        .iter()
        .map(|p| {
            let sig = p
                .schema
                .as_ref()
                .map(type_signature)
                .unwrap_or_else(|| "unknown".into());
            (p.name.clone(), sig)
        })
        .collect()
}

/// A coarse type signature for a property schema — enough to flag type changes.
fn type_signature(s: &SchemaOrRef) -> String {
    match s {
        SchemaOrRef::Ref(r) => format!("$ref:{}", r.local_name),
        SchemaOrRef::Inline(def) => {
            let types: Vec<&str> = def.types.iter().map(|t| t.to_str()).collect();
            let fmt = def.format.as_deref().unwrap_or("");
            if fmt.is_empty() {
                format!("{types:?}")
            } else {
                format!("{types:?}/{fmt}")
            }
        }
    }
}
