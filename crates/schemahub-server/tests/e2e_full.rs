//! Full end-to-end workflow test for SchemaHub's JJ-backed registry path.
//!
//! This intentionally exercises the real in-process gRPC server as a user
//! journey instead of isolating one feature: create schemas with an import
//! closure, tag a release snapshot, branch, mutate, merge, diff, inspect
//! history, request descriptors/codegen, and compile the generated Rust.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::*;
use schemahub_api::schemahub_v1 as pb;

const MONEY_PROTO: &str = r#"syntax = "proto3";
package commerce.v1;

message Money {
  string currency_code = 1;
  int64 units = 2;
  int32 nanos = 3;
}
"#;

const ORDER_PROTO: &str = r#"syntax = "proto3";
package commerce.v1;

import "acme/commerce/common.proto";

message Order {
  string id = 1;
  Money total = 2;
}
"#;

fn cargo_check(project_dir: &Path) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .arg("check")
        .arg("--quiet")
        .env("CARGO_TARGET_DIR", project_dir.join("target"))
        .current_dir(project_dir)
        .output()
        .expect("run cargo check");

    assert!(
        output.status.success(),
        "cargo check failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_compile_crate(project_dir: &Path, generated: &[u8]) {
    fs::create_dir_all(project_dir.join("src")).expect("create temp crate src");
    fs::write(
        project_dir.join("Cargo.toml"),
        r#"[package]
name = "schemahub-full-e2e-codegen"
version = "0.0.0"
edition = "2021"

[dependencies]
prost = "0.13"
"#,
    )
    .expect("write Cargo.toml");
    fs::write(
        project_dir.join("src/lib.rs"),
        b"#![allow(warnings)]\ninclude!(\"generated.rs\");\n",
    )
    .expect("write lib.rs");
    fs::write(project_dir.join("src/generated.rs"), generated).expect("write generated.rs");
}

fn add_string_field(
    schema_path: &str,
    message_name: &str,
    field_name: &str,
    field_number: u32,
    idempotency_key: &str,
    branch: &str,
) -> pb::ApplyMutationRequest {
    pb::ApplyMutationRequest {
        project: "acme".into(),
        repo: "commerce".into(),
        branch: branch.into(),
        base_revision: String::new(),
        idempotency_key: idempotency_key.into(),
        force: false,
        operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
            pb::ProtobufMutation {
                schema_path: schema_path.into(),
                operation: Some(pb::protobuf_mutation::Operation::AddField(
                    pb::ProtoAddField {
                        message_name: message_name.into(),
                        field_name: field_name.into(),
                        field_type: "string".into(),
                        field_number,
                        repeated: false,
                        doc_comment: String::new(),
                    },
                )),
            },
        )),
    }
}

