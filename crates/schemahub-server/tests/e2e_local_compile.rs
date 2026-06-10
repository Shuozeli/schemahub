//! End-to-end local compile smoke tests.
//!
//! These tests drive the real in-process gRPC server, create schemas, request
//! Rust codegen via `PreviewCodegen`, then place the generated source into a
//! temporary Cargo project and run `cargo check`. This covers the cloud-build
//! path more directly than string assertions: SchemaHub output must compile as a
//! downstream build input.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::*;
use schemahub_api::schemahub_v1 as pb;

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

fn write_compile_crate(project_dir: &Path, cargo_toml: &str, generated: &[u8]) {
    fs::create_dir_all(project_dir.join("src")).expect("create temp crate src");
    fs::write(project_dir.join("Cargo.toml"), cargo_toml).expect("write Cargo.toml");
    fs::write(
        project_dir.join("src/lib.rs"),
        b"#![allow(warnings)]\ninclude!(\"generated.rs\");\n",
    )
    .expect("write lib.rs");
    fs::write(project_dir.join("src/generated.rs"), generated).expect("write generated.rs");
}

fn integration_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/integration")
        .join(name);
    fs::read_to_string(path).expect("read integration fixture")
}

#[tokio::test]
async fn protobuf_preview_codegen_compiles_in_local_cargo_project() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let src = r#"syntax = "proto3";
package local.compile.v1;

message BuildEvent {
  string id = 1;
  int64 sequence = 2;
  repeated string labels = 3;
}
"#;
    create_schema(
        &mut c.schema,
        "local",
        "compile",
        "main",
        "build_event.proto",
        pb::SchemaFormat::Protobuf,
        src,
        "local-compile-protobuf",
    )
    .await;

    // Act.
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "local".into(),
            repo: "compile".into(),
            schema_path: "build_event.proto".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
        })
        .await
        .expect("preview protobuf rust codegen")
        .into_inner();

    // Assert: generated code compiles in a fresh local Cargo project.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-protobuf-local-compile"
version = "0.0.0"
edition = "2021"

[dependencies]
prost = "0.13"
"#,
        &preview.content,
    );
    cargo_check(tmp.path());
}

#[tokio::test]
async fn protobuf_import_closure_preview_codegen_compiles_in_local_cargo_project() {
    // Arrange: root.proto imports common.proto using SchemaHub's logical
    // project/repo/schema path. PreviewCodegen must compile the full closure,
    // not only the root schema.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let common_src = r#"syntax = "proto3";
package local.compile.v1;

message BuildMetadata {
  string source = 1;
  int64 revision = 2;
}
"#;
    let root_src = r#"syntax = "proto3";
package local.compile.v1;

import "local/compile/common.proto";

message BuildEnvelope {
  string id = 1;
  BuildMetadata metadata = 2;
}
"#;
    create_schema(
        &mut c.schema,
        "local",
        "compile",
        "main",
        "common.proto",
        pb::SchemaFormat::Protobuf,
        common_src,
        "local-compile-protobuf-common",
    )
    .await;
    create_schema(
        &mut c.schema,
        "local",
        "compile",
        "main",
        "root.proto",
        pb::SchemaFormat::Protobuf,
        root_src,
        "local-compile-protobuf-root",
    )
    .await;

    // Act.
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "local".into(),
            repo: "compile".into(),
            schema_path: "root.proto".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
        })
        .await
        .expect("preview protobuf closure rust codegen")
        .into_inner();

    // Assert.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-protobuf-closure-local-compile"
version = "0.0.0"
edition = "2021"

[dependencies]
prost = "0.13"
"#,
        &preview.content,
    );
    cargo_check(tmp.path());
}

#[tokio::test]
async fn flatbuffers_preview_codegen_compiles_in_local_cargo_project() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let src = r#"namespace local.compile;

table BuildRecord {
  id: string;
  count: int;
}

root_type BuildRecord;
"#;
    create_schema(
        &mut c.schema,
        "local",
        "compile",
        "main",
        "build_record.fbs",
        pb::SchemaFormat::Flatbuffers,
        src,
        "local-compile-flatbuffers",
    )
    .await;

    // Act.
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "local".into(),
            repo: "compile".into(),
            schema_path: "build_record.fbs".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
        })
        .await
        .expect("preview flatbuffers rust codegen")
        .into_inner();

    // Assert: generated code compiles in a fresh local Cargo project.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-flatbuffers-local-compile"
version = "0.0.0"
edition = "2021"

[dependencies]
flatbuffers = "25.12.19"
"#,
        &preview.content,
    );
    cargo_check(tmp.path());
}

