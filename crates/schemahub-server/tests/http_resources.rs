//! HTTP resource navigation contract used by the browser console.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD,
    ORIGIN,
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
    assert_eq!(operation_ids.len(), 23);
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
    assert!(document["paths"]
        ["/api/projects/{project}/repos/{repo}/revisions/{commit}/artifacts/{schema_path}"]["get"]
        .is_object());
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
                .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
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
    assert!(response.headers()[ACCESS_CONTROL_ALLOW_METHODS]
        .to_str()
        .expect("allowed methods")
        .contains("GET"));
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
async fn project_navigation_reports_persisted_repo_count_and_caller_role() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, projects) = get_json(app, "/api/projects").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects[0]["name"], "acme");
    assert_eq!(projects[0]["repos"], 1);
    assert_eq!(projects[0]["role"], "Owner");
}

#[tokio::test]
async fn repository_navigation_reports_persisted_runtime_policy() {
    // Arrange
    let app = resource_app();

    // Act
    let (status, repositories) = get_json(app, "/api/projects/acme/repos").await;

    // Assert
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repositories[0]["project"], "acme");
    assert_eq!(repositories[0]["repo"], "commerce");
    assert_eq!(repositories[0]["defaultBranch"], "trunk");
    assert_eq!(repositories[0]["compatibility"], "backward");
    assert_eq!(repositories[0]["protectedBranches"][1], "release/*");
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
    assert_eq!(changes.as_array().map(Vec::len), Some(1));
    assert_eq!(changes[0]["etag"], "v1");
    assert_eq!(search_status, StatusCode::OK);
    assert_eq!(search["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(search["results"][0]["changeId"], "observed-drift");
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
