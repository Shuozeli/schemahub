use schemahub_types::{
    CompatibilityDirection, CompatibilityRules, CompatibilityViolation,
};

use crate::ast::OpenApiParseResult;
use crate::diff::diff_envelopes;
use schemahub_types::SchemaChange;

/// Check compatibility between two OpenApiParseResult envelopes.
///
/// Rules per design.md Section 4.4:
/// | Change                          | BACKWARD | FORWARD | FULL |
/// |----------------------------------|----------|---------|------|
/// | Add optional request parameter  |   OK     |   OK    |  OK  |
/// | Add required request parameter  |   FAIL   |   OK    |  FAIL|
/// | Remove request parameter        |   OK     |   FAIL  |  FAIL|
/// | Add new endpoint (path+method)  |   OK     |   FAIL  |  FAIL|
/// | Remove endpoint                 |   FAIL   |   OK    |  FAIL|
/// | Remove response field           |   FAIL   |   OK    |  FAIL|
/// | Add optional response field     |   OK     |   OK    |  OK  |
/// | Change response field type      |   FAIL   |   FAIL  |  FAIL|
///
/// For v1: operate at the blob/declaration level. Use diff to detect added/removed declarations.
/// Treat any modification to an existing declaration as FULL violation (conservative).
pub fn check_compatibility(
    old: &OpenApiParseResult,
    new: &OpenApiParseResult,
    rules: &CompatibilityRules,
) -> Vec<CompatibilityViolation> {
    let direction = rules.direction;

    if direction == CompatibilityDirection::Disabled {
        return vec![];
    }

    let changes = diff_envelopes(old, new);
    let mut violations = Vec::new();

    for change in &changes {
        match change {
            SchemaChange::DeclarationAdded { name } => {
                // Adding a new path item: OK for BACKWARD, FAIL for FORWARD and FULL
                if name.starts_with("path:") {
                    match direction {
                        CompatibilityDirection::Forward | CompatibilityDirection::Full => {
                            violations.push(CompatibilityViolation {
                                declaration_name: name.clone(),
                                field_name: None,
                                message: format!(
                                    "Adding new endpoint '{name}' is not compatible with {direction:?} compatibility"
                                ),
                            });
                        }
                        _ => {}
                    }
                }
                // Adding new component declarations is generally OK for backward compat
                // (they are only violations in forward compat for non-path items in strict reading,
                // but for v1 we allow adding new schema components)
            }
            SchemaChange::DeclarationRemoved { name } => {
                // Removing a path item: FAIL for BACKWARD and FULL, OK for FORWARD
                if name.starts_with("path:") {
                    match direction {
                        CompatibilityDirection::Backward | CompatibilityDirection::Full => {
                            violations.push(CompatibilityViolation {
                                declaration_name: name.clone(),
                                field_name: None,
                                message: format!(
                                    "Removing endpoint '{name}' is not compatible with {direction:?} compatibility"
                                ),
                            });
                        }
                        _ => {}
                    }
                }
                // Removing a schema: treat as backward violation
                if name.starts_with("schema:") {
                    match direction {
                        CompatibilityDirection::Backward | CompatibilityDirection::Full => {
                            violations.push(CompatibilityViolation {
                                declaration_name: name.clone(),
                                field_name: None,
                                message: format!(
                                    "Removing schema '{name}' is not compatible with {direction:?} compatibility"
                                ),
                            });
                        }
                        _ => {}
                    }
                }
            }
            SchemaChange::DeclarationModified { name, .. } => {
                // Conservative: any modification is a FULL violation for all directions
                violations.push(CompatibilityViolation {
                    declaration_name: name.clone(),
                    field_name: None,
                    message: format!(
                        "Declaration '{name}' was modified; detailed field-level analysis deferred to v2"
                    ),
                });
            }
        }
    }

    violations
}
