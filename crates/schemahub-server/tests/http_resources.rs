//! HTTP resource navigation contract used by the browser console.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD,
    CACHE_CONTROL, CONTENT_TYPE, ORIGIN,
};
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::Router;
use schemahub_compiler_protobuf::ProtobufCompiler;
use schemahub_core::change_record::{ChangeEdit, CreateChange};
use schemahub_core::Core;
use schemahub_jj::{MemoryObjectDb, ObjectDb, RefSpec};
use schemahub_server::config::{Config, HttpConfig, ProjectSection, RepoSection, TokenIdentity};
use schemahub_server::{build_core, http, BUILD_VERSION};
use schemahub_types::{Compiler, IdentityKind, MutationEffect, SchemaPath};
use serde_json::json;
use tower::ServiceExt;

fn resource_fixture() -> (Arc<Core>, Router) {
    resource_fixture_with_policy(http::HttpPolicy::default())
}

fn resource_fixture_with_policy(policy: http::HttpPolicy) -> (Arc<Core>, Router) {
    let mut config = Config::default();
    config.auth.tokens.insert(
        "owner-token".to_string(),
        TokenIdentity {
            id: "alice".to_string(),
            display: Some("Alice".to_string()),
            kind: IdentityKind::Human,
            delegated_by: None,
        },
    );
    config.auth.tokens.insert(
        "agent-token".to_string(),
        TokenIdentity {
            id: "schema-agent".to_string(),
            display: Some("Schema Agent".to_string()),
            kind: IdentityKind::Agent,
            delegated_by: Some("alice".to_string()),
        },
    );
    config.projects.insert(
        "acme".to_string(),
        ProjectSection {
            visibility: Some("private".to_string()),
            owners: vec!["alice".to_string()],
            members: HashMap::from([("schema-agent".to_string(), "writer".to_string())]),
        },
    );
    config.projects.insert(
        "beta".to_string(),
        ProjectSection {
            visibility: Some("private".to_string()),
            owners: vec!["alice".to_string()],
            members: HashMap::new(),
        },
    );
    config.repos.insert(
        "acme/commerce".to_string(),
        RepoSection {
            default_bookmark: Some("trunk".to_string()),
            compatibility: Some("backward".to_string()),
            protected_bookmarks: Some(vec!["trunk".to_string(), "release/*".to_string()]),
            review: None,
            serving: None,
        },
    );
    config.repos.insert(
        "acme/events".to_string(),
        RepoSection {
            default_bookmark: Some("main".to_string()),
            compatibility: Some("full".to_string()),
            protected_bookmarks: Some(vec!["main".to_string()]),
            review: None,
            serving: None,
        },
    );
    config.repos.insert(
        "beta/edge".to_string(),
        RepoSection {
            default_bookmark: Some("main".to_string()),
            compatibility: Some("full".to_string()),
            protected_bookmarks: Some(vec!["main".to_string()]),
            review: None,
            serving: None,
        },
    );
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db.clone(), &config);
    let app = http::router_with_policy(
        core.clone(),
        db,
        "memory".to_string(),
        "static-bearer-rbac".to_string(),
        http::Readiness::new(true),
        policy,
    );
    (core, app)
}

fn resource_app() -> Router {
    resource_fixture().1
}

fn protobuf_effect(source: &str) -> MutationEffect {
    let parsed = ProtobufCompiler::new()
        .parse(source)
        .expect("parse protobuf fixture");
    MutationEffect {
        meta: Some(parsed.meta),
        upserts: parsed.decls,
        removes: Vec::new(),
    }
}

fn probe_app(accepting_traffic: bool) -> Router {
    probe_app_with_readiness(http::Readiness::new(accepting_traffic), "noop")
}

fn probe_app_with_readiness(readiness: http::Readiness, auth_mode: &str) -> Router {
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db.clone(), &Config::default());
    http::router(
        core,
        db,
        "memory".to_string(),
        auth_mode.to_string(),
        readiness,
    )
}

fn probe_app_with_http_config(config: HttpConfig) -> Router {
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db.clone(), &Config::default());
    let policy = http::HttpPolicy::from_config(&config).expect("valid HTTP policy");
    http::router_with_policy(
        core,
        db,
        "memory".to_string(),
        "noop".to_string(),
        http::Readiness::new(true),
        policy,
    )
}

fn probe_app_with_gui() -> (tempfile::TempDir, Router) {
    let gui_dir = tempfile::tempdir().expect("GUI tempdir");
    std::fs::create_dir(gui_dir.path().join("assets")).expect("create GUI assets");
    std::fs::write(
        gui_dir.path().join("index.html"),
        "<!doctype html><title>SchemaHub Console</title><div id=\"root\"></div>",
    )
    .expect("write GUI index");
    std::fs::write(
        gui_dir.path().join("assets").join("app-deadbeef.js"),
        "document.title = 'SchemaHub Console';",
    )
    .expect("write GUI asset");
    std::fs::write(
        gui_dir.path().join("assets").join("runtime.js"),
        "window.__schemahub_runtime = true;",
    )
    .expect("write unhashed GUI asset");
    std::fs::write(gui_dir.path().join("favicon.svg"), "<svg></svg>").expect("write GUI favicon");
    let app = probe_app_with_http_config(HttpConfig {
        gui_dir: Some(gui_dir.path().to_path_buf()),
        ..HttpConfig::default()
    });
    (gui_dir, app)
}

async fn get_probe_json(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let json = serde_json::from_slice(&body).expect("JSON body");
    (status, json)
}

async fn get_json(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", "Bearer owner-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let json = serde_json::from_slice(&body).expect("JSON body");
    (status, json)
}

async fn request_json(
    app: Router,
    method: Method,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("HTTP response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let json = serde_json::from_slice(&body).expect("JSON body");
    (status, json)
}

#[tokio::test]
async fn healthz_reports_process_liveness_without_authentication() {
    // Arrange
    let app = probe_app(true);

    // Act
    let (status, health) = get_probe_json(app, "/healthz").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["status"], "ok");
    assert_eq!(health["service"], "schemahub-server");
    assert_eq!(health["version"], BUILD_VERSION);
}

