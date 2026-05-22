/// Integration tests: search index stays in sync with schema lifecycle operations.
mod helpers;
use helpers::*;
use schemahub_storage::keys;

const PROTO_V1: &str = r#"syntax = "proto3"; message Order { uint64 id = 1; string name = 2; }"#;
const PROTO_V2: &str = r#"syntax = "proto3"; message Order { uint64 id = 1; string name = 2; bool active = 3; } enum Status { UNKNOWN = 0; ACTIVE = 1; }"#;

fn search_keys(core: &schemahub_core::Core, decl: &str, project: &str, repo: &str) -> Vec<String> {
    let prefix = keys::search_prefix_repo(decl, project, repo);
    core.storage
        .scan_prefix(&prefix)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

#[test]
fn create_schema_writes_search_entries() {
    let core = make_core_with_repo("acme", "orders");

    core.create_schema(
        "acme", "orders", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // "Order" declaration must appear in the search index.
    let hits = search_keys(&core, "Order", "acme", "orders");
    assert!(!hits.is_empty(), "search entry for 'Order' should exist after create_schema");
}

#[test]
fn update_schema_refreshes_search_entries() {
    let core = make_core_with_repo("acme", "orders");

    core.create_schema(
        "acme", "orders", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    let head = core.get_branch_head("acme", "orders", "main").unwrap().to_hex();

    // V2 adds a new enum "Status".
    core.update_schema(
        "acme", "orders", "main",
        "order.proto", PROTO_V2.as_bytes(),
        &head, &idem(), false, "test", None,
    ).unwrap();

    // "Order" still present; new "Status" entry added.
    let order_hits = search_keys(&core, "Order", "acme", "orders");
    assert!(!order_hits.is_empty(), "search entry for 'Order' should still exist after update");

    let status_hits = search_keys(&core, "Status", "acme", "orders");
    assert!(!status_hits.is_empty(), "search entry for 'Status' should exist after update adds it");
}

#[test]
fn delete_schema_removes_search_entries() {
    let core = make_core_with_repo("acme", "orders");

    core.create_schema(
        "acme", "orders", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // Confirm entry is present.
    let before = search_keys(&core, "Order", "acme", "orders");
    assert!(!before.is_empty(), "search entry should exist before delete");

    let head = core.get_branch_head("acme", "orders", "main").unwrap().to_hex();
    core.delete_schema(
        "acme", "orders", "main",
        "order.proto", &head, &idem(), false, "test", None,
    ).unwrap();

    let after = search_keys(&core, "Order", "acme", "orders");
    assert!(after.is_empty(), "search entry for 'Order' should be gone after delete_schema");
}

#[test]
fn apply_mutation_updates_search_entries() {
    use schemahub_core::mutation::single::MutateRequest;
    use schemahub_types::Mutation;

    let core = make_core_with_repo("acme", "orders");

    core.create_schema(
        "acme", "orders", "main",
        "order.proto", PROTO_V1.as_bytes(), "protobuf",
        "", &idem(), "test", None,
    ).unwrap();

    // "Order" should be indexed.
    let before = search_keys(&core, "Order", "acme", "orders");
    assert!(!before.is_empty());

    // Apply AddMessage mutation to add "Invoice".
    use schemahub_plugin_protobuf::operations::{op_tag, ProtoOperationEnvelope, OpAddMessage};
    use prost::Message as _;

    let op = OpAddMessage {
        message_name: "Invoice".to_string(),
        doc_comment: String::new(),
    };
    let op_bytes = ProtoOperationEnvelope::encode_op(op_tag::ADD_MESSAGE, op.encode_to_vec());

    let schema_path = schemahub_types::SchemaPath::new("acme", "orders", "order.proto");
    let mutation = Mutation {
        format_id: "protobuf".to_string(),
        schema_path: schema_path.clone(),
        declaration_name: String::new(),
        operation: op_bytes.into(),
    };

    let head = core.get_branch_head("acme", "orders", "main").unwrap().to_hex();
    core.apply_mutation(MutateRequest {
        project: "acme".to_string(),
        repo: "orders".to_string(),
        branch: "main".to_string(),
        base_revision: head,
        idempotency_key: idem(),
        force: false,
        mutation,
        token: None,
        author: "test".to_string(),
    }).unwrap();

    // "Invoice" should now be in the search index.
    let invoice_hits = search_keys(&core, "Invoice", "acme", "orders");
    assert!(!invoice_hits.is_empty(), "search entry for 'Invoice' should exist after AddMessage mutation");
}
