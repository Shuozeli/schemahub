use schemahub_types::errors::MutationError;

use crate::ast::{EnumBlob, EnumValueDef, FieldDef, StructBlob, TableBlob};
use crate::operations::{FbsOp, OpAddEnumValue, OpAddField, OpDeprecateField, OpRenameField};

// ── Table mutations ───────────────────────────────────────────────────────────

pub fn apply_to_table(blob: &TableBlob, op: &FbsOp) -> Result<TableBlob, MutationError> {
    match op {
        FbsOp::AddField(o) => apply_add_field(blob, o),
        FbsOp::DeprecateField(o) => apply_deprecate_field(blob, o),
        FbsOp::RenameField(o) => apply_rename_field(blob, o),
        FbsOp::AddTable(_)
        | FbsOp::RemoveTable(_)
        | FbsOp::RenameTable(_)
        | FbsOp::AddEnum(_)
        | FbsOp::AddEnumValue(_)
        | FbsOp::AddUnion(_)
        | FbsOp::UpdateImport(_) => Err(MutationError::InvalidOperation(
            "declaration-level operation applied to a table blob".into(),
        )),
    }
}

fn apply_add_field(blob: &TableBlob, op: &OpAddField) -> Result<TableBlob, MutationError> {
    // Enforce unique name
    if blob.fields.iter().any(|f| f.name == op.field_name) {
        return Err(MutationError::InvalidOperation(format!(
            "field '{}' already exists in table '{}'",
            op.field_name, blob.name
        )));
    }

    // Slot index must be max_existing + 1
    let next_slot = blob.fields.iter().map(|f| f.slot_index).max().map(|m| m + 1).unwrap_or(0);

    let mut new_blob = blob.clone();
    new_blob.fields.push(FieldDef {
        name: op.field_name.clone(),
        field_type: op.field_type.clone(),
        default_value: op.default_value.clone(),
        deprecated: false,
        slot_index: next_slot,
        doc_comment: op.doc_comment.clone(),
    });
    Ok(new_blob)
}

fn apply_deprecate_field(
    blob: &TableBlob,
    op: &OpDeprecateField,
) -> Result<TableBlob, MutationError> {
    let mut new_blob = blob.clone();
    let field = new_blob
        .fields
        .iter_mut()
        .find(|f| f.name == op.field_name)
        .ok_or_else(|| MutationError::FieldNotFound {
            declaration: blob.name.clone(),
            field: op.field_name.clone(),
        })?;
    field.deprecated = true;
    Ok(new_blob)
}

fn apply_rename_field(blob: &TableBlob, op: &OpRenameField) -> Result<TableBlob, MutationError> {
    // Ensure new name doesn't clash
    if blob.fields.iter().any(|f| f.name == op.new_field_name) {
        return Err(MutationError::InvalidOperation(format!(
            "field '{}' already exists in table '{}'",
            op.new_field_name, blob.name
        )));
    }
    let mut new_blob = blob.clone();
    let field = new_blob
        .fields
        .iter_mut()
        .find(|f| f.name == op.old_field_name)
        .ok_or_else(|| MutationError::FieldNotFound {
            declaration: blob.name.clone(),
            field: op.old_field_name.clone(),
        })?;
    field.name = op.new_field_name.clone();
    Ok(new_blob)
}

// ── Enum mutations ────────────────────────────────────────────────────────────

pub fn apply_to_enum(blob: &EnumBlob, op: &FbsOp) -> Result<EnumBlob, MutationError> {
    match op {
        FbsOp::AddEnumValue(o) => apply_add_enum_value(blob, o),
        _ => Err(MutationError::InvalidOperation(format!(
            "operation {:?} cannot be applied to an enum blob",
            std::mem::discriminant(op)
        ))),
    }
}

fn apply_add_enum_value(blob: &EnumBlob, op: &OpAddEnumValue) -> Result<EnumBlob, MutationError> {
    if blob.values.iter().any(|v| v.value == op.value) {
        return Err(MutationError::InvalidOperation(format!(
            "enum value {} is already in use in enum '{}'",
            op.value, blob.name
        )));
    }
    if blob.values.iter().any(|v| v.name == op.value_name) {
        return Err(MutationError::InvalidOperation(format!(
            "enum value '{}' already exists in enum '{}'",
            op.value_name, blob.name
        )));
    }
    let mut new_blob = blob.clone();
    new_blob.values.push(EnumValueDef {
        name: op.value_name.clone(),
        value: op.value,
        doc_comment: op.doc_comment.clone(),
    });
    Ok(new_blob)
}

// ── Struct mutations ──────────────────────────────────────────────────────────