#[tokio::test]
async fn generated_openapi_covers_the_runtime_http_contract() {
    // Arrange
    let app = probe_app(true);

    // Act
    let (status, document) = get_probe_json(app, "/api/openapi.json").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(document["info"]["title"], "SchemaHub HTTP Interfaces");
    assert_eq!(document["info"]["version"], BUILD_VERSION);
    assert_eq!(document["info"]["x-schemahub-public-api"], "schemahub.v1");
    assert_eq!(document["info"]["x-schemahub-gui-bff-prefix"], "/api/");
    assert!(document["info"]["description"]
        .as_str()
        .expect("OpenAPI description")
        .contains("schemahub.v1 gRPC/protobuf"));
    let paths = document["paths"].as_object().expect("OpenAPI paths object");
    let operation_ids: Vec<_> = paths
        .values()
        .filter_map(serde_json::Value::as_object)
        .flat_map(serde_json::Map::values)
        .filter_map(|operation| operation["operationId"].as_str())
        .collect();
    let unique_operation_ids: std::collections::BTreeSet<_> =
        operation_ids.iter().copied().collect();
    assert_eq!(paths.len(), 22);
    assert_eq!(operation_ids.len(), 24);
    assert_eq!(unique_operation_ids.len(), operation_ids.len());
    for (path, item) in paths {
        let (expected_surface, expected_compatibility) = if path.starts_with("/api/") {
            ("gui-bff", "excluded")
        } else {
            ("operations", "supported")
        };
        assert_eq!(
            item["x-schemahub-api-surface"], expected_surface,
            "unexpected surface for {path}"
        );
        assert_eq!(
            item["x-schemahub-compatibility-promise"], expected_compatibility,
            "unexpected compatibility promise for {path}"
        );
    }
    assert!(document["paths"]["/api/projects/{project}/repos/{repo}/changes"]["post"].is_object());
    assert!(
        document["paths"]["/api/projects/{project}/repos/{repo}/changes/{change_id}"]["patch"]
            .is_object()
    );
    assert!(document["paths"]
        ["/api/projects/{project}/repos/{repo}/revisions/{commit}/artifacts/{schema_path}"]["get"]
        .is_object());
    assert_eq!(
        document["paths"]["/api/projects"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/ProjectPageDto"
    );
    assert_eq!(
        document["paths"]["/api/projects"]["get"]["parameters"][0]["name"],
        "pageSize"
    );
    assert_eq!(
        document["paths"]["/api/projects/{project}/repos"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/RepoPageDto"
    );
    assert_eq!(
        document["paths"]["/api/projects/{project}/repos/{repo}/changes"]["get"]["responses"]
            ["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ChangePageDto"
    );
    assert_eq!(
        document["paths"]["/api/projects/{project}/repos/{repo}/dashboard"]["get"]["responses"]
            ["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/RepoDashboardPageDto"
    );
    assert_eq!(
        document["paths"]["/api/projects/{project}/repos/{repo}/dashboard"]["get"]["parameters"][2]
            ["name"],
        "ref"
    );
    assert_eq!(
        document["components"]["securitySchemes"]["bearerAuth"]["scheme"],
        "bearer"
    );
    assert_eq!(
        document["components"]["schemas"]["ChangeRecordDto"]["properties"]["externalReferences"]
            ["items"]["type"],
        "string"
    );
}

#[tokio::test]
async fn generated_openapi_uses_the_canonical_release_bytes() {
    // Arrange
    let app = probe_app(true);

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/openapi.json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()[CACHE_CONTROL], "public, max-age=3600");
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("OpenAPI body");
    assert_eq!(body.as_ref(), http::openapi_json_bytes());
}

#[tokio::test]
async fn gui_bff_routes_advertise_their_surface_classification() {
    // Arrange
    let app = probe_app(true);

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/openapi.json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[http::HTTP_API_SURFACE_HEADER],
        http::HTTP_API_SURFACE_GUI_BFF
    );
}

#[tokio::test]
async fn operational_routes_do_not_claim_the_gui_bff_surface() {
    // Arrange
    let app = probe_app(true);

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response
        .headers()
        .contains_key(http::HTTP_API_SURFACE_HEADER));
}

#[tokio::test]
async fn readyz_reports_a_successful_storage_transaction() {
    // Arrange
    let app = probe_app(true);

    // Act
    let (status, readiness) = get_probe_json(app, "/readyz").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(readiness["status"], "ready");
    assert_eq!(readiness["acceptingTraffic"], true);
    assert_eq!(readiness["authentication"]["mode"], "noop");
    assert_eq!(readiness["authentication"]["status"], "ok");
    assert_eq!(readiness["storage"]["backend"], "memory");
    assert_eq!(readiness["storage"]["status"], "ok");
}

#[tokio::test]
async fn readyz_rejects_traffic_while_the_process_is_draining() {
    // Arrange
    let app = probe_app(false);

    // Act
    let (status, readiness) = get_probe_json(app, "/readyz").await;

    // Assert
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(readiness["status"], "not_ready");
    assert_eq!(readiness["acceptingTraffic"], false);
    assert_eq!(readiness["authentication"]["status"], "not_checked");
    assert_eq!(readiness["storage"]["status"], "not_checked");
}

#[tokio::test]
async fn readyz_fails_closed_when_jwt_verification_keys_are_stale() {
    // Arrange
    let readiness = http::Readiness::new(true);
    readiness.mark_auth_unready();
    let app = probe_app_with_readiness(readiness, "jwt-rbac");

    // Act
    let (status, body) = get_probe_json(app, "/readyz").await;

    // Assert
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "not_ready");
    assert_eq!(body["acceptingTraffic"], true);
    assert_eq!(body["authentication"]["mode"], "jwt-rbac");
    assert_eq!(body["authentication"]["status"], "stale_keys");
    assert_eq!(body["storage"]["status"], "not_checked");
}

#[tokio::test]
async fn http_responses_propagate_the_callers_request_id() {
    // Arrange
    let app = probe_app(true);

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header("x-request-id", "agent-run-42")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-request-id"], "agent-run-42");
}

