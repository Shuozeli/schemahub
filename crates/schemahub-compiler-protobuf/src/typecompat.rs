//! Wire-type classification and the type-change compatibility allowlist
//! (design.md §3.1 / v1 §4.2), evaluated against the real `FieldType`.

use protoc_rs_schema::{FieldType, WireType};

/// How compatible a field type change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeChangeCompat {
    /// BACKWARD ✓, FORWARD ✓, FULL ✓.
    FullyCompat,
    /// BACKWARD ✓, FORWARD ✗, FULL ✗.
    BackwardOnly,
    /// Breaking in all directions.
    Breaking,
}

/// Resolve a `.proto` scalar type name to a `FieldType` (None for message/enum).
pub fn scalar_field_type(name: &str) -> Option<FieldType> {
    FieldType::from_proto_name(name)
}

/// The wire type for a `(name, field_type)` pair. The explicit `FieldType` is
/// authoritative when present (so an `enum` field — whose `FieldType::Enum`
/// has a Varint wire type — is classified correctly even though its type *name*
/// is not a scalar). When no `FieldType` is given we fall back to resolving the
/// scalar name; an unresolvable name (message/enum without a `FieldType`) is
/// treated as length-delimited (the conservative default for a bare reference).
fn wire_of_field(name: &str, field_type: Option<FieldType>) -> WireType {
    match field_type {
        Some(t) => t.wire_type(),
        None => match scalar_field_type(name) {
            Some(t) => t.wire_type(),
            None => WireType::LengthDelimited,
        },
    }
}

/// Whether a type-name change crosses the protobuf wire-type boundary, using the
/// real `FieldType` of each side when known.
pub fn is_cross_wire_type_typed(
    from: &str,
    from_ty: Option<FieldType>,
    to: &str,
    to_ty: Option<FieldType>,
) -> bool {
    wire_of_field(from, from_ty) != wire_of_field(to, to_ty)
}

/// Classify a type change against the wire-type boundary and the allowlist,
/// using the real `FieldType` of each side when known. The allowlist is keyed on
/// the `.proto` type names (scalar names, or `enum` for enum fields).
pub fn classify_type_change_typed(
    from: &str,
    from_ty: Option<FieldType>,
    to: &str,
    to_ty: Option<FieldType>,
) -> TypeChangeCompat {
    if from == to {
        return TypeChangeCompat::FullyCompat;
    }
    if is_cross_wire_type_typed(from, from_ty, to, to_ty) {
        return TypeChangeCompat::Breaking;
    }
    // Same wire type — apply the allowlist, normalizing enum fields to "enum"
    // (their `.proto` type name is the enum name, not a scalar keyword).
    let from_key = allowlist_key(from, from_ty);
    let to_key = allowlist_key(to, to_ty);
    const FULLY: &[(&str, &str)] = &[
        ("int32", "int64"),
        ("uint32", "uint64"),
        ("sint32", "sint64"),
        ("string", "bytes"),
        ("bytes", "string"),
        ("int32", "uint32"),
        ("int32", "bool"),
        ("uint32", "int32"),
        // enum and int32/uint32/... share the varint wire type; enum→int32 (and
        // the reverse) is FULL-compatible.
        ("enum", "int32"),
        ("int32", "enum"),
        ("enum", "uint32"),
        ("uint32", "enum"),
        ("enum", "int64"),
        ("enum", "uint64"),
    ];
    const BACKWARD_ONLY: &[(&str, &str)] = &[
        ("int64", "int32"),
        ("sint64", "sint32"),
        ("uint64", "uint32"),
    ];
    if FULLY.iter().any(|(a, b)| from_key == *a && to_key == *b) {
        return TypeChangeCompat::FullyCompat;
    }
    if BACKWARD_ONLY
        .iter()
        .any(|(a, b)| from_key == *a && to_key == *b)
    {
        return TypeChangeCompat::BackwardOnly;
    }
    TypeChangeCompat::Breaking
}

/// The allowlist key for a `(name, field_type)` pair: the scalar `.proto` name,
/// or `"enum"` when the field is an enum (whose own name is the type name).
fn allowlist_key(name: &str, field_type: Option<FieldType>) -> &str {
    match field_type {
        Some(FieldType::Enum) => "enum",
        _ => name,
    }
}

/// Whether a mutation may change a field's type (used by `ChangeFieldType`),
/// using the real `FieldType` of each side when known. Cross-wire and
/// non-allowlisted same-wire changes are rejected.
pub fn type_change_allowed_typed(
    from: &str,
    from_ty: Option<FieldType>,
    to: &str,
    to_ty: Option<FieldType>,
) -> Result<(), String> {
    if from == to {
        return Ok(());
    }
    if is_cross_wire_type_typed(from, from_ty, to, to_ty) {
        return Err(format!(
            "type change '{from}'→'{to}' crosses the wire-type boundary (always breaking)"
        ));
    }
    match classify_type_change_typed(from, from_ty, to, to_ty) {
        TypeChangeCompat::Breaking => Err(format!(
            "type change '{from}'→'{to}' is not in the allowlist"
        )),
        _ => Ok(()),
    }
}
