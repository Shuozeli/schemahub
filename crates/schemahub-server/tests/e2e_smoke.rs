//! Smoke test exercising the shared `common` harness: create a protobuf schema
//! and pull its reconstructed source back. Template for the broader e2e suites.

mod common;

use common::*;
use schemahub_api::schemahub_v1 as pb;

#[tokio::test]
async fn smoke_create_and_pull_protobuf() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    let src = "syntax = \"proto3\";\npackage demo;\n\nmessage Ping {\n  string msg = 1;\n}\n";

    // Act
    create_schema(
        &mut c.schema,
        "demo",
        "api",
        "main",
        "ping.proto",
        pb::SchemaFormat::Protobuf,
        src,
        "k1",
    )
    .await;
    let pulled = pull_source(&mut c.explore, "demo", "api", "ping.proto", vref_branch("main")).await;

    // Assert
    assert!(pulled.contains("message Ping"), "got:\n{pulled}");
    assert!(pulled.contains("msg = 1"), "got:\n{pulled}");
}