#[tokio::test]
async fn default_http_policy_emits_no_cross_origin_headers() {
    // Arrange
    let app = probe_app(true);

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header(ORIGIN, "https://gui.example.test")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
}

#[tokio::test]
async fn bundled_gui_serves_the_same_origin_console_entry() {
    // Arrange
    let (_gui_dir, app) = probe_app_with_gui();

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "no-cache");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["referrer-policy"], "same-origin");
    assert_eq!(
        response.headers()["content-security-policy"],
        "default-src 'self'; base-uri 'none'; connect-src 'self'; font-src 'self'; form-action 'none'; frame-ancestors 'none'; frame-src 'none'; img-src 'self' data:; media-src 'none'; object-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'"
    );
    assert_eq!(
        response.headers()["permissions-policy"],
        "camera=(), geolocation=(), microphone=()"
    );
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert!(!response
        .headers()
        .contains_key(http::HTTP_API_SURFACE_HEADER));
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("GUI body");
    assert!(String::from_utf8_lossy(&body).contains("SchemaHub Console"));
}

#[tokio::test]
async fn bundled_gui_serves_a_deep_operator_route_as_the_spa_entry() {
    // Arrange
    let (_gui_dir, app) = probe_app_with_gui();

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/acme/repos/commerce/changes/change-1")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "no-cache");
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("GUI body");
    assert!(String::from_utf8_lossy(&body).contains("SchemaHub Console"));
}

#[tokio::test]
async fn bundled_gui_serves_hashed_assets_with_immutable_caching() {
    // Arrange
    let (_gui_dir, app) = probe_app_with_gui();

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/assets/app-deadbeef.js")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["content-security-policy"],
        "default-src 'self'; base-uri 'none'; connect-src 'self'; font-src 'self'; form-action 'none'; frame-ancestors 'none'; frame-src 'none'; img-src 'self' data:; media-src 'none'; object-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'"
    );
    assert!(response.headers()[CONTENT_TYPE]
        .to_str()
        .expect("asset content type")
        .contains("javascript"));
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("asset body");
    assert_eq!(body, "document.title = 'SchemaHub Console';");
}

#[tokio::test]
async fn bundled_gui_does_not_cache_an_unhashed_asset_as_immutable() {
    // Arrange
    let (_gui_dir, app) = probe_app_with_gui();

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/assets/runtime.js")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "no-cache");
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("asset body");
    assert_eq!(body, "window.__schemahub_runtime = true;");
}

#[tokio::test]
async fn bundled_gui_does_not_turn_an_unknown_bff_route_into_html() {
    // Arrange
    let (_gui_dir, app) = probe_app_with_gui();

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/not-a-real-resource")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers()[http::HTTP_API_SURFACE_HEADER],
        http::HTTP_API_SURFACE_GUI_BFF
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("missing-route body");
    assert!(!String::from_utf8_lossy(&body).contains("SchemaHub Console"));
}

#[tokio::test]
async fn trusted_http_origin_receives_explicit_cross_origin_headers() {
    // Arrange
    let app = probe_app_with_http_config(HttpConfig {
        allowed_origins: vec!["https://gui.example.test".to_string()],
        ..HttpConfig::default()
    });

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header(ORIGIN, "https://gui.example.test")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
        "https://gui.example.test"
    );
    let exposed = response.headers()[ACCESS_CONTROL_EXPOSE_HEADERS]
        .to_str()
        .expect("exposed headers");
    assert!(exposed.contains("etag"));
    assert!(exposed.contains("x-request-id"));
    assert!(exposed.contains("x-schemahub-closure-digest"));
    assert!(exposed.contains(http::HTTP_API_SURFACE_HEADER));
}

#[tokio::test]
async fn trusted_http_origin_preflight_allows_the_bff_contract() {
    // Arrange
    let app = probe_app_with_http_config(HttpConfig {
        allowed_origins: vec!["https://gui.example.test".to_string()],
        ..HttpConfig::default()
    });

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/api/projects")
                .header(ORIGIN, "https://gui.example.test")
                .header(ACCESS_CONTROL_REQUEST_METHOD, "PATCH")
                .header(
                    ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization,content-type,if-none-match,x-request-id",
                )
                .body(Body::empty())
                .expect("preflight request"),
        )
        .await
        .expect("preflight response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
        "https://gui.example.test"
    );
    let methods = response.headers()[ACCESS_CONTROL_ALLOW_METHODS]
        .to_str()
        .expect("allowed methods");
    assert!(methods.contains("GET"));
    assert!(methods.contains("POST"));
    assert!(methods.contains("PATCH"));
    let allowed = response.headers()[ACCESS_CONTROL_ALLOW_HEADERS]
        .to_str()
        .expect("allowed headers");
    assert!(allowed.contains("authorization"));
    assert!(allowed.contains("if-none-match"));
    assert!(allowed.contains("x-request-id"));
    assert_eq!(
        response.headers()[http::HTTP_API_SURFACE_HEADER],
        http::HTTP_API_SURFACE_GUI_BFF
    );
}

#[tokio::test]
async fn unlisted_http_origin_receives_no_cross_origin_permission() {
    // Arrange
    let app = probe_app_with_http_config(HttpConfig {
        allowed_origins: vec!["https://gui.example.test".to_string()],
        ..HttpConfig::default()
    });

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header(ORIGIN, "https://attacker.example.test")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
}

#[tokio::test]
async fn oversized_http_json_body_is_rejected_before_a_change_is_created() {
    // Arrange
    let policy = http::HttpPolicy::from_config(&HttpConfig {
        max_request_body_bytes: 1_024,
        ..HttpConfig::default()
    })
    .expect("valid bounded HTTP policy");
    let (core, app) = resource_fixture_with_policy(policy);
    let body = json!({
        "title": "oversized proposal",
        "description": "x".repeat(2_048),
        "targetBookmark": "trunk"
    });

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/projects/acme/repos/commerce/changes")
                .header("authorization", "Bearer owner-token")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("HTTP response");

    // Assert
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.headers()[http::HTTP_API_SURFACE_HEADER],
        http::HTTP_API_SURFACE_GUI_BFF
    );
    assert!(core
        .list_change_records("acme", "commerce", Some("owner-token"))
        .expect("list changes after rejected request")
        .is_empty());
}

