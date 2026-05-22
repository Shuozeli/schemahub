mod helpers;
use helpers::*;
use bytes::Bytes;
use schemahub_core::CoreError;
use schemahub_types::SchemaPath;

const PROTO_A: &str = r#"syntax = "proto3"; message A { uint64 id = 1; }"#;
const PROTO_B: &str = r#"syntax = "proto3"; message B { string name = 1; }"#;

// Helper: create two schemas on main, return the head after both are created.
fn setup_two_schemas(core: &schemahub_core::Core) -> String {
    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let h = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();
    core.create_schema(
        "acme", "schemas", "main",
        "b.proto", PROTO_B.as_bytes(), "protobuf",
        &h, &idem(), "test", None,
    ).unwrap();
    core.get_branch_head("acme", "schemas", "main").unwrap().to_hex()
}

// ── apply_mutations (batch) ───────────────────────────────────────────────────

#[test]
fn batch_empty_mutations_returns_invalid_argument() {
    let core = make_core_with_repo("acme", "schemas");

    let req = BatchMutateRequest {
        project: "acme".to_string(),
        repo: "schemas".to_string(),
        branch: "main".to_string(),
        base_revision: String::new(),
        idempotency_key: idem(),
        force: false,
        mutations: vec![],
        token: None,
        author: "test".to_string(),
    };

    let err = core.apply_mutations(req).unwrap_err();
    assert!(matches!(err, CoreError::InvalidArgument(_)));
}

#[test]
fn batch_mixed_formats_returns_invalid_argument() {
    let core = make_core_with_repo("acme", "schemas");

    let proto_m = schemahub_types::Mutation {
        schema_path: SchemaPath::new("acme".to_string(), "schemas".to_string(), "a.proto".to_string()),
        format_id: "protobuf".to_string(),
        declaration_name: "A".to_string(),
        operation: Bytes::new(),
    };
    let fbs_m = schemahub_types::Mutation {
        schema_path: SchemaPath::new("acme".to_string(), "schemas".to_string(), "b.fbs".to_string()),
        format_id: "flatbuffers".to_string(),
        declaration_name: "B".to_string(),
        operation: Bytes::new(),
    };

    let req = BatchMutateRequest {
        project: "acme".to_string(),
        repo: "schemas".to_string(),
        branch: "main".to_string(),
        base_revision: String::new(),
        idempotency_key: idem(),
        force: false,
        mutations: vec![proto_m, fbs_m],
        token: None,
        author: "test".to_string(),
    };

    let err = core.apply_mutations(req).unwrap_err();
    assert!(matches!(err, CoreError::InvalidArgument(_)), "expected InvalidArgument, got {err:?}");
}

#[test]
fn batch_stale_base_revision_returns_conflict() {
    let core = make_core_with_repo("acme", "schemas");
    let stale_head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Advance main.
    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let m = Mutation {
        schema_path: SchemaPath::new("acme".to_string(), "schemas".to_string(), "a.proto".to_string()),
        format_id: "protobuf".to_string(),
        declaration_name: "A".to_string(),
        operation: Bytes::new(),
    };

    let req = BatchMutateRequest {
        project: "acme".to_string(),
        repo: "schemas".to_string(),
        branch: "main".to_string(),
        base_revision: stale_head, // stale
        idempotency_key: idem(),
        force: false,
        mutations: vec![m],
        token: None,
        author: "test".to_string(),
    };

    let err = core.apply_mutations(req).unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }));
}