#[tokio::test]
async fn flatbuffers_include_closure_preview_codegen_compiles_in_local_cargo_project() {
    // Arrange: root.fbs includes common.fbs using a SchemaHub logical path.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let common_src = r#"namespace local.compile;

table BuildOwner {
  id: string;
  email: string;
}

root_type BuildOwner;
"#;
    let root_src = r#"include "local/compile/common.fbs";

namespace local.compile;

table BuildRecord {
  id: string;
  owner: BuildOwner;
  count: int;
}

root_type BuildRecord;
"#;
    create_schema(
        &mut c.schema,
        "local",
        "compile",
        "main",
        "common.fbs",
        pb::SchemaFormat::Flatbuffers,
        common_src,
        "local-compile-flatbuffers-common",
    )
    .await;
    create_schema(
        &mut c.schema,
        "local",
        "compile",
        "main",
        "root.fbs",
        pb::SchemaFormat::Flatbuffers,
        root_src,
        "local-compile-flatbuffers-root",
    )
    .await;

    // Act.
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "local".into(),
            repo: "compile".into(),
            schema_path: "root.fbs".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
        })
        .await
        .expect("preview flatbuffers closure rust codegen")
        .into_inner();

    // Assert.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-flatbuffers-closure-local-compile"
version = "0.0.0"
edition = "2021"

[dependencies]
flatbuffers = "25.12.19"
"#,
        &preview.content,
    );
    cargo_check(tmp.path());
}

#[tokio::test]
async fn rich_flatbuffers_include_closure_preview_codegen_compiles_and_reports_bundle_contract() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let common_src = integration_fixture("rich_catalog_common.fbs");
    let root_src = integration_fixture("rich_catalog.fbs");
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "catalog/common.fbs",
        pb::SchemaFormat::Flatbuffers,
        &common_src,
        "local-compile-rich-flatbuffers-common",
    )
    .await;
    create_schema(
        &mut c.schema,
        "acme",
        "core",
        "main",
        "catalog/rich_catalog.fbs",
        pb::SchemaFormat::Flatbuffers,
        &root_src,
        "local-compile-rich-flatbuffers-root",
    )
    .await;

    // Act.
    let descriptors = c
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "catalog/rich_catalog.fbs".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("get flatbuffers descriptor bundle")
        .into_inner();
    let preview = c
        .codegen
        .preview_codegen(pb::PreviewCodegenRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "catalog/rich_catalog.fbs".into(),
            at: Some(vref_branch("main")),
            language: pb::Language::Rust as i32,
        })
        .await
        .expect("preview rich flatbuffers closure rust codegen")
        .into_inner();

    // Assert: descriptors are the deterministic source bundle for the include
    // closure, while PreviewCodegen is a single generated Rust artifact.
    assert_eq!(
        descriptors.format,
        pb::SchemaFormat::Flatbuffers as i32,
        "descriptor format should identify FlatBuffers"
    );
    assert!(
        !descriptors.at_commit.is_empty(),
        "descriptor bundle should report resolved commit"
    );
    let descriptor_bundle =
        String::from_utf8(descriptors.descriptor_bytes).expect("descriptor bundle is utf-8");
    let common_marker = "// ── acme/core/catalog/common.fbs ──";
    let root_marker = "// ── acme/core/catalog/rich_catalog.fbs ──";
    let common_pos = descriptor_bundle
        .find(common_marker)
        .expect("bundle should include common fixture marker");
    let root_pos = descriptor_bundle
        .find(root_marker)
        .expect("bundle should include root fixture marker");
    assert!(
        common_pos < root_pos,
        "bundle entries should be sorted by SchemaHub path:\n{descriptor_bundle}"
    );
    assert!(
        descriptor_bundle.contains("table Money"),
        "bundle should include declarations from included file:\n{descriptor_bundle}"
    );
    assert!(
        descriptor_bundle.contains("union CatalogEntity"),
        "bundle should include rich root declarations:\n{descriptor_bundle}"
    );
    assert!(
        !preview.is_archive,
        "preview codegen currently returns one Rust source artifact"
    );
    assert!(
        !preview.at_commit.is_empty(),
        "preview should report resolved commit"
    );
    let code = String::from_utf8(preview.content.clone()).expect("generated rust is utf-8");
    for symbol in [
        "Money",
        "Supplier",
        "CatalogItem",
        "CatalogGroup",
        "CatalogEntity",
        "CatalogEnvelope",
    ] {
        assert!(
            code.contains(symbol),
            "generated Rust should contain {symbol}; got:\n{code}"
        );
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-rich-flatbuffers-closure-local-compile"
version = "0.0.0"
edition = "2021"

[dependencies]
flatbuffers = "25.12.19"
"#,
        &preview.content,
    );
    cargo_check(tmp.path());
}