#[tokio::test]
async fn metrics_expose_request_latency_status_and_readiness_counters() {
    // Arrange
    let app = probe_app(true);
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("readiness response");

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("metrics response");
    let status = response.status();
    let content_type = response.headers()["content-type"].clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("metrics body");
    let metrics = String::from_utf8(body.to_vec()).expect("UTF-8 metrics");

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/plain; version=0.0.4; charset=utf-8");
    assert!(metrics.contains(&format!(
        "schemahub_build_info{{version=\"{BUILD_VERSION}\"}} 1"
    )));
    assert!(metrics.contains("schemahub_http_requests_total 2"));
    assert!(metrics.contains("schemahub_http_responses_total{class=\"2xx\"} 1"));
    assert!(metrics.contains("schemahub_http_request_duration_seconds_bucket"));
    assert!(metrics.contains("schemahub_readiness_checks_total{result=\"ready\"} 1"));
}

#[tokio::test]
async fn project_navigation_reports_a_bounded_projection_and_caller_role() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, projects) = get_json(app, "/api/projects").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects["projects"][0]["name"], "acme");
    assert_eq!(projects["projects"][0]["role"], "Owner");
    assert!(projects["projects"][0].get("repos").is_none());
    assert_eq!(projects["nextPageToken"], "");
}

#[tokio::test]
async fn repository_navigation_reports_persisted_runtime_policy() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, repositories) = get_json(app, "/api/projects/acme/repos").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repositories["repositories"][0]["project"], "acme");
    assert_eq!(repositories["repositories"][0]["repo"], "commerce");
    assert_eq!(repositories["repositories"][0]["defaultBranch"], "trunk");
    assert_eq!(repositories["repositories"][0]["compatibility"], "backward");
    assert_eq!(
        repositories["repositories"][0]["protectedBranches"][1],
        "release/*"
    );
    assert_eq!(repositories["nextPageToken"], "");
}

#[tokio::test]
async fn project_catalog_continuation_returns_the_next_bounded_page() {
    // Arrange
    let app = resource_app();
    let (first_status, first_page) = get_json(app.clone(), "/api/projects?pageSize=1").await;
    let page_token = first_page["nextPageToken"].as_str().expect("project token");

    // Act
    let (second_status, second_page) = get_json(
        app,
        &format!("/api/projects?pageSize=1&pageToken={page_token}"),
    )
    .await;

    // Assert
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first_page["projects"][0]["name"], "acme");
    assert!(!page_token.is_empty());
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_page["projects"][0]["name"], "beta");
    assert_eq!(second_page["nextPageToken"], "");
}

#[tokio::test]
async fn repository_catalog_continuation_returns_the_next_bounded_page() {
    // Arrange
    let app = resource_app();
    let (first_status, first_page) =
        get_json(app.clone(), "/api/projects/acme/repos?pageSize=1").await;
    let page_token = first_page["nextPageToken"]
        .as_str()
        .expect("repository token");

    // Act
    let (second_status, second_page) = get_json(
        app,
        &format!("/api/projects/acme/repos?pageSize=1&pageToken={page_token}"),
    )
    .await;

    // Assert
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first_page["repositories"][0]["repo"], "commerce");
    assert!(!page_token.is_empty());
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_page["repositories"][0]["repo"], "events");
    assert_eq!(second_page["nextPageToken"], "");
}

#[tokio::test]
async fn repository_catalog_rejects_a_project_catalog_token() {
    // Arrange
    let app = resource_app();
    let (_, project_page) = get_json(app.clone(), "/api/projects?pageSize=1").await;
    let project_token = project_page["nextPageToken"]
        .as_str()
        .expect("project token");

    // Act
    let (status, _) = get_json(
        app,
        &format!("/api/projects/acme/repos?pageSize=1&pageToken={project_token}"),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn repository_catalog_rejects_a_token_from_another_project() {
    // Arrange
    let app = resource_app();
    let (_, repository_page) = get_json(app.clone(), "/api/projects/acme/repos?pageSize=1").await;
    let repository_token = repository_page["nextPageToken"]
        .as_str()
        .expect("repository token");

    // Act
    let (status, _) = get_json(
        app,
        &format!("/api/projects/beta/repos?pageSize=1&pageToken={repository_token}"),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn repository_catalog_rejects_a_token_from_another_prefix() {
    // Arrange
    let app = resource_app();
    let (_, repository_page) = get_json(app.clone(), "/api/projects/acme/repos?pageSize=1").await;
    let repository_token = repository_page["nextPageToken"]
        .as_str()
        .expect("repository token");

    // Act
    let (status, _) = get_json(
        app,
        &format!(
            "/api/projects/acme/repos?pageSize=1&namePrefix=events&pageToken={repository_token}"
        ),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn repository_catalog_filters_a_bounded_page_by_name_prefix() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, page) =
        get_json(app, "/api/projects/acme/repos?pageSize=1&namePrefix=events").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["repositories"][0]["repo"], "events");
    assert_eq!(page["nextPageToken"], "");
}

#[tokio::test]
async fn repository_dashboard_uses_the_runtime_default_for_an_empty_repo() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, dashboard) = get_json(app, "/api/projects/acme/repos/commerce/dashboard").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dashboard["repo"]["defaultBranch"], "trunk");
    assert_eq!(dashboard["schemas"], json!([]));
    assert_eq!(dashboard["openConflicts"], 0);
    assert_eq!(dashboard["resolvedCommit"], "");
    assert_eq!(dashboard["nextPageToken"], "");
}