/// All struct mutations are rejected — structs are immutable once defined.
pub fn apply_to_struct(_blob: &StructBlob, _op: &FbsOp) -> Result<StructBlob, MutationError> {
    Err(MutationError::InvalidOperation(
        "Structs are immutable once defined".into(),
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::FieldDef;
    use crate::operations::{
        FbsOp, OpAddEnumValue, OpAddField, OpDeprecateField, OpRenameField,
    };

    fn make_table(fields: Vec<FieldDef>) -> TableBlob {
        TableBlob { name: "Order".into(), fields, doc_comment: String::new() }
    }

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

    fn make_enum(values: Vec<(String, i64)>) -> EnumBlob {
        EnumBlob {
            name: "Status".into(),
            base_type: "int32".into(),
            values: values
                .into_iter()
                .map(|(name, value)| EnumValueDef { name, value, doc_comment: String::new() })
                .collect(),
            doc_comment: String::new(),
        }
    }

    #[test]
    fn add_field_appends_with_next_slot() {
        let t = make_table(vec![field("id", "string", 0), field("amount", "int32", 1)]);
        let op = FbsOp::AddField(OpAddField {
            field_name: "created_at".into(),
            field_type: "int64".into(),
            default_value: String::new(),
            doc_comment: String::new(),
        });
        let result = apply_to_table(&t, &op).unwrap();
        assert_eq!(result.fields.len(), 3);
        assert_eq!(result.fields[2].name, "created_at");
        assert_eq!(result.fields[2].slot_index, 2);
    }

    #[test]
    fn add_field_first_slot_is_zero() {
        let t = make_table(vec![]);
        let op = FbsOp::AddField(OpAddField {
            field_name: "id".into(),
            field_type: "string".into(),
            default_value: String::new(),
            doc_comment: String::new(),
        });
        let result = apply_to_table(&t, &op).unwrap();
        assert_eq!(result.fields[0].slot_index, 0);
    }

    #[test]
    fn add_field_rejects_duplicate_name() {
        let t = make_table(vec![field("id", "string", 0)]);
        let op = FbsOp::AddField(OpAddField {
            field_name: "id".into(),
            field_type: "int32".into(),
            default_value: String::new(),
            doc_comment: String::new(),
        });
        let result = apply_to_table(&t, &op);
        assert!(result.is_err(), "should reject duplicate name");
    }

    #[test]
    fn deprecate_field_sets_flag() {
        let t = make_table(vec![field("old_field", "string", 0)]);
        let op = FbsOp::DeprecateField(OpDeprecateField { field_name: "old_field".into() });
        let result = apply_to_table(&t, &op).unwrap();
        assert!(result.fields[0].deprecated);
        assert_eq!(result.fields[0].slot_index, 0); // slot unchanged
    }

    #[test]
    fn deprecate_field_not_found() {
        let t = make_table(vec![]);
        let op = FbsOp::DeprecateField(OpDeprecateField { field_name: "missing".into() });
        let result = apply_to_table(&t, &op);
        assert!(matches!(result, Err(MutationError::FieldNotFound { .. })));
    }

    #[test]
    fn rename_field_preserves_slot() {
        let t = make_table(vec![field("old_name", "string", 0)]);
        let op = FbsOp::RenameField(OpRenameField {
            old_field_name: "old_name".into(),
            new_field_name: "new_name".into(),
        });
        let result = apply_to_table(&t, &op).unwrap();
        assert_eq!(result.fields[0].name, "new_name");
        assert_eq!(result.fields[0].slot_index, 0);
    }

    #[test]
    fn rename_field_not_found() {
        let t = make_table(vec![field("id", "string", 0)]);
        let op = FbsOp::RenameField(OpRenameField {
            old_field_name: "missing".into(),
            new_field_name: "other".into(),
        });
        let result = apply_to_table(&t, &op);
        assert!(matches!(result, Err(MutationError::FieldNotFound { .. })));
    }

    #[test]
    fn rename_field_clash_rejected() {
        let t = make_table(vec![field("a", "string", 0), field("b", "int32", 1)]);
        let op = FbsOp::RenameField(OpRenameField {
            old_field_name: "a".into(),
            new_field_name: "b".into(),
        });
        let result = apply_to_table(&t, &op);
        assert!(result.is_err(), "should reject rename to existing name");
    }

    #[test]
    fn add_enum_value_ok() {
        let e = make_enum(vec![("UNKNOWN".into(), 0)]);
        let op = FbsOp::AddEnumValue(OpAddEnumValue {
            enum_name: "Status".into(),
            value_name: "ACTIVE".into(),
            value: 1,
            doc_comment: String::new(),
        });
        let result = apply_to_enum(&e, &op).unwrap();
        assert_eq!(result.values.len(), 2);
        assert_eq!(result.values[1].name, "ACTIVE");
        assert_eq!(result.values[1].value, 1);
    }

    #[test]
    fn add_enum_value_conflict_rejected() {
        let e = make_enum(vec![("UNKNOWN".into(), 0)]);
        let op = FbsOp::AddEnumValue(OpAddEnumValue {
            enum_name: "Status".into(),
            value_name: "OTHER".into(),
            value: 0, // conflict
            doc_comment: String::new(),
        });
        let result = apply_to_enum(&e, &op);
        assert!(result.is_err(), "should reject duplicate value number");
    }

    #[test]
    fn struct_is_immutable() {
        let s = StructBlob {
            name: "Vec3".into(),
            fields: vec![],
            doc_comment: String::new(),
        };
        let op = FbsOp::AddField(OpAddField {
            field_name: "w".into(),
            field_type: "float".into(),
            default_value: String::new(),
            doc_comment: String::new(),
        });
        let result = apply_to_struct(&s, &op);
        assert!(result.is_err(), "structs must reject all mutations");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("immutable"), "got: {err}");
    }

    #[test]
    fn remove_field_rejected() {
        // There is no RemoveField operation — verify that any operation not in the
        // FbsOp enum simply returns an error when applied to a table.
        let t = make_table(vec![field("id", "string", 0)]);
        // AddEnum is a declaration-level op, should fail on table
        let op = FbsOp::AddEnum(crate::operations::OpAddEnum {
            enum_name: "Status".into(),
            base_type: "int32".into(),
            doc_comment: String::new(),
        });
        let result = apply_to_table(&t, &op);
        assert!(result.is_err(), "declaration-level op should fail on table blob");
    }
}
