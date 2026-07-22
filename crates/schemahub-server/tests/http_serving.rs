//! HTTP cache semantics for immutable schema artifacts.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::header::{ETAG, IF_NONE_MATCH};
use axum::http::{Request, StatusCode};
use schemahub_compiler_protobuf::ProtobufCompiler;
use schemahub_jj::{MemoryObjectDb, ObjectDb, RefSpec};
use schemahub_server::{build_core, config::Config, http};
use schemahub_types::{Compiler, MutationEffect};
use tower::ServiceExt;

#[tokio::test]
async fn http_artifact_returns_etag_and_honors_if_none_match() {
    // Arrange
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db.clone(), &Config::default());
    let parsed = ProtobufCompiler::new()
        .parse("syntax = \"proto3\"; message User { string id = 1; }")
        .expect("parse schema");
    core.jj()
        .commit_write(
            "acme",
            "commerce",
            "main",
            "user.proto",
            &RefSpec::bookmark("main"),
            MutationEffect {
                meta: Some(parsed.meta),
                upserts: parsed.decls,
                removes: Vec::new(),
            },
            "alice",
            "seed",
        )
        .expect("seed schema");
    let app = http::router(
        core,
        db,
        "memory".to_string(),
        "noop".to_string(),
        http::Readiness::new(true),
    );
    let resolve_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/projects/acme/repos/commerce/revisions/resolve?ref=main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("resolve HTTP revision");
    let resolve_body = to_bytes(resolve_response.into_body(), usize::MAX)
        .await
        .expect("read revision JSON");
    let revision: serde_json::Value =
        serde_json::from_slice(&resolve_body).expect("decode revision JSON");
    let commit = revision["commitId"].as_str().expect("commitId");
    let artifact_uri = format!(
        "/api/projects/acme/repos/commerce/revisions/{commit}/artifacts/user.proto?kind=source"
    );

    // Act
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&artifact_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("get HTTP artifact");
    let etag = first.headers().get(ETAG).expect("ETag").clone();
    let closure_digest = first
        .headers()
        .get("x-schemahub-closure-digest")
        .expect("closure digest")
        .clone();
    let first_status = first.status();
    let first_body = to_bytes(first.into_body(), usize::MAX)
        .await
        .expect("read source body");
    let conditional = app
        .oneshot(
            Request::builder()
                .uri(&artifact_uri)
                .header(IF_NONE_MATCH, etag.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("conditional HTTP artifact");

    // Assert
    assert_eq!(first_status, StatusCode::OK);
    assert!(String::from_utf8(first_body.to_vec())
        .unwrap()
        .contains("message User"));
    assert!(etag.to_str().unwrap().starts_with("\"sha256:"));
    assert!(closure_digest.to_str().unwrap().starts_with("sha256:"));
    assert_eq!(conditional.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(conditional.headers().get(ETAG), Some(&etag));
    assert!(to_bytes(conditional.into_body(), usize::MAX)
        .await
        .expect("read 304 body")
        .is_empty());
}