#[tokio::test]
async fn repository_dashboard_continuation_pages_schema_branch_and_tag_names_without_restarts() {
    // Arrange
    let (core, app) = resource_fixture();
    let base = core
        .jj()
        .commit_write_multi(
            "acme",
            "commerce",
            "trunk",
            &RefSpec::bookmark("trunk"),
            vec![
                (
                    "orders.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message Order { string id = 1; }"),
                ),
                (
                    "common.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message Common { string id = 1; }"),
                ),
            ],
            "alice",
            "seed dashboard schemas",
        )
        .expect("seed schemas")
        .commit_id;
    for branch in ["feature/b", "feature/a"] {
        core.jj()
            .create_bookmark(
                "acme",
                "commerce",
                branch,
                &RefSpec::commit(base.clone()),
                "alice",
            )
            .expect("create branch");
    }
    for tag in ["v2", "v1"] {
        core.jj()
            .create_tag(
                "acme",
                "commerce",
                tag,
                &RefSpec::commit(base.clone()),
                "alice",
            )
            .expect("create tag");
    }

    // Act
    let (_, first) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/dashboard?ref=trunk&pageSize=1",
    )
    .await;
    let first_token = first["nextPageToken"]
        .as_str()
        .expect("first dashboard token");
    let (_, second) = get_json(
        app.clone(),
        &format!(
            "/api/projects/acme/repos/commerce/dashboard?ref=trunk&pageSize=1&pageToken={first_token}"
        ),
    )
    .await;
    let second_token = second["nextPageToken"]
        .as_str()
        .expect("second dashboard token");
    let (third_status, third) = get_json(
        app,
        &format!(
            "/api/projects/acme/repos/commerce/dashboard?ref=trunk&pageSize=1&pageToken={second_token}"
        ),
    )
    .await;

    // Assert
    assert_eq!(first["schemas"][0]["path"], "common.proto");
    assert_eq!(first["branches"], json!(["feature/a"]));
    assert_eq!(first["tags"], json!(["v1"]));
    assert!(!first_token.is_empty());
    assert_eq!(second["schemas"][0]["path"], "orders.proto");
    assert_eq!(second["branches"], json!(["feature/b"]));
    assert_eq!(second["tags"], json!(["v2"]));
    assert!(!second_token.is_empty());
    assert_eq!(third_status, StatusCode::OK);
    assert_eq!(third["schemas"], json!([]));
    assert_eq!(third["branches"], json!(["trunk"]));
    assert_eq!(third["tags"], json!([]));
    assert_eq!(third["nextPageToken"], "");
}

#[tokio::test]
async fn repository_dashboard_continuation_keeps_the_first_pages_immutable_schema_snapshot() {
    // Arrange
    let (core, app) = resource_fixture();
    core.jj()
        .commit_write_multi(
            "acme",
            "commerce",
            "trunk",
            &RefSpec::bookmark("trunk"),
            vec![
                (
                    "a.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message A {}"),
                ),
                (
                    "b.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message B {}"),
                ),
            ],
            "alice",
            "seed snapshot",
        )
        .expect("seed snapshot");
    let (_, first) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/dashboard?ref=trunk&pageSize=1",
    )
    .await;
    let page_token = first["nextPageToken"].as_str().expect("dashboard token");
    let resolved_commit = first["resolvedCommit"]
        .as_str()
        .expect("resolved dashboard commit");
    core.jj()
        .commit_write(
            "acme",
            "commerce",
            "trunk",
            "new.proto",
            &RefSpec::bookmark("trunk"),
            protobuf_effect("syntax = \"proto3\"; message New {}"),
            "alice",
            "advance mutable ref",
        )
        .expect("advance trunk");

    // Act
    let (status, second) = get_json(
        app,
        &format!(
            "/api/projects/acme/repos/commerce/dashboard?ref=trunk&pageSize=10&pageToken={page_token}"
        ),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["resolvedCommit"], resolved_commit);
    assert_eq!(second["schemas"].as_array().map(Vec::len), Some(1));
    assert_eq!(second["schemas"][0]["path"], "b.proto");
    assert!(second["schemas"]
        .as_array()
        .is_some_and(|schemas| schemas.iter().all(|schema| schema["path"] != "new.proto")));
}

#[tokio::test]
async fn repository_dashboard_rejects_a_continuation_under_another_repository() {
    // Arrange
    let (core, app) = resource_fixture();
    core.jj()
        .commit_write_multi(
            "acme",
            "commerce",
            "trunk",
            &RefSpec::bookmark("trunk"),
            vec![
                (
                    "a.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message A {}"),
                ),
                (
                    "b.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message B {}"),
                ),
            ],
            "alice",
            "seed dashboard token",
        )
        .expect("seed token source");
    let (_, first) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/dashboard?ref=trunk&pageSize=1",
    )
    .await;
    let page_token = first["nextPageToken"].as_str().expect("dashboard token");

    // Act
    let (status, _) = get_json(
        app,
        &format!(
            "/api/projects/acme/repos/events/dashboard?ref=main&pageSize=1&pageToken={page_token}"
        ),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn repository_dashboard_rejects_a_continuation_under_another_ref_expression() {
    // Arrange
    let (core, app) = resource_fixture();
    core.jj()
        .commit_write_multi(
            "acme",
            "commerce",
            "trunk",
            &RefSpec::bookmark("trunk"),
            vec![
                (
                    "a.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message A {}"),
                ),
                (
                    "b.proto".to_string(),
                    protobuf_effect("syntax = \"proto3\"; message B {}"),
                ),
            ],
            "alice",
            "seed ref-bound token",
        )
        .expect("seed token source");
    let (_, first) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/dashboard?ref=trunk&pageSize=1",
    )
    .await;
    let page_token = first["nextPageToken"].as_str().expect("dashboard token");
    let resolved_commit = first["resolvedCommit"].as_str().expect("resolved commit");

    // Act
    let (status, _) = get_json(
        app,
        &format!(
            "/api/projects/acme/repos/commerce/dashboard?ref=@{resolved_commit}&pageSize=1&pageToken={page_token}"
        ),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn session_reports_server_derived_agent_delegation() {
    // Arrange
    let app = resource_app();

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header("authorization", "Bearer agent-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let session: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["id"], "schema-agent");
    assert_eq!(session["display"], "Schema Agent");
    assert_eq!(session["kind"], "agent");
    assert_eq!(session["delegatedBy"], "alice");
}

#[tokio::test]
async fn malformed_authorization_header_does_not_fall_through_as_anonymous() {
    // Arrange
    let app = resource_app();
    let malformed = HeaderValue::from_bytes(b"\xff").expect("opaque header value");

    // Act
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header("authorization", malformed)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("HTTP response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let error: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");

    // Assert
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"], "authorization header is not valid ASCII");
}

#[tokio::test]
async fn server_config_reports_the_composed_authentication_mode() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, config) = get_json(app, "/api/admin/config").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["authMode"], "static-bearer-rbac");
    assert_eq!(config["storageBackend"], "memory");
}

