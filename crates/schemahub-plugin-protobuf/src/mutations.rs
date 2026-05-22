use schemahub_types::errors::MutationError;

use crate::ast::{EnumBlob, EnumValueDef, FieldDef, MessageBlob, ReservedRange};
use crate::compat::TypeChangeCompat;
use crate::operations::{
    OpAddEnumValue, OpAddField, OpChangeFieldLabel, OpChangeFieldType, OpRemoveField,
    OpRenameField, OpReorderFields, ProtoOp,
};

// ── Wire-type classification ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireType {
    Varint,
    I64,
    Len,
    I32,
}

fn wire_type_of(t: &str) -> Option<WireType> {
    match t {
        "int32" | "int64" | "uint32" | "uint64" | "sint32" | "sint64" | "bool" => {
            Some(WireType::Varint)
        }
        "fixed64" | "sfixed64" | "double" => Some(WireType::I64),
        "string" | "bytes" => Some(WireType::Len),
        "fixed32" | "sfixed32" | "float" => Some(WireType::I32),
        // message types and repeated are also Len, but we can't always tell from name alone
        _ => {
            // Assume message type → Len
            Some(WireType::Len)
        }
    }
}

/// Check whether a type change is cross-wire-type (always rejected).
fn is_cross_wire_type(from: &str, to: &str) -> bool {
    match (wire_type_of(from), wire_type_of(to)) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

/// Public version used by compat module.
pub fn is_cross_wire_type_pub(from: &str, to: &str) -> bool {
    is_cross_wire_type(from, to)
}

/// Classify a type change for compatibility purposes.
/// Returns TypeChangeCompat describing the compatibility level.
pub fn check_type_change_compat(from: &str, to: &str) -> TypeChangeCompat {
    if from == to {
        return TypeChangeCompat::FullyCompat;
    }
    // Fully compatible pairs (BACKWARD ✓, FORWARD ✓, FULL ✓)
    let fully_compat = [
        ("int32", "int64"),
        ("uint32", "uint64"),
        ("sint32", "sint64"),
        ("string", "bytes"),
        ("bytes", "string"),
        ("uint64", "uint32"),
        ("sint64", "sint32"),
    ];
    for (a, b) in &fully_compat {
        if from == *a && to == *b {
            return TypeChangeCompat::FullyCompat;
        }
    }
    // Backward-only (BACKWARD ✓, FORWARD ✗, FULL ✗)
    let backward_only = [
        ("int64", "int32"),
        ("sint64", "sint32"),
    ];
    for (a, b) in &backward_only {
        if from == *a && to == *b {
            return TypeChangeCompat::BackwardOnly;
        }
    }
    TypeChangeCompat::Breaking
}

/// Check whether a same-wire-type change is in the allowlist.
/// Returns Ok(()) if allowed, Err(reason) if not.
pub fn check_type_change_allowlist(from: &str, to: &str) -> Result<(), String> {
    // Allowlist of permitted type changes (from design.md OQ-13)
    let allowed = [
        ("int32", "int64"),
        ("int64", "int32"),
        ("uint32", "uint64"),
        ("uint64", "uint32"),
        ("sint32", "sint64"),
        ("sint64", "sint32"),
        ("string", "bytes"),
        ("bytes", "string"),
        // enum → int32: treated as "enum type name" → int32; we can't verify the enum name
        // but we allow any non-scalar → int32 (conservative)
    ];

    // Same type is always ok
    if from == to {
        return Ok(());
    }

    for (a, b) in &allowed {
        if from == *a && to == *b {
            return Ok(());
        }
    }

    Err(format!(
        "type change from '{from}' to '{to}' is not in the allowlist"
    ))
}

// ── Number/name reservation helpers ──────────────────────────────────────────

fn is_number_reserved(msg: &MessageBlob, number: u32) -> bool {
    msg.reserved_numbers
        .iter()
        .any(|r| number >= r.start && number <= r.end)
}

fn is_number_in_use(msg: &MessageBlob, number: u32) -> bool {
    msg.fields.iter().any(|f| f.number == number)
}

// ── Message mutation ──────────────────────────────────────────────────────────

pub fn apply_to_message(
    msg: &MessageBlob,
    op: &ProtoOp,
) -> Result<MessageBlob, MutationError> {
    match op {
        ProtoOp::AddField(o) => apply_add_field(msg, o),
        ProtoOp::RemoveField(o) => apply_remove_field(msg, o),
        ProtoOp::RenameField(o) => apply_rename_field(msg, o),
        ProtoOp::ChangeFieldType(o) => apply_change_field_type(msg, o),
        ProtoOp::ChangeFieldLabel(o) => apply_change_field_label(msg, o),
        ProtoOp::ReorderFields(o) => apply_reorder_fields(msg, o),
        // These are declaration-level ops handled at a higher level
        ProtoOp::AddMessage(_) | ProtoOp::RemoveMessage(_) | ProtoOp::RenameMessage(_) => {
            Err(MutationError::InvalidOperation(
                "declaration-level operation applied to a single message blob".into(),
            ))
        }
        ProtoOp::AddEnum(_) | ProtoOp::AddEnumValue(_) | ProtoOp::RemoveEnum(_)
        | ProtoOp::RemoveEnumValue(_) | ProtoOp::RenameEnumValue(_) => {
            Err(MutationError::InvalidOperation(
                "enum operation applied to a message blob".into(),
            ))
        }
        ProtoOp::AddService(_) | ProtoOp::RemoveService(_)
        | ProtoOp::AddRpc(_) | ProtoOp::RemoveRpc(_) | ProtoOp::RenameRpc(_) => {
            Err(MutationError::InvalidOperation(
                "service operation applied to a message blob".into(),
            ))
        }
    }
}

fn apply_add_field(msg: &MessageBlob, op: &OpAddField) -> Result<MessageBlob, MutationError> {
    if is_number_in_use(msg, op.field_number) {
        return Err(MutationError::FieldNumberConflict(op.field_number));
    }
    if is_number_reserved(msg, op.field_number) {
        return Err(MutationError::FieldNumberReserved(op.field_number));
    }

    let mut new_msg = msg.clone();
    new_msg.fields.push(FieldDef {
        name: op.field_name.clone(),
        field_type: op.field_type.clone(),
        number: op.field_number,
        repeated: op.repeated,
        doc_comment: op.doc_comment.clone(),
        ..Default::default()
    });
    Ok(new_msg)
}

fn apply_remove_field(msg: &MessageBlob, op: &OpRemoveField) -> Result<MessageBlob, MutationError> {
    let field_idx = msg
        .fields
        .iter()
        .position(|f| f.name == op.field_name)
        .ok_or_else(|| MutationError::FieldNotFound {
            declaration: msg.name.clone(),
            field: op.field_name.clone(),
        })?;

    let field = &msg.fields[field_idx];
    let number = field.number;
    let name = field.name.clone();

    let mut new_msg = msg.clone();
    new_msg.fields.remove(field_idx);

    // Auto-add reservation for the removed field's number and name
    new_msg
        .reserved_numbers
        .push(ReservedRange { start: number, end: number });
    new_msg.reserved_names.push(name);

    Ok(new_msg)
}

fn apply_rename_field(msg: &MessageBlob, op: &OpRenameField) -> Result<MessageBlob, MutationError> {
    let mut new_msg = msg.clone();
    let field = new_msg
        .fields
        .iter_mut()
        .find(|f| f.name == op.old_field_name)
        .ok_or_else(|| MutationError::FieldNotFound {
            declaration: msg.name.clone(),
            field: op.old_field_name.clone(),
        })?;
    field.name = op.new_field_name.clone();
    Ok(new_msg)
}

fn apply_change_field_type(
    msg: &MessageBlob,
    op: &OpChangeFieldType,
) -> Result<MessageBlob, MutationError> {
    let field = msg
        .fields
        .iter()
        .find(|f| f.name == op.field_name)
        .ok_or_else(|| MutationError::FieldNotFound {
            declaration: msg.name.clone(),
            field: op.field_name.clone(),
        })?;

    let from = &field.field_type;
    let to = &op.new_type;

    if is_cross_wire_type(from, to) {
        return Err(MutationError::InvalidOperation(format!(
            "type change from '{from}' to '{to}' crosses wire-type boundary"
        )));
    }

    check_type_change_allowlist(from, to)
        .map_err(MutationError::InvalidOperation)?;

    let mut new_msg = msg.clone();
    let field = new_msg
        .fields
        .iter_mut()
        .find(|f| f.name == op.field_name)
        .unwrap(); // safe: we found it above
    field.field_type = op.new_type.clone();
    Ok(new_msg)
}

fn apply_change_field_label(
    msg: &MessageBlob,
    op: &OpChangeFieldLabel,
) -> Result<MessageBlob, MutationError> {
    let repeated = match op.new_label.as_str() {
        "optional" => false,
        "repeated" => true,
        other => {
            return Err(MutationError::InvalidOperation(format!(
                "unknown label '{other}'; expected 'optional' or 'repeated'"
            )));
        }
    };

    let mut new_msg = msg.clone();
    let field = new_msg
        .fields
        .iter_mut()
        .find(|f| f.name == op.field_name)
        .ok_or_else(|| MutationError::FieldNotFound {
            declaration: msg.name.clone(),
            field: op.field_name.clone(),
        })?;
    field.repeated = repeated;
    Ok(new_msg)
}

fn apply_reorder_fields(
    msg: &MessageBlob,
    op: &OpReorderFields,
) -> Result<MessageBlob, MutationError> {
    // Validate: all field names present and no extras
    let existing_names: std::collections::HashSet<&str> =
        msg.fields.iter().map(|f| f.name.as_str()).collect();
    let order_names: std::collections::HashSet<&str> =
        op.field_order.iter().map(|s| s.as_str()).collect();

    if existing_names != order_names {
        return Err(MutationError::InvalidOperation(
            "ReorderFields: field names in new order do not match existing fields".into(),
        ));
    }

    let field_map: std::collections::HashMap<&str, &FieldDef> =
        msg.fields.iter().map(|f| (f.name.as_str(), f)).collect();

    let new_fields: Vec<FieldDef> = op
        .field_order
        .iter()
        .map(|name| field_map[name.as_str()].clone())
        .collect();

    let mut new_msg = msg.clone();
    new_msg.fields = new_fields;
    Ok(new_msg)
}

// ── Enum mutation ─────────────────────────────────────────────────────────────

pub fn apply_add_enum_value(
    e: &EnumBlob,
    op: &OpAddEnumValue,
) -> Result<EnumBlob, MutationError> {
    if e.values.iter().any(|v| v.number == op.number) {
        return Err(MutationError::InvalidOperation(format!(
            "enum value number {} is already in use",
            op.number
        )));
    }
    if e.values.iter().any(|v| v.name == op.value_name) {
        return Err(MutationError::InvalidOperation(format!(
            "enum value '{}' already exists",
            op.value_name
        )));
    }
    let mut new_enum = e.clone();
    new_enum.values.push(EnumValueDef {
        name: op.value_name.clone(),
        number: op.number,
        doc_comment: op.doc_comment.clone(),
    });
    Ok(new_enum)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::FieldDef;
    use crate::operations::{
        OpAddField, OpRemoveField, OpRenameField, OpReorderFields,
    };

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

    #[test]
    fn add_field_to_message() {
        let msg = make_msg(vec![field("user_id", "string", 1)]);
        let op = ProtoOp::AddField(OpAddField {
            field_name: "amount".into(),
            field_type: "int64".into(),
            field_number: 2,
            repeated: false,
            doc_comment: String::new(),
        });
        let result = apply_to_message(&msg, &op).unwrap();
        assert_eq!(result.fields.len(), 2);
        assert_eq!(result.fields[1].name, "amount");
    }

    #[test]
    fn remove_field_auto_reserves() {
        let msg = make_msg(vec![field("user_id", "string", 1), field("amount", "int64", 2)]);
        let op = ProtoOp::RemoveField(OpRemoveField {
            field_name: "user_id".into(),
        });
        let result = apply_to_message(&msg, &op).unwrap();
        assert_eq!(result.fields.len(), 1);
        assert_eq!(result.reserved_numbers.len(), 1);
        assert_eq!(result.reserved_numbers[0].start, 1);
        assert!(result.reserved_names.contains(&"user_id".to_string()));
    }

    #[test]
    fn cannot_add_field_with_reserved_number() {
        let mut msg = make_msg(vec![]);
        msg.reserved_numbers.push(ReservedRange { start: 5, end: 5 });
        let op = ProtoOp::AddField(OpAddField {
            field_name: "new_field".into(),
            field_type: "string".into(),
            field_number: 5,
            repeated: false,
            doc_comment: String::new(),
        });
        let result = apply_to_message(&msg, &op);
        assert!(matches!(result, Err(MutationError::FieldNumberReserved(5))));
    }

    #[test]
    fn cannot_add_field_with_duplicate_number() {
        let msg = make_msg(vec![field("name", "string", 3)]);
        let op = ProtoOp::AddField(OpAddField {
            field_name: "other".into(),
            field_type: "string".into(),
            field_number: 3,
            repeated: false,
            doc_comment: String::new(),
        });
        let result = apply_to_message(&msg, &op);
        assert!(matches!(result, Err(MutationError::FieldNumberConflict(3))));
    }

    #[test]
    fn reorder_fields_preserves_numbers() {
        let msg = make_msg(vec![
            field("a", "string", 1),
            field("b", "int32", 2),
            field("c", "bool", 3),
        ]);
        let op = ProtoOp::ReorderFields(OpReorderFields {
            field_order: vec!["c".into(), "a".into(), "b".into()],
        });
        let result = apply_to_message(&msg, &op).unwrap();
        assert_eq!(result.fields[0].name, "c");
        assert_eq!(result.fields[0].number, 3);
        assert_eq!(result.fields[1].name, "a");
        assert_eq!(result.fields[1].number, 1);
    }

    #[test]
    fn rename_field_changes_name() {
        let msg = make_msg(vec![field("old_name", "string", 1)]);
        let op = ProtoOp::RenameField(OpRenameField {
            old_field_name: "old_name".into(),
            new_field_name: "new_name".into(),
        });
        let result = apply_to_message(&msg, &op).unwrap();
        assert_eq!(result.fields[0].name, "new_name");
        assert_eq!(result.fields[0].number, 1);
    }

    #[test]
    fn change_type_cross_wire_rejected() {
        let msg = make_msg(vec![field("amount", "int32", 1)]);
        let op = ProtoOp::ChangeFieldType(crate::operations::OpChangeFieldType {
            field_name: "amount".into(),
            new_type: "string".into(),
        });
        let result = apply_to_message(&msg, &op);
        assert!(result.is_err());
    }

    #[test]
    fn change_type_int32_to_int64_ok() {
        let msg = make_msg(vec![field("amount", "int32", 1)]);
        let op = ProtoOp::ChangeFieldType(crate::operations::OpChangeFieldType {
            field_name: "amount".into(),
            new_type: "int64".into(),
        });
        let result = apply_to_message(&msg, &op).unwrap();
        assert_eq!(result.fields[0].field_type, "int64");
    }
}