#[test]
fn batch_idempotency_returns_same_commit() {
    // We need a real mutation that the plugin can apply.
    // The simplest is an AddField protobuf mutation. Build it via the protobuf plugin directly.
    let core = make_core_with_repo("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();
    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    // Build an AddField mutation for message A.
    let add_field_op = build_add_field_proto_op("a.proto", "A", "label", 2);
    let key = idem();

    let r1 = core.apply_mutations(BatchMutateRequest {
        project: "acme".to_string(),
        repo: "schemas".to_string(),
        branch: "main".to_string(),
        base_revision: head.clone(),
        idempotency_key: key.clone(),
        force: false,
        mutations: vec![add_field_op.clone()],
        token: None,
        author: "test".to_string(),
    }).unwrap();

    let r2 = core.apply_mutations(BatchMutateRequest {
        project: "acme".to_string(),
        repo: "schemas".to_string(),
        branch: "main".to_string(),
        base_revision: head.clone(),
        idempotency_key: key.clone(),
        force: false,
        mutations: vec![add_field_op],
        token: None,
        author: "test".to_string(),
    }).unwrap();

    assert_eq!(r1, r2, "batch idempotency key should return same commit");
}

// ── apply_mutation (single) ───────────────────────────────────────────────────

#[test]
fn single_mutation_add_field_happy_path() {
    let core = make_core_with_repo("acme", "schemas");
    core.create_schema(
        "acme", "schemas", "main",
        "a.proto", PROTO_A.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "schemas", "main").unwrap().to_hex();

    let add_field_op = build_add_field_proto_op("a.proto", "A", "label", 2);
    let new_commit = core.apply_mutations(BatchMutateRequest {
        project: "acme".to_string(),
        repo: "schemas".to_string(),
        branch: "main".to_string(),
        base_revision: head,
        idempotency_key: idem(),
        force: false,
        mutations: vec![add_field_op],
        token: None,
        author: "test".to_string(),
    }).unwrap();

    let new_head = core.get_branch_head("acme", "schemas", "main").unwrap();
    assert_eq!(new_head.to_hex(), new_commit);

    // The declaration should now have the new field.
    let detail = core.get_declaration("acme", "schemas", "a.proto", "A", &new_commit).unwrap();
    assert!(detail.is_some());
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Encode a ProtoOperationEnvelope::AddField mutation using prost encoding primitives.
/// Operation bytes format: ProtoOperationEnvelope { tag: varint (field 1), payload: bytes (field 2) }
/// where payload is OpAddField { field_name: string (1), field_type: string (2), field_number: uint32 (3) }.
fn build_add_field_proto_op(
    schema_name: &str,
    message_name: &str,
    field_name: &str,
    field_number: u32,
) -> Mutation {
    use prost::encoding::{encode_key, encode_varint, WireType};

    fn encode_string_field(tag: u32, value: &str, buf: &mut Vec<u8>) {
        encode_key(tag, WireType::LengthDelimited, buf);
        let bytes = value.as_bytes();
        encode_varint(bytes.len() as u64, buf);
        buf.extend_from_slice(bytes);
    }

    fn encode_varint_field(tag: u32, value: u64, buf: &mut Vec<u8>) {
        encode_key(tag, WireType::Varint, buf);
        encode_varint(value, buf);
    }

    // Build OpAddField: field_name (1), field_type (2), field_number (3).
    // repeated (4) and doc_comment (5) are omitted (default values).
    let mut op_bytes = Vec::new();
    encode_string_field(1, field_name, &mut op_bytes);
    encode_string_field(2, "string", &mut op_bytes);
    encode_varint_field(3, field_number as u64, &mut op_bytes);

    // Build ProtoOperationEnvelope: tag=ADD_FIELD=1 (varint, field 1), payload=bytes (field 2).
    let mut envelope = Vec::new();
    encode_varint_field(1, 1u64, &mut envelope); // tag = op_tag::ADD_FIELD = 1
    encode_key(2, WireType::LengthDelimited, &mut envelope);
    encode_varint(op_bytes.len() as u64, &mut envelope);
    envelope.extend_from_slice(&op_bytes);

    Mutation {
        schema_path: SchemaPath::new("acme".to_string(), "schemas".to_string(), schema_name.to_string()),
        format_id: "protobuf".to_string(),
        declaration_name: message_name.to_string(),
        operation: Bytes::from(envelope),
    }
}