#[tokio::test]
async fn complete_protobuf_registry_workflow_compiles_generated_output() {
    // Arrange: start the real server and create a two-file Protobuf schema set
    // where order.proto imports common.proto by SchemaHub logical path.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let money_create = create_schema(
        &mut c.schema,
        "acme",
        "commerce",
        "main",
        "common.proto",
        pb::SchemaFormat::Protobuf,
        MONEY_PROTO,
        "full-create-money",
    )
    .await;
    let order_create = create_schema(
        &mut c.schema,
        "acme",
        "commerce",
        "main",
        "order.proto",
        pb::SchemaFormat::Protobuf,
        ORDER_PROTO,
        "full-create-order",
    )
    .await;
    assert!(!money_create.new_commit.is_empty());
    assert!(!order_create.new_commit.is_empty());

    let release = c
        .refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            name: "release-2026-06-04".into(),
            target: Some(vref_branch("main")),
            message: "full e2e release snapshot".into(),
        })
        .await
        .expect("create release tag")
        .into_inner()
        .tag
        .expect("release tag");
    assert!(!release.commit_hash.is_empty());

    c.refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            name: "feature/shipping-note".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create feature branch");

    // Act: mutate the feature branch, merge it back to main, then request
    // history/diff/artifacts at the resulting main branch.
    let feature_write = c
        .schema
        .apply_mutation(add_string_field(
            "order.proto",
            "Order",
            "shipping_note",
            3,
            "full-add-shipping-note",
            "feature/shipping-note",
        ))
        .await
        .expect("add field on feature branch")
        .into_inner();
    assert!(!feature_write.new_commit.is_empty());

    let main_before_merge = pull_source(
        &mut c.explore,
        "acme",
        "commerce",
        "order.proto",
        vref_branch("main"),
    )
    .await;
    assert!(
        !main_before_merge.contains("shipping_note"),
        "main should not move before merge, got:\n{main_before_merge}"
    );

    let merge = c
        .refs
        .merge(pb::MergeRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            source_branch: "feature/shipping-note".into(),
            target_branch: "main".into(),
            base_revision: String::new(),
            idempotency_key: "full-merge-shipping-note".into(),
            message: "merge shipping note".into(),
        })
        .await
        .expect("merge feature branch")
        .into_inner();
    assert!(!merge.new_commit.is_empty());

    let main_after_merge = pull_source(
        &mut c.explore,
        "acme",
        "commerce",
        "order.proto",
        vref_branch("main"),
    )
    .await;
    let release_source = pull_source(
        &mut c.explore,
        "acme",
        "commerce",
        "order.proto",
        vref_tag("release-2026-06-04"),
    )
    .await;

    let diff = c
        .refs
        .diff(pb::DiffRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            base: Some(vref_tag("release-2026-06-04")),
            head: Some(vref_branch("main")),
            schema_path: "order.proto".into(),
        })
        .await
        .expect("diff release to main")
        .into_inner();

    let log = c
        .history
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            at: Some(vref_branch("main")),
            limit: 0,
        })
        .await
        .expect("history log")
        .into_inner();
    let oplog = c
        .history
        .op_log(pb::OpLogRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            limit: 0,
        })
        .await
        .expect("operation log")
        .into_inner();

    let descriptors = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            schema_path: "order.proto".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("get descriptor closure")
        .into_inner();
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "acme".into(),
            repo: "commerce".into(),
            schema_path: "order.proto".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
            rust_pluggable_buffer: false,
        })
        .await
        .expect("preview rust codegen")
        .into_inner();

    // Assert: branch/tag/JJ history semantics and compiler artifacts are all
    // observable through public gRPC APIs.
    assert!(
        main_after_merge.contains("shipping_note"),
        "main should contain merged field, got:\n{main_after_merge}"
    );
    assert!(
        !release_source.contains("shipping_note"),
        "release tag should remain pinned before feature merge, got:\n{release_source}"
    );
    assert!(
        diff.schema_diffs
            .iter()
            .flat_map(|schema_diff| schema_diff.changes.iter())
            .any(|change| change.decl_name == "Order" && change.change_type == "modified"),
        "diff should report modified Order declaration, got: {:?}",
        diff.schema_diffs
    );
    assert!(
        log.entries.len() >= 4,
        "create common + create order + feature write + merge should produce commits, got {}",
        log.entries.len()
    );
    assert!(
        oplog.operations.len() >= 5,
        "creates, tag, branch, feature write, and merge should produce operations, got {}",
        oplog.operations.len()
    );
    assert_eq!(descriptors.format, pb::SchemaFormat::Protobuf as i32);
    assert!(
        !descriptors.descriptor_bytes.is_empty(),
        "descriptor closure should be non-empty"
    );
    assert!(
        !descriptors.at_commit.is_empty(),
        "descriptor response should report resolved commit"
    );
    assert!(!preview.is_archive, "Rust preview is one source artifact");
    assert!(
        !preview.at_commit.is_empty(),
        "preview response should report resolved commit"
    );
    let code = String::from_utf8(preview.content.clone()).expect("generated rust is utf-8");
    assert!(
        code.contains("Money"),
        "generated code should include import closure:\n{code}"
    );
    assert!(
        code.contains("Order"),
        "generated code should include root message:\n{code}"
    );
    assert!(
        code.contains("shipping_note"),
        "generated code should include merged field:\n{code}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(tmp.path(), &preview.content);
    cargo_check(tmp.path());
}