#[tokio::test]
async fn browser_change_continuation_returns_the_next_indexed_record_page() {
    // Arrange
    let (core, app) = resource_fixture();
    for change_id in ["change-c", "change-a", "change-b"] {
        core.create_change_record(
            CreateChange {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                change_id: Some(change_id.to_string()),
                target_bookmark: "trunk".to_string(),
                base_revision: None,
                title: format!("Proposal {change_id}"),
                description: String::new(),
                external_references: Vec::new(),
                edits: Vec::new(),
            },
            Some("agent-token"),
        )
        .expect("create change");
    }
    let expected = core
        .list_change_records("acme", "commerce", Some("owner-token"))
        .expect("list expected records");

    // Act
    let (first_status, first) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/changes?pageSize=1",
    )
    .await;
    let page_token = first["nextPageToken"].as_str().expect("change token");
    let (second_status, second) = get_json(
        app,
        &format!("/api/projects/acme/repos/commerce/changes?pageSize=1&pageToken={page_token}"),
    )
    .await;

    // Assert
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first["changes"].as_array().map(Vec::len), Some(1));
    assert_eq!(first["changes"][0]["name"], expected[0].name);
    assert!(!page_token.is_empty());
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second["changes"].as_array().map(Vec::len), Some(1));
    assert_eq!(second["changes"][0]["name"], expected[1].name);
}

