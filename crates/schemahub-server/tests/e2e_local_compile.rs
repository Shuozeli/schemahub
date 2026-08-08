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

fn flatbuffers_git_coordinate() -> (String, String) {
    let manifest: toml::Value =
        toml::from_str(include_str!("../../../Cargo.toml")).expect("parse workspace Cargo.toml");
    let dependency = &manifest["workspace"]["dependencies"]["flatc-rs-codegen"];
    let repository = dependency["git"]
        .as_str()
        .expect("flatc-rs-codegen Git repository");
    let revision = dependency["rev"]
        .as_str()
        .expect("flatc-rs-codegen immutable revision");
    (repository.to_owned(), revision.to_owned())
}

fn preview_request(
    project: &str,
    repo: &str,
    schema_path: &str,
    language: pb::Language,
) -> pb::PreviewCodegenRequest {
    pb::PreviewCodegenRequest {
        project: project.into(),
        repo: repo.into(),
        schema_path: schema_path.into(),
        at: Some(vref_branch("main")),
        language: language as i32,
        rust_pluggable_buffer: false,
    }
}

#[tokio::test]
async fn immutable_serving_rust_artifact_compiles_in_local_cargo_project() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let source = r#"syntax = "proto3";
package immutable.compile.v1;

message StoredEvent {
  string id = 1;
  int64 sequence = 2;
}
"#;
    create_schema(
        &mut c.schema,
        "immutable",
        "compile",
        "main",
        "stored_event.proto",
        pb::SchemaFormat::Protobuf,
        source,
        "immutable-serving-compile",
    )
    .await;
    let revision = c
        .serving
        .resolve_revision(pb::ResolveRevisionRequest {
            parent: "projects/immutable/repos/compile".to_string(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("resolve immutable revision")
        .into_inner();

    // Act.
    let artifact = c
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: revision.name,
            schema_path: "stored_event.proto".to_string(),
            kind: pb::SchemaArtifactKind::GeneratedCode as i32,
            language: pb::Language::Rust as i32,
            ..Default::default()
        })
        .await
        .expect("fetch immutable generated code")
        .into_inner();

    // Assert.
    assert!(artifact.artifact_digest.starts_with("sha256:"));
    assert!(artifact.closure_digest.starts_with("sha256:"));
    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-immutable-serving-compile"
version = "0.0.0"
edition = "2021"

[dependencies]
prost = "0.13"
"#,
        &artifact.content,
    );
    cargo_check(tmp.path());
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
        .preview_codegen(preview_request(
            "local",
            "compile",
            "build_event.proto",
            pb::Language::Rust,
        ))
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
        .preview_codegen(preview_request(
            "local",
            "compile",
            "root.proto",
            pb::Language::Rust,
        ))
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
async fn rich_protobuf_three_level_closure_codegen_compiles_in_local_cargo_project() {
    // Arrange: three files in one package exercise transitive imports together
    // with maps, nested messages, oneofs, proto3 optional, bytes, and streaming
    // service declarations.
    let url = start_server().await;
    let mut c = clients(&url).await;
    for (path, fixture, idempotency_key) in [
        (
            "common.proto",
            "complex_common.proto",
            "complex-protobuf-common",
        ),
        (
            "customer.proto",
            "complex_customer.proto",
            "complex-protobuf-customer",
        ),
        (
            "order.proto",
            "complex_order.proto",
            "complex-protobuf-order",
        ),
    ] {
        let source = integration_fixture(fixture);
        create_schema(
            &mut c.schema,
            "complex",
            "protobuf",
            "main",
            path,
            pb::SchemaFormat::Protobuf,
            &source,
            idempotency_key,
        )
        .await;
    }

    // Act.
    let preview = c
        .codegen
        .preview_codegen(preview_request(
            "complex",
            "protobuf",
            "order.proto",
            pb::Language::Rust,
        ))
        .await
        .expect("preview rich protobuf closure rust codegen")
        .into_inner();

    // Assert: every layer of the closure contributes generated types, and the
    // complete artifact compiles as downstream Rust code.
    let code = String::from_utf8(preview.content.clone()).expect("generated rust is utf-8");
    for symbol in [
        "struct Money",
        "enum Region",
        "struct Customer",
        "struct Address",
        "enum Contact",
        "struct Order",
        "struct LineItem",
        "enum Settlement",
        "struct WatchOrdersRequest",
    ] {
        assert!(
            code.contains(symbol),
            "generated protobuf closure lost `{symbol}`:\n{code}"
        );
    }
    assert!(
        !preview.at_commit.is_empty(),
        "complex protobuf preview should report a concrete commit"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-rich-protobuf-closure-local-compile"
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
async fn proto2_defaults_extensions_and_service_codegen_compiles_in_local_cargo_project() {
    // Arrange.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let source = integration_fixture("legacy_account.proto");
    create_schema(
        &mut c.schema,
        "legacy",
        "protobuf",
        "main",
        "legacy_account.proto",
        pb::SchemaFormat::Protobuf,
        &source,
        "local-compile-protobuf-proto2",
    )
    .await;

    // Act.
    let preview = c
        .codegen
        .preview_codegen(preview_request(
            "legacy",
            "protobuf",
            "legacy_account.proto",
            pb::Language::Rust,
        ))
        .await
        .expect("preview proto2 rust codegen")
        .into_inner();

    // Assert.
    let code = String::from_utf8(preview.content.clone()).expect("generated rust is utf-8");
    assert!(
        code.contains("struct LegacyAccount"),
        "proto2 message missing from generated Rust:\n{code}"
    );
    assert!(
        code.contains("enum State"),
        "nested proto2 enum missing from generated Rust:\n{code}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-proto2-local-compile"
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
async fn cross_package_protobuf_closure_codegen_compiles_in_local_cargo_project() {
    // Arrange: three files in three distinct packages form an import chain
    // (order.v1 -> user.v1 -> common.v1) where every cross-file reference is
    // written as a fully-qualified name from a different package. This exercises
    // closure symbol resolution across package boundaries and the downstream
    // generator's cross-module type paths, which single-package closure tests do
    // not cover.
    let url = start_server().await;
    let mut c = clients(&url).await;
    for (path, fixture, idempotency_key) in [
        (
            "common/types.proto",
            "cloudbuild_common.proto",
            "cross-package-protobuf-common",
        ),
        (
            "user/profile.proto",
            "cloudbuild_user.proto",
            "cross-package-protobuf-user",
        ),
        (
            "order/purchase.proto",
            "cloudbuild_order.proto",
            "cross-package-protobuf-order",
        ),
    ] {
        let source = integration_fixture(fixture);
        create_schema(
            &mut c.schema,
            "acme",
            "core",
            "main",
            path,
            pb::SchemaFormat::Protobuf,
            &source,
            idempotency_key,
        )
        .await;
    }

    // Act.
    let preview = c
        .codegen
        .preview_codegen(preview_request(
            "acme",
            "core",
            "order/purchase.proto",
            pb::Language::Rust,
        ))
        .await
        .expect("preview cross-package protobuf closure rust codegen")
        .into_inner();

    // Assert: every package in the chain contributes generated types, and the
    // fully-qualified cross-package references compile as downstream Rust code.
    let code = String::from_utf8(preview.content.clone()).expect("generated rust is utf-8");
    for symbol in [
        "struct Money",
        "struct AuditInfo",
        "struct UserProfile",
        "struct PurchaseOrder",
    ] {
        assert!(
            code.contains(symbol),
            "generated cross-package protobuf closure lost `{symbol}`:\n{code}"
        );
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-cross-package-protobuf-local-compile"
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
        .preview_codegen(preview_request(
            "local",
            "compile",
            "build_record.fbs",
            pb::Language::Rust,
        ))
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
async fn flatbuffers_pluggable_buffer_preview_codegen_compiles_in_local_cargo_project() {
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
        "pluggable_record.fbs",
        pb::SchemaFormat::Flatbuffers,
        src,
        "local-compile-flatbuffers-pluggable",
    )
    .await;

    // Act.
    let mut req = preview_request(
        "local",
        "compile",
        "pluggable_record.fbs",
        pb::Language::Rust,
    );
    req.rust_pluggable_buffer = true;
    let preview = c
        .codegen
        .preview_codegen(req)
        .await
        .expect("preview flatbuffers pluggable-buffer rust codegen")
        .into_inner();

    // Assert: generated code exposes the pluggable reader runtime and compiles.
    let code = String::from_utf8(preview.content.clone()).expect("generated rust is utf-8");
    assert!(
        code.contains("__flatc_rs_runtime"),
        "generated Rust should include the pluggable-buffer runtime:\n{code}"
    );
    assert!(
        code.contains("root_as_build_record_in"),
        "generated Rust should include a buffer-generic root helper:\n{code}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let (flatbuffers_repository, flatbuffers_revision) = flatbuffers_git_coordinate();
    let cargo_toml = format!(
        r#"[package]
name = "schemahub-flatbuffers-pluggable-buffer-local-compile"
version = "0.0.0"
edition = "2021"

[dependencies]
flatbuffers = "25.12.19"
flatc-rs-runtime = {{ git = "{flatbuffers_repository}", rev = "{flatbuffers_revision}" }}
"#,
    );
    write_compile_crate(tmp.path(), &cargo_toml, &preview.content);
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
        .preview_codegen(preview_request(
            "local",
            "compile",
            "root.fbs",
            pb::Language::Rust,
        ))
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
async fn complex_flatbuffers_codegen_uses_requested_root_when_dependency_sorts_last() {
    // Arrange: the requested root path sorts before its dependency, and both
    // files declare root_type. This proves root selection follows the request,
    // not lexical closure order. The schemas also cover bit flags, unions,
    // vectors, required/key fields, and rpc_service declarations.
    let url = start_server().await;
    let mut c = clients(&url).await;
    let sensor = integration_fixture("complex_sensor.fbs");
    let telemetry = integration_fixture("complex_telemetry.fbs");
    create_schema(
        &mut c.schema,
        "complex",
        "flatbuffers",
        "main",
        "z_sensor.fbs",
        pb::SchemaFormat::Flatbuffers,
        &sensor,
        "complex-flatbuffers-sensor",
    )
    .await;
    create_schema(
        &mut c.schema,
        "complex",
        "flatbuffers",
        "main",
        "a_telemetry.fbs",
        pb::SchemaFormat::Flatbuffers,
        &telemetry,
        "complex-flatbuffers-telemetry",
    )
    .await;

    // Act.
    let preview = c
        .codegen
        .preview_codegen(preview_request(
            "complex",
            "flatbuffers",
            "a_telemetry.fbs",
            pb::Language::Rust,
        ))
        .await
        .expect("preview complex flatbuffers closure rust codegen")
        .into_inner();

    // Assert.
    let code = String::from_utf8(preview.content.clone()).expect("generated rust is utf-8");
    for symbol in [
        "SensorState",
        "ReadingFlags",
        "Sensor",
        "Alert",
        "TelemetryPayload",
        "TelemetryEnvelope",
    ] {
        assert!(
            code.contains(symbol),
            "generated FlatBuffers closure lost `{symbol}`:\n{code}"
        );
    }
    assert!(
        code.contains("root_as_telemetry_envelope"),
        "requested root helper missing; dependency root may have won:\n{code}"
    );
    assert!(
        !code.contains("root_as_sensor"),
        "dependency root helper should not replace the requested root:\n{code}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    write_compile_crate(
        tmp.path(),
        r#"[package]
name = "schemahub-complex-flatbuffers-closure-local-compile"
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
        .preview_codegen(preview_request(
            "acme",
            "core",
            "catalog/rich_catalog.fbs",
            pb::Language::Rust,
        ))
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