#[tokio::test]
async fn browser_change_list_rejects_a_token_under_another_repository() {
    // Arrange
    let (core, app) = resource_fixture();
    for change_id in ["first", "second"] {
        core.create_change_record(
            CreateChange {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                change_id: Some(change_id.to_string()),
                target_bookmark: "trunk".to_string(),
                base_revision: None,
                title: change_id.to_string(),
                description: String::new(),
                external_references: Vec::new(),
                edits: Vec::new(),
            },
            Some("agent-token"),
        )
        .expect("create change");
    }
    let (_, first) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/changes?pageSize=1",
    )
    .await;
    let page_token = first["nextPageToken"].as_str().expect("change token");

    // Act
    let (status, _) = get_json(
        app,
        &format!("/api/projects/acme/repos/events/changes?pageSize=1&pageToken={page_token}"),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn browser_change_list_rejects_a_token_with_another_status_filter() {
    // Arrange
    let (core, app) = resource_fixture();
    for change_id in ["first", "second"] {
        core.create_change_record(
            CreateChange {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                change_id: Some(change_id.to_string()),
                target_bookmark: "trunk".to_string(),
                base_revision: None,
                title: change_id.to_string(),
                description: String::new(),
                external_references: Vec::new(),
                edits: Vec::new(),
            },
            Some("agent-token"),
        )
        .expect("create change");
    }
    let (_, first) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/changes?pageSize=1",
    )
    .await;
    let page_token = first["nextPageToken"].as_str().expect("change token");

    // Act
    let (status, _) = get_json(
        app,
        &format!(
            "/api/projects/acme/repos/commerce/changes?pageSize=1&status=draft&pageToken={page_token}"
        ),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn browser_change_list_filters_one_bounded_page_by_lifecycle_status() {
    // Arrange
    let (core, app) = resource_fixture();
    core.create_change_record(
        CreateChange {
            project: "acme".to_string(),
            repo: "commerce".to_string(),
            change_id: Some("draft-only".to_string()),
            target_bookmark: "trunk".to_string(),
            base_revision: None,
            title: "Draft proposal".to_string(),
            description: String::new(),
            external_references: Vec::new(),
            edits: Vec::new(),
        },
        Some("agent-token"),
    )
    .expect("create draft");

    // Act
    let (status, page) = get_json(
        app,
        "/api/projects/acme/repos/commerce/changes?pageSize=1&status=draft",
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["changes"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["changes"][0]["status"], "draft");
    assert_eq!(page["nextPageToken"], "");
}

#[tokio::test]
async fn browser_note_creation_and_listing_preserve_agent_attribution() {
    // Arrange
    let app = resource_app();

    // Act
    let (create_status, created) = request_json(
        app.clone(),
        Method::POST,
        "/api/projects/acme/repos/commerce/changes",
        "agent-token",
        json!({
            "changeId": "observed-drift",
            "title": "Observed nullable identifier drift",
            "description": "Captured by the ingest agent before implementation.",
            "externalReferences": ["INC-2048", "https://tracker.example.test/issues/2048"]
        }),
    )
    .await;
    let (list_status, changes) =
        get_json(app.clone(), "/api/projects/acme/repos/commerce/changes").await;
    let (search_status, search) = get_json(
        app,
        "/api/projects/acme/repos/commerce/search?q=INC-2048&ref=trunk",
    )
    .await;

    // Assert
    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(
        created["name"],
        "projects/acme/repos/commerce/changes/observed-drift"
    );
    assert_eq!(created["status"], "draft");
    assert_eq!(created["targetBookmark"], "trunk");
    assert_eq!(
        created["externalReferences"],
        json!(["INC-2048", "https://tracker.example.test/issues/2048"])
    );
    assert_eq!(created["edits"], json!([]));
    assert_eq!(created["createdBy"]["identity"], "schema-agent");
    assert_eq!(created["createdBy"]["kind"], "agent");
    assert_eq!(created["createdBy"]["delegatedBy"], "alice");
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(changes["changes"].as_array().map(Vec::len), Some(1));
    assert_eq!(changes["changes"][0]["etag"], "v1");
    assert_eq!(changes["nextPageToken"], "");
    assert_eq!(search_status, StatusCode::OK);
    assert_eq!(search["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(search["results"][0]["changeId"], "observed-drift");
}

#[tokio::test]
async fn browser_can_create_an_executable_source_edit() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, created) = request_json(
        app,
        Method::POST,
        "/api/projects/acme/repos/commerce/changes",
        "agent-token",
        json!({
            "changeId": "browser-executable",
            "title": "Create order storage schema",
            "description": "Authored directly in the browser console.",
            "edits": [{
                "kind": "replace_source",
                "schemaPath": "schemas/order.proto",
                "formatId": "protobuf",
                "source": "syntax = \"proto3\"; message Order { string id = 1; }"
            }]
        }),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["targetBookmark"], "trunk");
    assert_eq!(created["createdBy"]["identity"], "schema-agent");
    assert_eq!(created["edits"][0]["kind"], "replace_source");
    assert_eq!(created["edits"][0]["schemaPath"], "schemas/order.proto");
    assert_eq!(created["edits"][0]["formatId"], "protobuf");
    assert_eq!(
        created["edits"][0]["source"],
        "syntax = \"proto3\"; message Order { string id = 1; }"
    );
}

#[tokio::test]
async fn browser_can_create_an_executable_schema_deletion() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, created) = request_json(
        app,
        Method::POST,
        "/api/projects/acme/repos/commerce/changes",
        "agent-token",
        json!({
            "changeId": "browser-deletion",
            "title": "Remove retired order schema",
            "edits": [{
                "kind": "delete_schema",
                "schemaPath": "schemas/legacy-order.proto",
                "formatId": "protobuf"
            }]
        }),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["edits"][0]["kind"], "delete_schema");
    assert_eq!(
        created["edits"][0]["schemaPath"],
        "schemas/legacy-order.proto"
    );
    assert_eq!(created["edits"][0]["formatId"], "protobuf");
    assert!(created["edits"][0]["source"].is_null());
}

#[tokio::test]
async fn browser_change_list_omits_large_source_payloads() {
    // Arrange
    let (core, app) = resource_fixture();
    core.create_change_record(
        CreateChange {
            project: "acme".to_string(),
            repo: "commerce".to_string(),
            change_id: Some("list-summary".to_string()),
            target_bookmark: "trunk".to_string(),
            base_revision: None,
            title: "Create order storage schema".to_string(),
            description: String::new(),
            external_references: Vec::new(),
            edits: vec![ChangeEdit::ReplaceSource {
                schema: SchemaPath::new("acme", "commerce", "schemas/order.proto"),
                format_id: "protobuf".to_string(),
                source: "syntax = \"proto3\"; message Order { string id = 1; }".to_string(),
            }],
        },
        Some("agent-token"),
    )
    .expect("create executable draft");

    // Act
    let (status, changes) = get_json(app, "/api/projects/acme/repos/commerce/changes").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(changes["changes"][0]["edits"][0]["kind"], "replace_source");
    assert!(changes["changes"][0]["edits"][0]["source"].is_null());
}

#[tokio::test]
async fn browser_can_attach_executable_edits_to_an_existing_note() {
    // Arrange
    let (core, app) = resource_fixture();
    let draft = core
        .create_change_record(
            CreateChange {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                change_id: Some("browser-attach".to_string()),
                target_bookmark: "trunk".to_string(),
                base_revision: None,
                title: "Observed order drift".to_string(),
                description: "Intent was recorded before the source was ready.".to_string(),
                external_references: Vec::new(),
                edits: Vec::new(),
            },
            Some("agent-token"),
        )
        .expect("create note-only draft");

    // Act
    let (status, updated) = request_json(
        app,
        Method::PATCH,
        "/api/projects/acme/repos/commerce/changes/browser-attach",
        "agent-token",
        json!({
            "etag": draft.etag,
            "edits": [{
                "kind": "replace_source",
                "schemaPath": "schemas/order.proto",
                "formatId": "protobuf",
                "source": "syntax = \"proto3\"; message Order { string id = 1; string note = 2; }"
            }]
        }),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["etag"], "v2");
    assert_eq!(updated["edits"].as_array().map(Vec::len), Some(1));
    assert_eq!(updated["edits"][0]["kind"], "replace_source");
    assert!(updated["validation"].is_null());
    assert_eq!(
        updated["edits"][0]["source"],
        "syntax = \"proto3\"; message Order { string id = 1; string note = 2; }"
    );
}

#[tokio::test]
async fn browser_edit_authoring_rejects_a_schema_format_mismatch_without_creating_state() {
    // Arrange
    let (core, app) = resource_fixture();

    // Act
    let (status, error) = request_json(
        app,
        Method::POST,
        "/api/projects/acme/repos/commerce/changes",
        "agent-token",
        json!({
            "changeId": "mismatched-edit",
            "title": "Invalid format",
            "edits": [{
                "kind": "delete_schema",
                "schemaPath": "schemas/order.proto",
                "formatId": "flatbuffers"
            }]
        }),
    )
    .await;

    // Assert
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error["error"]
        .as_str()
        .is_some_and(|message| message.contains("does not match")));
    assert!(core
        .list_change_records("acme", "commerce", Some("agent-token"))
        .expect("list changes")
        .is_empty());
}

#[tokio::test]
async fn core_created_executable_change_can_complete_through_browser_actions() {
    // Arrange: this is the same Core entry point used by gRPC/CLI-created
    // changes, while all subsequent lifecycle calls go through the web BFF.
    let (core, app) = resource_fixture();
    let draft = core
        .create_change_record(
            CreateChange {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                change_id: Some("agent-executable".to_string()),
                target_bookmark: "trunk".to_string(),
                base_revision: None,
                title: "Add order storage schema".to_string(),
                description: "Executable proposal from an agent client".to_string(),
                external_references: Vec::new(),
                edits: vec![ChangeEdit::ReplaceSource {
                    schema: SchemaPath::new("acme", "commerce", "order.proto"),
                    format_id: "protobuf".to_string(),
                    source: "syntax = \"proto3\"; message Order { string id = 1; }".to_string(),
                }],
            },
            Some("agent-token"),
        )
        .expect("create executable change");
    let action_uri = |action: &str| {
        format!("/api/projects/acme/repos/commerce/changes/agent-executable/actions/{action}")
    };

    // Act
    let (validate_status, validated) = request_json(
        app.clone(),
        Method::POST,
        &action_uri("validate"),
        "agent-token",
        json!({ "etag": draft.etag }),
    )
    .await;
    let (ready_status, ready) = request_json(
        app.clone(),
        Method::POST,
        &action_uri("ready"),
        "agent-token",
        json!({ "etag": validated["etag"] }),
    )
    .await;
    let (approve_status, approved) = request_json(
        app.clone(),
        Method::POST,
        &action_uri("approve"),
        "owner-token",
        json!({
            "etag": ready["etag"],
            "reason": "Validation and storage contract reviewed"
        }),
    )
    .await;
    let (apply_status, applied) = request_json(
        app.clone(),
        Method::POST,
        &action_uri("apply"),
        "agent-token",
        json!({
            "etag": approved["etag"],
            "requestId": "gui-apply-agent-executable"
        }),
    )
    .await;
    let (search_status, search) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/search?q=order&ref=trunk",
    )
    .await;
    let (stale_status, stale_error) = request_json(
        app,
        Method::POST,
        &action_uri("abandon"),
        "agent-token",
        json!({ "etag": ready["etag"] }),
    )
    .await;

    // Assert
    assert_eq!(validate_status, StatusCode::OK);
    assert_eq!(validated["validation"]["valid"], true);
    assert_eq!(validated["edits"][0]["kind"], "replace_source");
    assert_eq!(ready_status, StatusCode::OK);
    assert_eq!(ready["status"], "ready");
    assert_eq!(approve_status, StatusCode::OK);
    assert_eq!(approved["reviews"][0]["reviewer"]["identity"], "alice");
    assert_eq!(approved["reviews"][0]["decision"], "approved");
    assert_eq!(apply_status, StatusCode::OK);
    assert_eq!(applied["status"], "applied");
    assert!(applied["applyResult"]["commitId"]
        .as_str()
        .is_some_and(|commit| !commit.is_empty()));
    assert_eq!(search_status, StatusCode::OK);
    let kinds: std::collections::BTreeSet<_> = search["results"]
        .as_array()
        .expect("search results")
        .iter()
        .filter_map(|result| result["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        std::collections::BTreeSet::from(["change", "declaration", "revision", "schema",])
    );
    assert_eq!(stale_status, StatusCode::PRECONDITION_FAILED);
    assert!(stale_error["error"]
        .as_str()
        .is_some_and(|message| message.contains("cannot abandon")));
}

#[tokio::test]
async fn browser_can_render_and_resolve_a_first_class_declaration_conflict() {
    // Arrange: two writes to the same bookmark use the same causal base, so JJ
    // retains both `Order` declarations as a first-class conflict.
    let (core, app) = resource_fixture();
    let compiler = ProtobufCompiler::new();
    let effect = |source: &str, include_meta: bool| {
        let parsed = compiler.parse(source).expect("parse protobuf");
        MutationEffect {
            meta: include_meta.then_some(parsed.meta),
            upserts: parsed.decls,
            removes: Vec::new(),
        }
    };
    let base = core
        .jj()
        .commit_write(
            "acme",
            "commerce",
            "trunk",
            "order.proto",
            &RefSpec::bookmark("trunk"),
            effect(
                "syntax = \"proto3\"; message Order { string id = 1; }",
                true,
            ),
            "alice",
            "base",
        )
        .expect("seed base")
        .commit_id;
    core.jj()
        .commit_write(
            "acme",
            "commerce",
            "trunk",
            "order.proto",
            &RefSpec::commit(base.clone()),
            effect(
                "syntax = \"proto3\"; message Order { string id = 1; string note = 2; }",
                false,
            ),
            "alice",
            "human side",
        )
        .expect("write human side");
    let conflicting = core
        .jj()
        .commit_write(
            "acme",
            "commerce",
            "trunk",
            "order.proto",
            &RefSpec::commit(base),
            effect(
                "syntax = \"proto3\"; message Order { string id = 1; int32 quantity = 2; }",
                false,
            ),
            "schema-agent",
            "agent side",
        )
        .expect("write agent side");
    assert_eq!(conflicting.conflicted_decls, vec!["Order"]);

    // Act
    let (list_status, listed) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/conflicts?bookmark=trunk",
    )
    .await;
    let (render_status, rendered) = get_json(
        app.clone(),
        "/api/projects/acme/repos/commerce/conflicts/render?bookmark=trunk&schemaPath=order.proto&declarationName=Order",
    )
    .await;
    let (resolve_status, resolved) = request_json(
        app.clone(),
        Method::POST,
        "/api/projects/acme/repos/commerce/conflicts/resolve",
        "owner-token",
        json!({
            "bookmark": "trunk",
            "schemaPath": "order.proto",
            "declarationName": "Order",
            "resolvedSource": "syntax = \"proto3\"; message Order { string id = 1; string note = 2; int32 quantity = 3; }",
            "message": "Resolve human and agent Order edits"
        }),
    )
    .await;
    let (clean_status, clean) = get_json(
        app,
        "/api/projects/acme/repos/commerce/conflicts?bookmark=trunk",
    )
    .await;

    // Assert
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(listed["conflicts"][0]["schemaPath"], "order.proto");
    assert_eq!(listed["conflicts"][0]["declarationName"], "Order");
    assert_eq!(render_status, StatusCode::OK);
    assert!(rendered["rendered"]
        .as_str()
        .is_some_and(|content| !content.is_empty()));
    assert_eq!(resolve_status, StatusCode::OK);
    assert!(resolved["commitId"]
        .as_str()
        .is_some_and(|commit| !commit.is_empty()));
    assert_eq!(clean_status, StatusCode::OK);
    assert_eq!(clean["conflicts"], json!([]));
}
