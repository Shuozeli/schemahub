//! HTTP/JSON BFF for the SchemaHub web console.
//!
//! This is intentionally read-mostly and DTO-oriented. The browser should not
//! import Rust/protobuf internals; it talks to these stable UI shapes while the
//! BFF adapts them to `Core`.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use schemahub_core::change_record::{
    ApplyResult, ChangeActor, ChangeEdit, ChangeRecord, ChangeRecordPageCursor, ChangeRecordStatus,
    ChangeReview, ChangeReviewDecision, ChangeUpdate, CreateChange, ValidationResult,
};
use schemahub_core::{
    detect_format_from_name, Core, CoreError, LogEntry, OperationRecord, RepoConfig,
    SchemaArtifactKind,
};
use schemahub_jj::{JjError, ObjectDb, RefSpec};
use schemahub_types::{
    Action, CodegenOptions, CompatibilityDirection, DeclBlob, DeclChange, DeclKind, Language,
    SchemaPath,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;
use utoipa::{IntoParams, ToSchema};

use crate::config::{validate_gui_directory, HttpConfig, DEFAULT_HTTP_MAX_REQUEST_BODY_BYTES};
use crate::observability::{self, ReadinessResult, ServerMetrics};

mod openapi;

/// Response header that identifies the compatibility class of an HTTP route.
pub const HTTP_API_SURFACE_HEADER: &str = "x-schemahub-api-surface";

/// Header value emitted by the unversioned browser-facing `/api/*` routes.
pub const HTTP_API_SURFACE_GUI_BFF: &str = "gui-bff";

const GUI_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; base-uri 'none'; connect-src 'self'; font-src 'self'; form-action 'none'; frame-ancestors 'none'; frame-src 'none'; img-src 'self' data:; media-src 'none'; object-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'";
const DEFAULT_GUI_PAGE_SIZE: usize = 50;
const MAX_GUI_PAGE_SIZE: usize = 200;
const PROJECT_CATALOG_TOKEN_KIND: &str = "projects";
const REPOSITORY_CATALOG_TOKEN_KIND: &str = "repositories";
const DASHBOARD_TOKEN_KIND: &str = "dashboard";
const CHANGE_TOKEN_KIND: &str = "changes";

#[derive(Clone)]
struct AppState {
    core: Arc<Core>,
    object_db: Arc<dyn ObjectDb>,
    readiness: Readiness,
    metrics: ServerMetrics,
    storage_backend: String,
    auth_mode: String,
}

/// Process-level readiness gate shared by the HTTP probe and the composition
/// root. The server marks itself unavailable before graceful draining starts.
#[derive(Clone, Debug)]
pub struct Readiness {
    accepting_traffic: Arc<AtomicBool>,
    auth_ready: Arc<AtomicBool>,
}

/// Validated browser-facing policy applied to every HTTP BFF route.
///
/// Same-origin is the default because an empty allowlist installs no CORS
/// layer. An explicit allowlist enables bearer-token browser calls from only
/// those origins; credentialed cookie requests remain disabled.
#[derive(Clone, Debug)]
pub struct HttpPolicy {
    allowed_origins: Vec<HeaderValue>,
    max_request_body_bytes: usize,
    gui_dir: Option<PathBuf>,
}

impl HttpPolicy {
    pub fn from_config(config: &HttpConfig) -> anyhow::Result<Self> {
        Self::from_config_with_gui_dir(config, None)
    }

    pub fn from_config_with_gui_dir(
        config: &HttpConfig,
        gui_dir_override: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let allowed_origins = config
            .allowed_origins
            .iter()
            .map(|origin| {
                HeaderValue::from_str(origin).map_err(|error| {
                    anyhow::anyhow!("invalid HTTP origin header {origin:?}: {error}")
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let configured_gui_dir = gui_dir_override.or_else(|| config.gui_dir.clone());
        let gui_dir = configured_gui_dir
            .as_deref()
            .map(validate_gui_directory)
            .transpose()?;
        Ok(Self {
            allowed_origins,
            max_request_body_bytes: config.max_request_body_bytes,
            gui_dir,
        })
    }

    fn cors_layer(&self) -> CorsLayer {
        let request_id_header = HeaderName::from_static("x-request-id");
        CorsLayer::new()
            .allow_origin(self.allowed_origins.clone())
            .allow_methods([Method::GET, Method::POST, Method::PATCH])
            .allow_headers([
                AUTHORIZATION,
                CONTENT_TYPE,
                IF_NONE_MATCH,
                request_id_header.clone(),
            ])
            .expose_headers([
                ETAG,
                request_id_header,
                HeaderName::from_static("x-schemahub-closure-digest"),
                HeaderName::from_static(HTTP_API_SURFACE_HEADER),
            ])
    }
}

impl Default for HttpPolicy {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            max_request_body_bytes: DEFAULT_HTTP_MAX_REQUEST_BODY_BYTES,
            gui_dir: None,
        }
    }
}

impl Readiness {
    pub fn new(accepting_traffic: bool) -> Self {
        Self {
            accepting_traffic: Arc::new(AtomicBool::new(accepting_traffic)),
            auth_ready: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn mark_ready(&self) {
        self.accepting_traffic.store(true, Ordering::Release);
    }

    pub fn mark_draining(&self) {
        self.accepting_traffic.store(false, Ordering::Release);
    }

    pub fn mark_auth_ready(&self) {
        self.auth_ready.store(true, Ordering::Release);
    }

    pub fn mark_auth_unready(&self) {
        self.auth_ready.store(false, Ordering::Release);
    }

    fn is_accepting_traffic(&self) -> bool {
        self.accepting_traffic.load(Ordering::Acquire)
    }

    fn is_auth_ready(&self) -> bool {
        self.auth_ready.load(Ordering::Acquire)
    }
}

pub async fn serve<F>(app: Router, addr: SocketAddr, shutdown: F) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// Build the HTTP serving router independently from its listener so integration
/// tests can exercise headers and response bodies without opening a port.
pub fn router(
    core: Arc<Core>,
    object_db: Arc<dyn ObjectDb>,
    storage_backend: String,
    auth_mode: String,
    readiness: Readiness,
) -> Router {
    router_with_metrics(
        core,
        object_db,
        storage_backend,
        auth_mode,
        readiness,
        ServerMetrics::default(),
    )
}

/// Build the HTTP router over a caller-owned metrics registry so the process
/// can aggregate HTTP and gRPC signals in one scrape.
pub fn router_with_metrics(
    core: Arc<Core>,
    object_db: Arc<dyn ObjectDb>,
    storage_backend: String,
    auth_mode: String,
    readiness: Readiness,
    metrics: ServerMetrics,
) -> Router {
    router_with_metrics_and_policy(
        core,
        object_db,
        storage_backend,
        auth_mode,
        readiness,
        metrics,
        HttpPolicy::default(),
    )
}

/// Build the HTTP router with an explicit cross-origin and request-body policy.
pub fn router_with_policy(
    core: Arc<Core>,
    object_db: Arc<dyn ObjectDb>,
    storage_backend: String,
    auth_mode: String,
    readiness: Readiness,
    policy: HttpPolicy,
) -> Router {
    router_with_metrics_and_policy(
        core,
        object_db,
        storage_backend,
        auth_mode,
        readiness,
        ServerMetrics::default(),
        policy,
    )
}

/// Build the production HTTP router with caller-owned metrics and policy.
pub fn router_with_metrics_and_policy(
    core: Arc<Core>,
    object_db: Arc<dyn ObjectDb>,
    storage_backend: String,
    auth_mode: String,
    readiness: Readiness,
    metrics: ServerMetrics,
    policy: HttpPolicy,
) -> Router {
    let gui_dir = policy.gui_dir.clone();
    let state = AppState {
        core,
        object_db,
        readiness,
        metrics: metrics.clone(),
        storage_backend,
        auth_mode,
    };
    let request_id_header = HeaderName::from_static("x-request-id");
    let app = match gui_dir {
        Some(gui_dir) => with_gui_routes(openapi::http_router(), gui_dir),
        None => openapi::http_router(),
    }
    .layer(DefaultBodyLimit::max(policy.max_request_body_bytes))
    .layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &axum::http::Request<_>| {
                let request_id = request
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("missing");
                tracing::info_span!(
                    "http_request",
                    event = "schemahub.http.request",
                    method = %request.method(),
                    path = request.uri().path(),
                    request_id,
                )
            })
            .on_response(DefaultOnResponse::new().level(Level::INFO)),
    )
    .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
    .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
    .layer(middleware::from_fn_with_state(
        metrics,
        observability::track_http_requests,
    ))
    .with_state(state);
    let app = if policy.allowed_origins.is_empty() {
        app
    } else {
        app.layer(policy.cors_layer())
    };
    app.layer(middleware::from_fn(mark_http_api_surface))
}

fn with_gui_routes(router: Router<AppState>, gui_dir: PathBuf) -> Router<AppState> {
    let index = ServeFile::new(gui_dir.join("index.html"));
    let mut gui = Router::new()
        .route_service("/", index.clone())
        .route_service("/projects", index.clone())
        .route_service("/projects/*path", index.clone())
        .route_service("/admin", index)
        .nest_service("/assets", ServeDir::new(gui_dir.join("assets")));
    let favicon = gui_dir.join("favicon.svg");
    if favicon.is_file() {
        gui = gui.route_service("/favicon.svg", ServeFile::new(favicon));
    }
    router.merge(gui.layer(middleware::from_fn(mark_gui_response)))
}

async fn mark_gui_response(request: axum::extract::Request, next: middleware::Next) -> Response {
    let immutable_asset = is_hashed_gui_asset_path(request.uri().path());
    let mut response = next.run(request).await;
    if response.status().is_success() {
        response.headers_mut().insert(
            CACHE_CONTROL,
            HeaderValue::from_static(if immutable_asset {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            }),
        );
        response.headers_mut().insert(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        );
        response.headers_mut().insert(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("same-origin"),
        );
        response.headers_mut().insert(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(GUI_CONTENT_SECURITY_POLICY),
        );
        response.headers_mut().insert(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
        );
        response.headers_mut().insert(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        );
    }
    response
}

fn is_hashed_gui_asset_path(path: &str) -> bool {
    let Some(relative) = path.strip_prefix("/assets/") else {
        return false;
    };
    let Some(file_name) = relative.rsplit('/').next() else {
        return false;
    };
    let Some((stem, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    if extension.is_empty()
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || stem.len() <= 9
    {
        return false;
    }
    let hash_start = stem.len() - 8;
    let (name_and_separator, hash) = stem.split_at(hash_start);
    name_and_separator.len() > 1
        && name_and_separator.ends_with('-')
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

async fn mark_http_api_surface(
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let path = request.uri().path();
    let is_gui_bff = path == "/api" || path.starts_with("/api/");
    let mut response = next.run(request).await;
    if is_gui_bff {
        response.headers_mut().insert(
            HeaderName::from_static(HTTP_API_SURFACE_HEADER),
            HeaderValue::from_static(HTTP_API_SURFACE_GUI_BFF),
        );
    }
    response
}

/// Return the OpenAPI 3.1 contract generated from the same annotated handlers
/// used to assemble the HTTP router.
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    static DOCUMENT: OnceLock<utoipa::openapi::OpenApi> = OnceLock::new();
    DOCUMENT.get_or_init(openapi::build_document).clone()
}

/// Return the canonical OpenAPI bytes used by both HTTP discovery and release
/// packaging.
///
/// `utoipa` extensions are backed by hash maps, so serializing the generated
/// document directly can emit semantically identical keys in a different
/// order across processes. Rebuilding every JSON object in lexical key order
/// makes the public document byte-stable as well as semantically stable.
pub fn openapi_json_bytes() -> &'static [u8] {
    static JSON: OnceLock<Vec<u8>> = OnceLock::new();
    JSON.get_or_init(|| {
        let value = serde_json::to_value(openapi_document())
            .expect("generated OpenAPI document must serialize to JSON");
        let canonical = canonicalize_json(value);
        let mut bytes = serde_json::to_vec_pretty(&canonical)
            .expect("canonical OpenAPI JSON value must serialize");
        bytes.push(b'\n');
        bytes
    })
}

fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize_json).collect())
        }
        serde_json::Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            let canonical = entries
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect();
            serde_json::Value::Object(canonical)
        }
        scalar => scalar,
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct LivenessDto {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ReadinessDto {
    status: &'static str,
    accepting_traffic: bool,
    authentication: AuthenticationReadinessDto,
    storage: StorageReadinessDto,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct AuthenticationReadinessDto {
    mode: String,
    status: &'static str,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct StorageReadinessDto {
    backend: String,
    status: &'static str,
}

#[utoipa::path(
    get,
    path = "/healthz",
    tag = "operations",
    operation_id = "getLiveness",
    responses((status = 200, description = "Process is alive", body = LivenessDto))
)]
async fn liveness() -> Response {
    probe_response(
        StatusCode::OK,
        LivenessDto {
            status: "ok",
            service: "schemahub-server",
            version: crate::BUILD_VERSION,
        },
    )
}

#[utoipa::path(
    get,
    path = "/readyz",
    tag = "operations",
    operation_id = "getReadiness",
    responses(
        (status = 200, description = "Process can accept traffic", body = ReadinessDto),
        (status = 503, description = "Process is draining or a required dependency is unavailable", body = ReadinessDto)
    )
)]
async fn readiness_probe(State(state): State<AppState>) -> Response {
    if !state.readiness.is_accepting_traffic() {
        state.metrics.record_readiness(ReadinessResult::Draining);
        return probe_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ReadinessDto {
                status: "not_ready",
                accepting_traffic: false,
                authentication: AuthenticationReadinessDto {
                    mode: state.auth_mode,
                    status: "not_checked",
                },
                storage: StorageReadinessDto {
                    backend: state.storage_backend,
                    status: "not_checked",
                },
            },
        );
    }

    if !state.readiness.is_auth_ready() {
        state.metrics.record_readiness(ReadinessResult::AuthFailure);
        return probe_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ReadinessDto {
                status: "not_ready",
                accepting_traffic: true,
                authentication: AuthenticationReadinessDto {
                    mode: state.auth_mode,
                    status: "stale_keys",
                },
                storage: StorageReadinessDto {
                    backend: state.storage_backend,
                    status: "not_checked",
                },
            },
        );
    }

    // ObjectDb is currently synchronous, so keep even the tiny read-only
    // transaction off the async executor. The reserved key is intentionally
    // absent: a successful `None` still proves that the backend can transact.
    let object_db = state.object_db;
    let storage_result = tokio::task::spawn_blocking(move || {
        object_db.get_record("__schemahub_health", "readiness")
    })
    .await;
    let storage_ready = match storage_result {
        Ok(Ok(_)) => true,
        Ok(Err(error)) => {
            tracing::warn!(
                event = "schemahub.readiness.storage_failed",
                backend = state.storage_backend,
                error = %error,
            );
            false
        }
        Err(error) => {
            tracing::error!(
                event = "schemahub.readiness.task_failed",
                backend = state.storage_backend,
                error = %error,
            );
            false
        }
    };
    state.metrics.record_readiness(if storage_ready {
        ReadinessResult::Ready
    } else {
        ReadinessResult::StorageFailure
    });
    let status = if storage_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    probe_response(
        status,
        ReadinessDto {
            status: if storage_ready { "ready" } else { "not_ready" },
            accepting_traffic: true,
            authentication: AuthenticationReadinessDto {
                mode: state.auth_mode,
                status: "ok",
            },
            storage: StorageReadinessDto {
                backend: state.storage_backend,
                status: if storage_ready { "ok" } else { "unavailable" },
            },
        },
    )
}

#[utoipa::path(
    get,
    path = "/metrics",
    tag = "operations",
    operation_id = "getPrometheusMetrics",
    responses((status = 200, description = "Prometheus exposition text", body = String, content_type = "text/plain"))
)]
async fn prometheus_metrics(State(state): State<AppState>) -> Response {
    let mut response = state.metrics.render_prometheus().into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[utoipa::path(
    get,
    path = "/api/openapi.json",
    tag = "discovery",
    operation_id = "getOpenApiDocument",
    responses((status = 200, description = "Generated OpenAPI 3.1 document", content_type = "application/json"))
)]
async fn serve_openapi_document() -> Response {
    let mut response = Response::new(Body::from(openapi_json_bytes()));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}

fn probe_response<T: Serialize>(status: StatusCode, body: T) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ProjectSummaryDto {
    name: String,
    visibility: String,
    role: String,
    last_operation: String,
    last_activity: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ProjectPageDto {
    projects: Vec<ProjectSummaryDto>,
    next_page_token: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RepoSummaryDto {
    project: String,
    repo: String,
    default_branch: String,
    protected_branches: Vec<String>,
    compatibility: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RepoPageDto {
    repositories: Vec<RepoSummaryDto>,
    next_page_token: String,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
struct CatalogPageQuery {
    /// 0 uses the server default; values above the maximum are clamped.
    #[serde(default)]
    page_size: i32,
    /// Opaque continuation returned by the previous response.
    #[serde(default)]
    page_token: String,
    /// Optional stable name-prefix filter.
    #[serde(default)]
    name_prefix: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SchemaSummaryDto {
    path: String,
    format: String,
    declarations: usize,
    dependencies: usize,
    conflict_count: usize,
    last_commit: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RepoDashboardPageDto {
    repo: RepoSummaryDto,
    schemas: Vec<SchemaSummaryDto>,
    branches: Vec<String>,
    tags: Vec<String>,
    latest_commit: CommitEntryDto,
    latest_operation: OperationEntryDto,
    open_conflicts: usize,
    resolved_commit: String,
    next_page_token: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct DeclarationSummaryDto {
    name: String,
    kind: String,
    detail: String,
    refs: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct DependencyDto {
    importing_schema: String,
    import_path: String,
    resolved_commit: String,
    status: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SchemaDetailDto {
    path: String,
    format: String,
    source: String,
    declarations: Vec<DeclarationSummaryDto>,
    dependencies: Vec<DependencyDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct DiffResultDto {
    base: String,
    head: String,
    changes: Vec<DiffChangeDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct DiffChangeDto {
    schema_path: String,
    declaration: String,
    kind: String,
    compatibility: String,
    summary: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct HistoryDto {
    commits: Vec<CommitEntryDto>,
    operations: Vec<OperationEntryDto>,
}

#[derive(Serialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CommitEntryDto {
    commit: String,
    change_id: String,
    parents: Vec<String>,
    author: String,
    message: String,
    timestamp: String,
}

#[derive(Serialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
struct OperationEntryDto {
    op_id: String,
    author: String,
    action: String,
    target: String,
    before: Option<String>,
    after: Option<String>,
    timestamp: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PreviewCodegenRequestDto {
    project: String,
    repo: String,
    schema_path: String,
    r#ref: String,
    language: String,
    rust_pluggable_buffer: Option<bool>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CodegenPreviewDto {
    content: String,
    is_archive: bool,
    at_commit: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RevisionDto {
    name: String,
    project: String,
    repo: String,
    commit_id: String,
    resolved_from: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ServerConfigDto {
    storage_backend: String,
    auth_mode: String,
    max_ops_per_transaction: usize,
    max_schemas_per_transaction: usize,
    supported_formats: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SessionDto {
    authenticated: bool,
    id: Option<String>,
    display: Option<String>,
    kind: String,
    delegated_by: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeActorDto {
    identity: String,
    kind: String,
    display_name: Option<String>,
    delegated_by: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeEditDto {
    kind: String,
    schema_path: String,
    format_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ValidationIssueDto {
    code: String,
    message: String,
    schema_name: Option<String>,
    declaration_name: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeValidationDto {
    valid: bool,
    resolved_base_commit: String,
    edit_digest: String,
    issues: Vec<ValidationIssueDto>,
    validated_at_unix_ms: i64,
    validator_version: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeReviewDto {
    reviewer: ChangeActorDto,
    decision: String,
    reason: String,
    create_time_unix_ms: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeApplyResultDto {
    commit_id: String,
    change_id: String,
    operation_id: String,
    conflicted_declarations: Vec<String>,
    artifact_digest: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeRecordDto {
    name: String,
    project: String,
    repo: String,
    target_bookmark: String,
    base_revision: Option<String>,
    title: String,
    description: String,
    external_references: Vec<String>,
    edits: Vec<ChangeEditDto>,
    created_by: ChangeActorDto,
    status: String,
    validation: Option<ChangeValidationDto>,
    reviews: Vec<ChangeReviewDto>,
    apply_result: Option<ChangeApplyResultDto>,
    etag: String,
    create_time_unix_ms: i64,
    update_time_unix_ms: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangePageDto {
    changes: Vec<ChangeRecordDto>,
    next_page_token: String,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
struct ChangePageQuery {
    /// 0 uses the server default; values above the maximum are clamped.
    #[serde(default)]
    page_size: i32,
    /// Opaque continuation returned by the previous response.
    #[serde(default)]
    page_token: String,
    /// Optional lifecycle status: draft, ready, applying, applied, rejected,
    /// or abandoned.
    #[serde(default)]
    status: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreateChangeDto {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    external_references: Vec<String>,
    #[serde(default)]
    target_bookmark: String,
    base_revision: Option<String>,
    change_id: Option<String>,
    #[serde(default)]
    edits: Vec<ChangeEditInputDto>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeEditInputDto {
    kind: String,
    schema_path: String,
    format_id: String,
    source: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateChangeEditsDto {
    etag: String,
    edits: Vec<ChangeEditInputDto>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ChangeActionDto {
    etag: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    request_id: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SearchResponseDto {
    query: String,
    r#ref: String,
    results: Vec<SearchResultDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SearchResultDto {
    kind: String,
    title: String,
    description: String,
    schema_path: Option<String>,
    declaration_name: Option<String>,
    revision: Option<String>,
    change_id: Option<String>,
    status: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ConflictListDto {
    bookmark: String,
    conflicts: Vec<ConflictSummaryDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ConflictSummaryDto {
    schema_path: String,
    declaration_name: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ConflictDetailDto {
    bookmark: String,
    schema_path: String,
    declaration_name: String,
    rendered: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ResolveConflictDto {
    #[serde(default)]
    bookmark: String,
    schema_path: String,
    declaration_name: String,
    resolved_source: String,
    #[serde(default)]
    message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ResolveConflictResultDto {
    commit_id: String,
    change_id: String,
    remaining_conflicts: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
struct RefQuery {
    #[serde(default)]
    r#ref: String,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
struct DashboardPageQuery {
    /// Bookmark, tag:<name>, or @<commit>; defaults to the repository default.
    #[serde(default)]
    r#ref: String,
    /// 0 uses the server default; values above the maximum are clamped.
    #[serde(default)]
    page_size: i32,
    /// Opaque continuation returned by the previous response.
    #[serde(default)]
    page_token: String,
}

struct DashboardContinuation {
    resolved_commit: String,
    schema_cursor: Option<String>,
    branch_cursor: Option<String>,
    tag_cursor: Option<String>,
}

#[derive(Deserialize, ToSchema)]
struct DiffQuery {
    #[serde(default)]
    base: String,
    #[serde(default)]
    head: String,
    #[serde(rename = "schemaPath")]
    schema_path: Option<String>,
}

#[derive(Deserialize, ToSchema)]
struct HistoryQuery {
    #[serde(default)]
    r#ref: String,
    limit: Option<usize>,
}

#[derive(Deserialize, ToSchema)]
struct SearchQuery {
    #[serde(default, alias = "query")]
    q: String,
    #[serde(default)]
    r#ref: String,
    limit: Option<usize>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ConflictQuery {
    #[serde(default)]
    bookmark: String,
    schema_path: Option<String>,
    declaration_name: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ArtifactQuery {
    #[serde(default = "default_artifact_kind")]
    kind: String,
    language: Option<String>,
    #[serde(default)]
    rust_pluggable_buffer: bool,
}

fn default_artifact_kind() -> String {
    "source".to_string()
}

#[utoipa::path(
    get,
    path = "/api/projects",
    tag = "projects",
    operation_id = "listProjects",
    params(CatalogPageQuery),
    responses(
        (status = 200, description = "One bounded page of projects visible to the caller", body = ProjectPageDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn list_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CatalogPageQuery>,
) -> Result<Json<ProjectPageDto>, ApiError> {
    let token = auth_token(&headers)?;
    let limit = gui_page_size(query.page_size)?;
    let cursor = parse_gui_catalog_page_token(
        &query.page_token,
        PROJECT_CATALOG_TOKEN_KIND,
        "",
        &query.name_prefix,
    )?;
    let page = state.core.list_projects_page(
        false,
        &query.name_prefix,
        cursor.as_deref(),
        limit,
        token.as_deref(),
    )?;
    let mut projects = Vec::with_capacity(page.projects.len());
    for project in page.projects {
        let role = state
            .core
            .caller_project_role(&project.name, token.as_deref())?
            .map(|role| format!("{role:?}"))
            .unwrap_or_else(|| "Reader".to_string());
        projects.push(ProjectSummaryDto {
            name: project.name,
            visibility: match project.visibility {
                schemahub_types::Visibility::Public => "public".to_string(),
                schemahub_types::Visibility::Private => "private".to_string(),
            },
            role,
            last_operation: "project updated".to_string(),
            last_activity: project.update_time_unix_ms.to_string(),
        });
    }
    let next_page_token = page
        .next_cursor
        .as_deref()
        .map(|cursor| {
            make_gui_catalog_page_token(PROJECT_CATALOG_TOKEN_KIND, "", &query.name_prefix, cursor)
        })
        .unwrap_or_default();
    Ok(Json(ProjectPageDto {
        projects,
        next_page_token,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/changes",
    tag = "changes",
    operation_id = "listChanges",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ChangePageQuery
    ),
    responses(
        (status = 200, description = "One bounded page of durable human and agent change records", body = ChangePageDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn list_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<ChangePageQuery>,
) -> Result<Json<ChangePageDto>, ApiError> {
    let token = auth_token(&headers)?;
    let limit = gui_page_size(query.page_size)?;
    let status_filter = gui_change_status(&query.status)?;
    let cursor = parse_gui_change_page_token(&query.page_token, &project, &repo, &query.status)?;
    let page = state.core.list_change_records_page(
        &project,
        &repo,
        status_filter,
        cursor.as_ref(),
        limit,
        token.as_deref(),
    )?;
    let next_page_token = page
        .next_cursor
        .as_ref()
        .map(|cursor| make_gui_change_page_token(&project, &repo, &query.status, cursor))
        .unwrap_or_default();
    Ok(Json(ChangePageDto {
        changes: page
            .records
            .into_iter()
            .map(|change| change_to_dto(change, false))
            .collect(),
        next_page_token,
    }))
}

#[utoipa::path(
    post,
    path = "/api/projects/{project}/repos/{repo}/changes",
    tag = "changes",
    operation_id = "createChange",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID")
    ),
    request_body = CreateChangeDto,
    responses(
        (status = 201, description = "Change record created", body = ChangeRecordDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn create_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Json(request): Json<CreateChangeDto>,
) -> Result<(StatusCode, Json<ChangeRecordDto>), ApiError> {
    let token = auth_token(&headers)?;
    let edits = change_edits_from_dto(&project, &repo, request.edits)?;
    let target_bookmark = if request.target_bookmark.trim().is_empty() {
        state
            .core
            .repository_default_bookmark(&project, &repo, Action::Write, token.as_deref())?
    } else {
        request.target_bookmark
    };
    let change = state.core.create_change_record(
        CreateChange {
            project,
            repo,
            change_id: request.change_id.filter(|value| !value.trim().is_empty()),
            target_bookmark,
            base_revision: request
                .base_revision
                .filter(|value| !value.trim().is_empty()),
            title: request.title,
            description: request.description,
            external_references: request.external_references,
            edits,
        },
        token.as_deref(),
    )?;
    Ok((StatusCode::CREATED, Json(change_to_dto(change, true))))
}

#[utoipa::path(
    patch,
    path = "/api/projects/{project}/repos/{repo}/changes/{change_id}",
    tag = "changes",
    operation_id = "updateChangeEdits",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("change_id" = String, Path, description = "Change record ID")
    ),
    request_body = UpdateChangeEditsDto,
    responses(
        (status = 200, description = "Draft executable edits replaced under ETag concurrency control", body = ChangeRecordDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn update_change_edits(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo, change_id)): Path<(String, String, String)>,
    Json(request): Json<UpdateChangeEditsDto>,
) -> Result<Json<ChangeRecordDto>, ApiError> {
    let token = auth_token(&headers)?;
    let name = change_resource_name(&project, &repo, &change_id);
    let edits = change_edits_from_dto(&project, &repo, request.edits)?;
    let change = state.core.update_change_record(
        &name,
        &request.etag,
        ChangeUpdate {
            edits: Some(edits),
            ..ChangeUpdate::default()
        },
        token.as_deref(),
    )?;
    Ok(Json(change_to_dto(change, true)))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/changes/{change_id}",
    tag = "changes",
    operation_id = "getChange",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("change_id" = String, Path, description = "Change record ID")
    ),
    responses(
        (status = 200, description = "Change record", body = ChangeRecordDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn get_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo, change_id)): Path<(String, String, String)>,
) -> Result<Json<ChangeRecordDto>, ApiError> {
    let token = auth_token(&headers)?;
    let name = change_resource_name(&project, &repo, &change_id);
    let change = state.core.get_change_record(&name, token.as_deref())?;
    Ok(Json(change_to_dto(change, true)))
}

#[utoipa::path(
    post,
    path = "/api/projects/{project}/repos/{repo}/changes/{change_id}/actions/{action}",
    tag = "changes",
    operation_id = "runChangeAction",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("change_id" = String, Path, description = "Change record ID"),
        ("action" = String, Path, description = "validate, ready, approve, reject, apply, or abandon")
    ),
    request_body = ChangeActionDto,
    responses(
        (status = 200, description = "Updated change record", body = ChangeRecordDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn change_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo, change_id, action)): Path<(String, String, String, String)>,
    Json(request): Json<ChangeActionDto>,
) -> Result<Json<ChangeRecordDto>, ApiError> {
    let token = auth_token(&headers)?;
    let token = token.as_deref();
    let name = change_resource_name(&project, &repo, &change_id);
    let change = match action.as_str() {
        "validate" => state
            .core
            .validate_change_record(&name, &request.etag, token)?,
        "ready" => state.core.mark_change_ready(&name, &request.etag, token)?,
        "approve" => {
            state
                .core
                .approve_change_record(&name, &request.etag, request.reason, token)?
        }
        "reject" => state
            .core
            .reject_change_record(&name, &request.etag, request.reason, token)?,
        "apply" => {
            if request.request_id.trim().is_empty() {
                return Err(ApiError::bad_request(
                    "apply action requires a stable requestId",
                ));
            }
            state
                .core
                .apply_change_record(&name, &request.etag, &request.request_id, token)?
        }
        "abandon" => state
            .core
            .abandon_change_record(&name, &request.etag, token)?,
        other => {
            return Err(ApiError::bad_request(format!(
                "unsupported change action {other:?}"
            )))
        }
    };
    Ok(Json(change_to_dto(change, true)))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos",
    tag = "repositories",
    operation_id = "listRepositories",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        CatalogPageQuery
    ),
    responses(
        (status = 200, description = "One bounded page of repositories visible to the caller", body = RepoPageDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn list_repos(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Query(query): Query<CatalogPageQuery>,
) -> Result<Json<RepoPageDto>, ApiError> {
    let token = auth_token(&headers)?;
    let limit = gui_page_size(query.page_size)?;
    let cursor = parse_gui_catalog_page_token(
        &query.page_token,
        REPOSITORY_CATALOG_TOKEN_KIND,
        &project,
        &query.name_prefix,
    )?;
    let page = state.core.list_repositories_page(
        &project,
        false,
        &query.name_prefix,
        cursor.as_deref(),
        limit,
        token.as_deref(),
    )?;
    let repositories = page
        .repositories
        .into_iter()
        .map(|repository| repo_summary(&repository.project, &repository.name, &repository.config))
        .collect();
    let next_page_token = page
        .next_cursor
        .as_deref()
        .map(|cursor| {
            make_gui_catalog_page_token(
                REPOSITORY_CATALOG_TOKEN_KIND,
                &project,
                &query.name_prefix,
                cursor,
            )
        })
        .unwrap_or_default();
    Ok(Json(RepoPageDto {
        repositories,
        next_page_token,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/dashboard",
    tag = "repositories",
    operation_id = "getRepositoryDashboard",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        DashboardPageQuery
    ),
    responses(
        (status = 200, description = "One bounded repository dashboard page pinned to an immutable schema snapshot", body = RepoDashboardPageDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn repo_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<DashboardPageQuery>,
) -> Result<Json<RepoDashboardPageDto>, ApiError> {
    let token = auth_token(&headers)?;
    let token = token.as_deref();
    let limit = gui_page_size(query.page_size)?;
    let config = state
        .core
        .get_repository(&project, &repo, false, token)?
        .map(|repository| repository.config)
        .ok_or_else(|| ApiError::not_found(format!("repository {project}/{repo} not found")))?;
    let ref_name = if query.r#ref.is_empty() {
        config.default_bookmark.clone()
    } else {
        query.r#ref
    };
    let continuation = parse_dashboard_page_token(&query.page_token, &project, &repo, &ref_name)?;
    let is_empty_default = ref_name == config.default_bookmark;

    let schema_done = continuation
        .as_ref()
        .is_some_and(|cursor| cursor.schema_cursor.is_none());
    let schema_start = continuation
        .as_ref()
        .and_then(|cursor| cursor.schema_cursor.as_deref());
    let schema_at = continuation
        .as_ref()
        .map(|cursor| RefSpec::commit(cursor.resolved_commit.clone()))
        .unwrap_or_else(|| refspec(&ref_name));
    let (schema_names, resolved_commit, next_schema_cursor) = if schema_done {
        (
            Vec::new(),
            continuation
                .as_ref()
                .expect("schema_done requires a continuation")
                .resolved_commit
                .clone(),
            None,
        )
    } else {
        match state.core.list_schemas_page_resolved(
            &project,
            &repo,
            &schema_at,
            schema_start,
            limit,
            token,
        ) {
            Ok((page, commit)) => {
                if continuation
                    .as_ref()
                    .is_some_and(|cursor| cursor.resolved_commit != commit)
                {
                    return Err(ApiError::bad_request(
                        "pageToken no longer resolves to its dashboard snapshot",
                    ));
                }
                (page.schemas, commit, page.next_cursor)
            }
            Err(error)
                if continuation.is_none() && is_empty_default && is_missing_bookmark(&error) =>
            {
                (Vec::new(), String::new(), None)
            }
            Err(error) => return Err(error.into()),
        }
    };

    let first_page = continuation.is_none();
    let branch_start = continuation
        .as_ref()
        .and_then(|cursor| cursor.branch_cursor.as_deref());
    let (branches, next_branch_cursor) = if first_page
        || continuation
            .as_ref()
            .is_some_and(|cursor| cursor.branch_cursor.is_some())
    {
        let page =
            state
                .core
                .list_bookmarks_page(&project, &repo, "", branch_start, limit, token)?;
        (
            page.refs.into_iter().map(|(name, _)| name).collect(),
            page.next_cursor,
        )
    } else {
        (Vec::new(), None)
    };
    let tag_start = continuation
        .as_ref()
        .and_then(|cursor| cursor.tag_cursor.as_deref());
    let (tags, next_tag_cursor) = if first_page
        || continuation
            .as_ref()
            .is_some_and(|cursor| cursor.tag_cursor.is_some())
    {
        let page = state
            .core
            .list_tags_page(&project, &repo, "", tag_start, limit, token)?;
        (
            page.refs.into_iter().map(|(name, _)| name).collect(),
            page.next_cursor,
        )
    } else {
        (Vec::new(), None)
    };

    let immutable_at =
        (!resolved_commit.is_empty()).then(|| RefSpec::commit(resolved_commit.clone()));
    let commits = if let Some(at) = immutable_at.as_ref() {
        state.core.log(&project, &repo, Some(at), Some(1), token)?
    } else {
        Vec::new()
    };
    let ops = state.core.op_log(&project, &repo, Some(1), token)?;

    let selected_schemas: BTreeSet<_> = schema_names.iter().cloned().collect();
    let (open_conflicts, conflicts_by_schema) = match immutable_at.as_ref() {
        Some(at) if !ref_name.starts_with('@') && !ref_name.starts_with("tag:") => {
            let stats =
                state
                    .core
                    .conflict_stats_at(&project, &repo, at, &selected_schemas, token)?;
            (stats.total, stats.by_schema)
        }
        _ => (0, BTreeMap::new()),
    };
    let mut schema_inventory = if let Some(at) = immutable_at.as_ref() {
        let (inventory, inventory_commit) = state.core.summarize_schema_inventory_at(
            &project,
            &repo,
            at,
            &selected_schemas,
            token,
        )?;
        if inventory_commit != resolved_commit {
            return Err(ApiError::internal(
                "dashboard schema inventory escaped its immutable snapshot",
            ));
        }
        inventory
    } else {
        BTreeMap::new()
    };

    let latest_commit_id = commits
        .first()
        .map(|c| c.commit_id.clone())
        .unwrap_or_default();
    let mut schema_summaries = Vec::with_capacity(schema_names.len());
    for schema_name in schema_names {
        let inventory = schema_inventory.remove(&schema_name).ok_or_else(|| {
            ApiError::internal(format!(
                "dashboard schema inventory omitted selected schema {schema_name}"
            ))
        })?;
        let conflict_count = conflicts_by_schema.get(&schema_name).copied().unwrap_or(0);
        schema_summaries.push(SchemaSummaryDto {
            format: schema_format(&schema_name).to_string(),
            path: schema_name,
            declarations: inventory.declarations,
            dependencies: inventory.dependencies,
            conflict_count,
            last_commit: latest_commit_id.clone(),
        });
    }

    let next_page_token = if next_schema_cursor.is_some()
        || next_branch_cursor.is_some()
        || next_tag_cursor.is_some()
    {
        make_dashboard_page_token(
            &project,
            &repo,
            &ref_name,
            &resolved_commit,
            next_schema_cursor.as_deref(),
            next_branch_cursor.as_deref(),
            next_tag_cursor.as_deref(),
        )
    } else {
        String::new()
    };

    Ok(Json(RepoDashboardPageDto {
        repo: repo_summary(&project, &repo, &config),
        schemas: schema_summaries,
        branches,
        tags,
        latest_commit: commits
            .first()
            .map(commit_to_dto)
            .unwrap_or_else(empty_commit),
        latest_operation: ops.first().map(operation_to_dto).unwrap_or_else(empty_op),
        open_conflicts,
        resolved_commit,
        next_page_token,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/search",
    tag = "repositories",
    operation_id = "searchRepository",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("q" = String, Query, description = "Case-insensitive search text"),
        ("ref" = Option<String>, Query, description = "Bookmark, tag:<name>, or @<commit>"),
        ("limit" = Option<usize>, Query, description = "Result limit from 1 through 200")
    ),
    responses(
        (status = 200, description = "Cross-resource search results", body = SearchResponseDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn search_resources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponseDto>, ApiError> {
    let token = auth_token(&headers)?;
    let token = token.as_deref();
    let needle = query.q.trim().to_ascii_lowercase();
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    if needle.is_empty() {
        return Ok(Json(SearchResponseDto {
            query: query.q,
            r#ref: query.r#ref,
            results: Vec::new(),
        }));
    }

    // Authorize even an empty repository/query result. Each following Core
    // read retains its own authorization check so this aggregation endpoint
    // cannot become an alternate policy path.
    let repository = state
        .core
        .get_repository(&project, &repo, false, token)?
        .ok_or_else(|| ApiError::not_found(format!("repository {project}/{repo} not found")))?;
    let ref_name = if query.r#ref.is_empty() {
        repository.config.default_bookmark.clone()
    } else {
        query.r#ref.clone()
    };
    let at = refspec(&ref_name);
    let is_empty_default = ref_name == repository.config.default_bookmark;
    let schemas = match state.core.list_schemas(&project, &repo, &at, token) {
        Ok(schemas) => schemas,
        Err(error) if is_empty_default && is_missing_bookmark(&error) => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let mut results = Vec::new();
    for schema_name in schemas {
        if schema_name.to_ascii_lowercase().contains(&needle) {
            results.push(SearchResultDto {
                kind: "schema".to_string(),
                title: schema_name.clone(),
                description: format!("{} schema", schema_format(&schema_name)),
                schema_path: Some(schema_name.clone()),
                declaration_name: None,
                revision: None,
                change_id: None,
                status: None,
            });
        }
        let schema = SchemaPath::new(&project, &repo, &schema_name);
        for declaration in state
            .core
            .list_declarations(&schema, &at, token)?
            .into_iter()
            .filter(|declaration| declaration.name.to_ascii_lowercase().contains(&needle))
        {
            results.push(SearchResultDto {
                kind: "declaration".to_string(),
                title: declaration.name.clone(),
                description: if declaration.doc_comment.is_empty() {
                    decl_kind_to_string(declaration.kind)
                } else {
                    declaration.doc_comment
                },
                schema_path: Some(schema_name.clone()),
                declaration_name: Some(declaration.name),
                revision: None,
                change_id: None,
                status: None,
            });
        }
    }

    let commits = match state.core.log(&project, &repo, Some(&at), Some(200), token) {
        Ok(commits) => commits,
        Err(error) if is_empty_default && is_missing_bookmark(&error) => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    for commit in commits {
        let searchable = format!(
            "{} {} {} {}",
            commit.commit_id, commit.change_id, commit.author, commit.message
        )
        .to_ascii_lowercase();
        if searchable.contains(&needle) {
            results.push(SearchResultDto {
                kind: "revision".to_string(),
                title: commit.message,
                description: format!("{} · {}", commit.author, commit.timestamp),
                schema_path: None,
                declaration_name: None,
                revision: Some(commit.commit_id),
                change_id: None,
                status: None,
            });
        }
    }

    for change in state.core.list_change_records(&project, &repo, token)? {
        let change_id = change
            .name
            .rsplit_once('/')
            .map(|(_, id)| id)
            .unwrap_or(change.name.as_str())
            .to_string();
        let searchable = format!(
            "{} {} {} {} {} {:?}",
            change_id,
            change.title,
            change.description,
            change.external_references.join(" "),
            change.created_by.identity,
            change.status
        )
        .to_ascii_lowercase();
        if searchable.contains(&needle) {
            results.push(SearchResultDto {
                kind: "change".to_string(),
                title: change.title,
                description: if change.description.is_empty() {
                    format!("created by {}", change.created_by.identity)
                } else {
                    change.description
                },
                schema_path: None,
                declaration_name: None,
                revision: None,
                change_id: Some(change_id),
                status: Some(change_status_to_string(change.status)),
            });
        }
    }
    results.truncate(limit);

    Ok(Json(SearchResponseDto {
        query: query.q,
        r#ref: ref_name,
        results,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/conflicts",
    tag = "conflicts",
    operation_id = "listConflicts",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("bookmark" = Option<String>, Query, description = "Bookmark; defaults to the repository default")
    ),
    responses(
        (status = 200, description = "Declaration conflicts on the bookmark", body = ConflictListDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn list_conflicts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<ConflictQuery>,
) -> Result<Json<ConflictListDto>, ApiError> {
    let token = auth_token(&headers)?;
    let token = token.as_deref();
    let repository = state
        .core
        .get_repository(&project, &repo, false, token)?
        .ok_or_else(|| ApiError::not_found(format!("repository {project}/{repo} not found")))?;
    let default_bookmark = repository.config.default_bookmark;
    let bookmark = if query.bookmark.is_empty() {
        default_bookmark.clone()
    } else {
        query.bookmark
    };
    let conflicts = match state.core.list_conflicts(&project, &repo, &bookmark, token) {
        Ok(conflicts) => conflicts,
        Err(error) if bookmark == default_bookmark && is_missing_bookmark(&error) => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    Ok(Json(ConflictListDto {
        bookmark,
        conflicts: conflicts.into_iter().map(conflict_summary).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/conflicts/render",
    tag = "conflicts",
    operation_id = "renderConflict",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("bookmark" = Option<String>, Query, description = "Bookmark; defaults to the repository default"),
        ("schemaPath" = String, Query, description = "Schema path within the repository"),
        ("declarationName" = String, Query, description = "Conflicted declaration name")
    ),
    responses(
        (status = 200, description = "Rendered declaration conflict", body = ConflictDetailDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn render_conflict(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<ConflictQuery>,
) -> Result<Json<ConflictDetailDto>, ApiError> {
    let token = auth_token(&headers)?;
    let token = token.as_deref();
    let repository = state
        .core
        .get_repository(&project, &repo, false, token)?
        .ok_or_else(|| ApiError::not_found(format!("repository {project}/{repo} not found")))?;
    let bookmark = if query.bookmark.is_empty() {
        repository.config.default_bookmark
    } else {
        query.bookmark
    };
    let schema_path = required_query_value(query.schema_path, "schemaPath")?;
    let declaration_name = required_query_value(query.declaration_name, "declarationName")?;
    let schema = SchemaPath::new(&project, &repo, &schema_path);
    let rendered = state
        .core
        .render_conflict(&schema, &bookmark, &declaration_name, token)?;
    Ok(Json(ConflictDetailDto {
        bookmark,
        schema_path,
        declaration_name,
        rendered,
    }))
}

#[utoipa::path(
    post,
    path = "/api/projects/{project}/repos/{repo}/conflicts/resolve",
    tag = "conflicts",
    operation_id = "resolveConflict",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID")
    ),
    request_body = ResolveConflictDto,
    responses(
        (status = 200, description = "Conflict-resolution commit", body = ResolveConflictResultDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn resolve_conflict(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Json(request): Json<ResolveConflictDto>,
) -> Result<Json<ResolveConflictResultDto>, ApiError> {
    let token = auth_token(&headers)?;
    let token = token.as_deref();
    let repository = state
        .core
        .get_repository(&project, &repo, false, token)?
        .ok_or_else(|| ApiError::not_found(format!("repository {project}/{repo} not found")))?;
    let bookmark = if request.bookmark.is_empty() {
        repository.config.default_bookmark
    } else {
        request.bookmark
    };
    let format_id = detect_format_from_name(&request.schema_path).ok_or_else(|| {
        ApiError::bad_request(format!(
            "cannot detect schema format from {:?}",
            request.schema_path
        ))
    })?;
    let compiler = state
        .core
        .registry()
        .get(format_id)
        .ok_or_else(|| ApiError::bad_request(format!("no compiler for {format_id}")))?;
    let parsed = compiler
        .parse(&request.resolved_source)
        .map_err(|error| ApiError::bad_request(format!("invalid resolved source: {error}")))?;
    let resolved: DeclBlob = parsed
        .decls
        .into_iter()
        .find(|(name, _)| name == &request.declaration_name)
        .map(|(_, declaration)| declaration)
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "resolved source does not define {:?}",
                request.declaration_name
            ))
        })?;
    let identity = state.core.resolve_identity(token)?;
    let author = identity.id().unwrap_or("anonymous");
    let message = if request.message.trim().is_empty() {
        format!("resolve conflict on {}", request.declaration_name)
    } else {
        request.message
    };
    let schema = SchemaPath::new(&project, &repo, &request.schema_path);
    let result = state.core.resolve_conflict(
        &schema,
        &bookmark,
        &request.declaration_name,
        resolved,
        author,
        &message,
        token,
    )?;
    Ok(Json(ResolveConflictResultDto {
        commit_id: result.commit_id,
        change_id: result.change_id,
        remaining_conflicts: result.conflicted_decls,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/schemas/{schema_path}",
    tag = "schemas",
    operation_id = "getSchema",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("schema_path" = String, Path, description = "Slash-preserving schema path"),
        ("ref" = Option<String>, Query, description = "Bookmark, tag:<name>, or @<commit>")
    ),
    responses(
        (status = 200, description = "Schema source, declarations, and dependencies", body = SchemaDetailDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn schema_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo, schema_path)): Path<(String, String, String)>,
    Query(query): Query<RefQuery>,
) -> Result<Json<SchemaDetailDto>, ApiError> {
    let token = auth_token(&headers)?;
    let ref_name = effective_ref(&state, &project, &repo, &query.r#ref, token.as_deref())?;
    let at = refspec(&ref_name);
    let path = SchemaPath::new(&project, &repo, &schema_path);
    let source = state.core.get_schema_source(&path, &at, token.as_deref())?;
    let declarations = state
        .core
        .list_declarations(&path, &at, token.as_deref())?
        .into_iter()
        .map(|decl| DeclarationSummaryDto {
            name: decl.name,
            kind: decl_kind_to_string(decl.kind),
            detail: decl.doc_comment,
            refs: Vec::new(),
        })
        .collect();
    let dependencies = state
        .core
        .list_dependencies(&path, &at, false, token.as_deref())?
        .into_iter()
        .map(|dep| DependencyDto {
            importing_schema: schema_path.clone(),
            import_path: dep.path,
            resolved_commit: dep.resolved_commit,
            status: "resolved".to_string(),
        })
        .collect();
    Ok(Json(SchemaDetailDto {
        path: schema_path.clone(),
        format: schema_format(&schema_path).to_string(),
        source,
        declarations,
        dependencies,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/diff",
    tag = "history",
    operation_id = "diffRepository",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("base" = Option<String>, Query, description = "Base bookmark, tag:<name>, or @<commit>"),
        ("head" = Option<String>, Query, description = "Head bookmark, tag:<name>, or @<commit>"),
        ("schemaPath" = Option<String>, Query, description = "Optional schema path filter")
    ),
    responses(
        (status = 200, description = "Declaration-level diff", body = DiffResultDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<DiffResultDto>, ApiError> {
    let token = auth_token(&headers)?;
    let default = effective_ref(&state, &project, &repo, "", token.as_deref())?;
    let base_name = if query.base.is_empty() {
        default.clone()
    } else {
        query.base.clone()
    };
    let head_name = if query.head.is_empty() {
        default
    } else {
        query.head.clone()
    };
    let base = refspec(&base_name);
    let head = refspec(&head_name);
    let schema_paths = if let Some(schema_path) = &query.schema_path {
        vec![schema_path.clone()]
    } else {
        let mut names = std::collections::BTreeSet::new();
        for side in [&base, &head] {
            match state.core.jj().list_schemas(&project, &repo, side) {
                Ok(schemas) => names.extend(schemas),
                Err(
                    JjError::BookmarkNotFound(_)
                    | JjError::TagNotFound(_)
                    | JjError::SchemaNotFound(_),
                ) => {}
                Err(err) => return Err(CoreError::from(err).into()),
            }
        }
        names.into_iter().collect()
    };

    let mut changes = Vec::new();
    for schema_path in schema_paths {
        let path = SchemaPath::new(&project, &repo, &schema_path);
        for change in state.core.diff(&path, &base, &head, token.as_deref())? {
            changes.push(diff_change_to_dto(&schema_path, change));
        }
    }
    Ok(Json(DiffResultDto {
        base: base_name,
        head: head_name,
        changes,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/history",
    tag = "history",
    operation_id = "getRepositoryHistory",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("ref" = Option<String>, Query, description = "Bookmark, tag:<name>, or @<commit>"),
        ("limit" = Option<usize>, Query, description = "Maximum commits and operations")
    ),
    responses(
        (status = 200, description = "Commit and operation history", body = HistoryDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<HistoryDto>, ApiError> {
    let token = auth_token(&headers)?;
    let ref_name = effective_ref(&state, &project, &repo, &query.r#ref, token.as_deref())?;
    let at = refspec(&ref_name);
    let limit = query.limit.unwrap_or(25);
    let commits = state
        .core
        .log(&project, &repo, Some(&at), Some(limit), token.as_deref())?
        .iter()
        .map(commit_to_dto)
        .collect();
    let operations = state
        .core
        .op_log(&project, &repo, Some(limit), token.as_deref())?
        .iter()
        .map(operation_to_dto)
        .collect();
    Ok(Json(HistoryDto {
        commits,
        operations,
    }))
}

#[utoipa::path(
    post,
    path = "/api/codegen/preview",
    tag = "artifacts",
    operation_id = "previewCodegen",
    request_body = PreviewCodegenRequestDto,
    responses(
        (status = 200, description = "Generated source preview", body = CodegenPreviewDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn preview_codegen(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PreviewCodegenRequestDto>,
) -> Result<Json<CodegenPreviewDto>, ApiError> {
    let token = auth_token(&headers)?;
    let schema = SchemaPath::new(&request.project, &request.repo, &request.schema_path);
    let lang = match request.language.as_str() {
        "rust" => Language::Rust,
        "typescript" => Language::TypeScript,
        other => {
            return Err(ApiError::bad_request(format!(
                "unsupported codegen language {other:?}"
            )))
        }
    };
    let options = CodegenOptions {
        rust_pluggable_buffer: request.rust_pluggable_buffer.unwrap_or(false),
    };
    let content = state.core.preview_codegen_at(
        &schema,
        &refspec(&request.r#ref),
        lang,
        &options,
        token.as_deref(),
    )?;
    Ok(Json(CodegenPreviewDto {
        content,
        is_archive: false,
        at_commit: String::new(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/revisions/resolve",
    tag = "artifacts",
    operation_id = "resolveRevision",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("ref" = Option<String>, Query, description = "Bookmark, tag:<name>, or @<commit>")
    ),
    responses(
        (status = 200, description = "Immutable revision resolved from the requested ref", body = RevisionDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn resolve_revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<RefQuery>,
) -> Result<Json<RevisionDto>, ApiError> {
    let token = auth_token(&headers)?;
    let ref_name = effective_ref(&state, &project, &repo, &query.r#ref, token.as_deref())?;
    let revision = state.core.resolve_schema_revision(
        &project,
        &repo,
        &refspec(&ref_name),
        ref_name,
        token.as_deref(),
    )?;
    Ok(Json(RevisionDto {
        name: revision.name,
        project: revision.project,
        repo: revision.repo,
        commit_id: revision.commit_id,
        resolved_from: revision.resolved_from,
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{project}/repos/{repo}/revisions/{commit}/artifacts/{schema_path}",
    tag = "artifacts",
    operation_id = "getSchemaArtifact",
    params(
        ("project" = String, Path, description = "Project resource ID"),
        ("repo" = String, Path, description = "Repository resource ID"),
        ("commit" = String, Path, description = "Immutable commit ID"),
        ("schema_path" = String, Path, description = "Slash-preserving schema path"),
        ("kind" = Option<String>, Query, description = "source, descriptors, or generated-code"),
        ("language" = Option<String>, Query, description = "rust or typescript for generated-code"),
        ("rustPluggableBuffer" = Option<bool>, Query, description = "Enable FlatBuffers Rust pluggable-buffer output")
    ),
    responses(
        (status = 200, description = "Immutable source, descriptor, or generated-code bytes", body = String),
        (status = 304, description = "If-None-Match matched the artifact digest"),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn schema_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo, commit, schema_path)): Path<(String, String, String, String)>,
    Query(query): Query<ArtifactQuery>,
) -> Result<Response, ApiError> {
    let token = auth_token(&headers)?;
    let kind = match query.kind.as_str() {
        "source" => SchemaArtifactKind::Source,
        "descriptors" | "descriptor" => SchemaArtifactKind::Descriptors,
        "generated-code" | "generated_code" | "code" => SchemaArtifactKind::GeneratedCode,
        other => {
            return Err(ApiError::bad_request(format!(
                "unsupported artifact kind {other:?}"
            )))
        }
    };
    let language = match (kind, query.language.as_deref()) {
        (SchemaArtifactKind::GeneratedCode, Some("rust")) => Some(Language::Rust),
        (SchemaArtifactKind::GeneratedCode, Some("typescript" | "ts")) => {
            Some(Language::TypeScript)
        }
        (SchemaArtifactKind::GeneratedCode, Some(other)) => {
            return Err(ApiError::bad_request(format!(
                "unsupported artifact language {other:?}"
            )))
        }
        (SchemaArtifactKind::GeneratedCode, None) => {
            return Err(ApiError::bad_request(
                "generated-code artifacts require language",
            ))
        }
        (_, _) => None,
    };
    let revision = format!("projects/{project}/repos/{repo}/revisions/{commit}");
    let artifact = state.core.get_schema_artifact(
        &revision,
        &schema_path,
        kind,
        language,
        &CodegenOptions {
            rust_pluggable_buffer: query.rust_pluggable_buffer,
        },
        token.as_deref(),
    )?;
    let etag = format!("\"{}\"", artifact.artifact_digest);
    let not_modified = headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag || value == artifact.artifact_digest);
    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        artifact.content.into_response()
    };
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&etag)
            .map_err(|_| ApiError::internal("artifact digest is invalid HTTP metadata"))?,
    );
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&artifact.media_type)
            .map_err(|_| ApiError::internal("artifact media type is invalid HTTP metadata"))?,
    );
    response.headers_mut().insert(
        "x-schemahub-closure-digest",
        HeaderValue::from_str(&artifact.closure_digest)
            .map_err(|_| ApiError::internal("closure digest is invalid HTTP metadata"))?,
    );
    Ok(response)
}

#[utoipa::path(
    get,
    path = "/api/admin/config",
    tag = "discovery",
    operation_id = "getServerConfig",
    responses((status = 200, description = "Public server capability configuration", body = ServerConfigDto))
)]
async fn server_config(State(state): State<AppState>) -> Json<ServerConfigDto> {
    Json(ServerConfigDto {
        storage_backend: state.storage_backend,
        auth_mode: state.auth_mode,
        max_ops_per_transaction: 100,
        max_schemas_per_transaction: 20,
        supported_formats: vec![
            "protobuf".to_string(),
            "flatbuffers".to_string(),
            "openapi".to_string(),
        ],
    })
}

#[utoipa::path(
    get,
    path = "/api/session",
    tag = "identity",
    operation_id = "getSession",
    responses(
        (status = 200, description = "Server-derived caller identity", body = SessionDto),
        (status = "default", description = "Request failed", content((ApiErrorDto = "application/json"), (String = "text/plain")))
    )
)]
async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionDto>, ApiError> {
    let token = auth_token(&headers)?;
    let identity = state.core.resolve_identity(token.as_deref())?;
    Ok(Json(SessionDto {
        authenticated: !identity.is_anonymous(),
        id: identity.id().map(str::to_string),
        display: identity.display().map(str::to_string),
        kind: format!("{:?}", identity.kind()).to_ascii_lowercase(),
        delegated_by: identity.delegated_by().map(str::to_string),
    }))
}

fn auth_token(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(raw) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let value = raw
        .to_str()
        .map_err(|_| ApiError::unauthorized("authorization header is not valid ASCII"))?;
    let token = value.trim_start_matches("Bearer ").trim();
    Ok((!token.is_empty()).then(|| token.to_string()))
}

fn refspec(value: &str) -> RefSpec {
    if let Some(commit) = value.strip_prefix('@') {
        RefSpec::commit(commit.to_string())
    } else if let Some(tag) = value.strip_prefix("tag:") {
        RefSpec::Tag(tag.to_string())
    } else {
        RefSpec::bookmark(if value.is_empty() { "main" } else { value })
    }
}

fn effective_ref(
    state: &AppState,
    project: &str,
    repo: &str,
    requested: &str,
    token: Option<&str>,
) -> Result<String, ApiError> {
    if !requested.is_empty() {
        return Ok(requested.to_string());
    }
    state
        .core
        .get_repository(project, repo, false, token)?
        .map(|repository| repository.config.default_bookmark)
        .ok_or_else(|| ApiError::not_found(format!("repository {project}/{repo} not found")))
}

fn is_missing_bookmark(error: &CoreError) -> bool {
    matches!(error, CoreError::Jj(JjError::BookmarkNotFound(_)))
}

fn schema_format(schema_path: &str) -> &'static str {
    detect_format_from_name(schema_path).unwrap_or("openapi")
}

fn compatibility_to_string(direction: CompatibilityDirection) -> String {
    match direction {
        CompatibilityDirection::Backward => "backward",
        CompatibilityDirection::Forward => "forward",
        CompatibilityDirection::Full => "full",
        CompatibilityDirection::Disabled => "disabled",
    }
    .to_string()
}

fn repo_summary(project: &str, repo: &str, config: &RepoConfig) -> RepoSummaryDto {
    RepoSummaryDto {
        project: project.to_string(),
        repo: repo.to_string(),
        default_branch: config.default_bookmark.clone(),
        protected_branches: config.protected_bookmarks.clone(),
        compatibility: compatibility_to_string(config.compatibility_direction),
    }
}

fn change_resource_name(project: &str, repo: &str, change_id: &str) -> String {
    format!("projects/{project}/repos/{repo}/changes/{change_id}")
}

fn change_edits_from_dto(
    project: &str,
    repo: &str,
    edits: Vec<ChangeEditInputDto>,
) -> Result<Vec<ChangeEdit>, ApiError> {
    edits
        .into_iter()
        .map(|edit| change_edit_from_dto(project, repo, edit))
        .collect()
}

fn change_edit_from_dto(
    project: &str,
    repo: &str,
    edit: ChangeEditInputDto,
) -> Result<ChangeEdit, ApiError> {
    let schema_path = edit.schema_path.trim();
    let format_id = edit.format_id.trim();
    if schema_path.is_empty() {
        return Err(ApiError::bad_request(
            "change edit schemaPath must not be empty",
        ));
    }
    let detected_format = detect_format_from_name(schema_path).ok_or_else(|| {
        ApiError::bad_request(
            "change edit schemaPath must end in .proto, .fbs, .yaml, .yml, or .json",
        )
    })?;
    if format_id != detected_format {
        return Err(ApiError::bad_request(format!(
            "change edit formatId {format_id:?} does not match {schema_path:?} ({detected_format})"
        )));
    }

    let schema = SchemaPath::new(project, repo, schema_path);
    match edit.kind.as_str() {
        "replace_source" => {
            let source = edit.source.ok_or_else(|| {
                ApiError::bad_request("replace_source change edit requires source")
            })?;
            if source.trim().is_empty() {
                return Err(ApiError::bad_request(
                    "replace_source change edit source must not be empty",
                ));
            }
            Ok(ChangeEdit::ReplaceSource {
                schema,
                format_id: format_id.to_string(),
                source,
            })
        }
        "delete_schema" => {
            if edit.source.is_some() {
                return Err(ApiError::bad_request(
                    "delete_schema change edit must not include source",
                ));
            }
            Ok(ChangeEdit::DeleteSchema {
                schema,
                format_id: format_id.to_string(),
            })
        }
        other => Err(ApiError::bad_request(format!(
            "unsupported browser change edit kind {other:?}; expected replace_source or delete_schema"
        ))),
    }
}

fn required_query_value(value: Option<String>, field: &str) -> Result<String, ApiError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request(format!("{field} query parameter is required")))
}

fn gui_page_size(requested: i32) -> Result<usize, ApiError> {
    if requested < 0 {
        return Err(ApiError::bad_request("pageSize must not be negative"));
    }
    Ok(if requested == 0 {
        DEFAULT_GUI_PAGE_SIZE
    } else {
        (requested as usize).min(MAX_GUI_PAGE_SIZE)
    })
}

fn parse_gui_catalog_page_token(
    token: &str,
    kind: &str,
    scope: &str,
    name_prefix: &str,
) -> Result<Option<String>, ApiError> {
    if token.is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = token.splitn(5, ':').collect();
    let decoded = |value: &str| {
        hex::decode(value)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    if parts.len() != 5
        || parts[0] != "v1"
        || parts[1] != kind
        || decoded(parts[2]).as_deref() != Some(scope)
        || decoded(parts[3]).as_deref() != Some(name_prefix)
    {
        return Err(ApiError::bad_request(
            "pageToken is invalid for this catalog scope or prefix",
        ));
    }
    decoded(parts[4])
        .filter(|cursor| valid_gui_catalog_cursor(cursor, name_prefix))
        .map(Some)
        .ok_or_else(|| ApiError::bad_request("pageToken has an invalid catalog cursor"))
}

fn valid_gui_catalog_cursor(cursor: &str, name_prefix: &str) -> bool {
    !cursor.trim().is_empty()
        && cursor.len() <= 128
        && !cursor.contains('/')
        && !cursor.chars().any(char::is_control)
        && cursor.starts_with(name_prefix)
}

fn make_gui_catalog_page_token(kind: &str, scope: &str, name_prefix: &str, cursor: &str) -> String {
    format!(
        "v1:{kind}:{}:{}:{}",
        hex::encode(scope),
        hex::encode(name_prefix),
        hex::encode(cursor)
    )
}

fn gui_change_status(value: &str) -> Result<Option<ChangeRecordStatus>, ApiError> {
    match value {
        "" => Ok(None),
        "draft" => Ok(Some(ChangeRecordStatus::Draft)),
        "ready" => Ok(Some(ChangeRecordStatus::Ready)),
        "applying" => Ok(Some(ChangeRecordStatus::Applying)),
        "applied" => Ok(Some(ChangeRecordStatus::Applied)),
        "rejected" => Ok(Some(ChangeRecordStatus::Rejected)),
        "abandoned" => Ok(Some(ChangeRecordStatus::Abandoned)),
        _ => Err(ApiError::bad_request(
            "status must be draft, ready, applying, applied, rejected, or abandoned",
        )),
    }
}

fn parse_gui_change_page_token(
    token: &str,
    project: &str,
    repo: &str,
    status: &str,
) -> Result<Option<ChangeRecordPageCursor>, ApiError> {
    if token.is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = token.splitn(7, ':').collect();
    let decoded = |value: &str| {
        hex::decode(value)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    let create_time_unix_ms = parts.get(5).and_then(|value| value.parse::<i64>().ok());
    let name = parts.get(6).and_then(|value| decoded(value));
    let expected_prefix = format!("projects/{project}/repos/{repo}/changes/");
    if parts.len() != 7
        || parts[0] != "v1"
        || parts[1] != CHANGE_TOKEN_KIND
        || decoded(parts[2]).as_deref() != Some(project)
        || decoded(parts[3]).as_deref() != Some(repo)
        || decoded(parts[4]).as_deref() != Some(status)
        || create_time_unix_ms.is_none_or(|value| value < 0)
        || name
            .as_deref()
            .and_then(|value| value.strip_prefix(&expected_prefix))
            .is_none_or(|change_id| {
                change_id.is_empty()
                    || change_id.contains('/')
                    || change_id.chars().any(char::is_control)
            })
    {
        return Err(ApiError::bad_request(
            "pageToken is invalid for this ChangeRecord parent or status",
        ));
    }
    Ok(Some(ChangeRecordPageCursor {
        create_time_unix_ms: create_time_unix_ms.expect("validated above"),
        name: name.expect("validated above"),
    }))
}

fn make_gui_change_page_token(
    project: &str,
    repo: &str,
    status: &str,
    cursor: &ChangeRecordPageCursor,
) -> String {
    format!(
        "v1:{CHANGE_TOKEN_KIND}:{}:{}:{}:{}:{}",
        hex::encode(project),
        hex::encode(repo),
        hex::encode(status),
        cursor.create_time_unix_ms,
        hex::encode(&cursor.name),
    )
}

fn parse_dashboard_page_token(
    token: &str,
    project: &str,
    repo: &str,
    ref_name: &str,
) -> Result<Option<DashboardContinuation>, ApiError> {
    if token.is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = token.splitn(9, ':').collect();
    let decoded = |index: usize| {
        parts
            .get(index)
            .and_then(|value| hex::decode(value).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    if parts.len() != 9
        || parts[0] != "v1"
        || parts[1] != DASHBOARD_TOKEN_KIND
        || decoded(2).as_deref() != Some(project)
        || decoded(3).as_deref() != Some(repo)
        || decoded(4).as_deref() != Some(ref_name)
    {
        return Err(ApiError::bad_request(
            "pageToken is invalid for this dashboard repository or ref",
        ));
    }
    let resolved_commit = decoded(5).ok_or_else(|| {
        ApiError::bad_request("pageToken has invalid dashboard snapshot encoding")
    })?;
    let schema_cursor = decoded(6)
        .ok_or_else(|| ApiError::bad_request("pageToken has invalid schema cursor encoding"))?;
    let branch_cursor = decoded(7)
        .ok_or_else(|| ApiError::bad_request("pageToken has invalid branch cursor encoding"))?;
    let tag_cursor = decoded(8)
        .ok_or_else(|| ApiError::bad_request("pageToken has invalid tag cursor encoding"))?;
    let optional_cursor = |value: String| (!value.is_empty()).then_some(value);
    let continuation = DashboardContinuation {
        resolved_commit,
        schema_cursor: optional_cursor(schema_cursor),
        branch_cursor: optional_cursor(branch_cursor),
        tag_cursor: optional_cursor(tag_cursor),
    };
    let commit_valid = continuation.resolved_commit.is_empty()
        || (continuation.resolved_commit.len() <= 128
            && continuation
                .resolved_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()));
    let cursors_valid = [
        continuation.schema_cursor.as_deref(),
        continuation.branch_cursor.as_deref(),
        continuation.tag_cursor.as_deref(),
    ]
    .into_iter()
    .flatten()
    .all(|cursor| {
        cursor.len() <= 1_024 && !cursor.starts_with('/') && !cursor.chars().any(char::is_control)
    });
    if !commit_valid
        || !cursors_valid
        || (continuation.resolved_commit.is_empty() && continuation.schema_cursor.is_some())
        || (continuation.schema_cursor.is_none()
            && continuation.branch_cursor.is_none()
            && continuation.tag_cursor.is_none())
    {
        return Err(ApiError::bad_request(
            "pageToken contains an invalid dashboard continuation",
        ));
    }
    Ok(Some(continuation))
}

#[allow(clippy::too_many_arguments)]
fn make_dashboard_page_token(
    project: &str,
    repo: &str,
    ref_name: &str,
    resolved_commit: &str,
    schema_cursor: Option<&str>,
    branch_cursor: Option<&str>,
    tag_cursor: Option<&str>,
) -> String {
    format!(
        "v1:{DASHBOARD_TOKEN_KIND}:{}:{}:{}:{}:{}:{}:{}",
        hex::encode(project),
        hex::encode(repo),
        hex::encode(ref_name),
        hex::encode(resolved_commit),
        hex::encode(schema_cursor.unwrap_or_default()),
        hex::encode(branch_cursor.unwrap_or_default()),
        hex::encode(tag_cursor.unwrap_or_default()),
    )
}

fn conflict_summary(path: String) -> ConflictSummaryDto {
    match path.rsplit_once('/') {
        Some((schema_path, declaration_name)) => ConflictSummaryDto {
            schema_path: schema_path.to_string(),
            declaration_name: declaration_name.to_string(),
        },
        None => ConflictSummaryDto {
            schema_path: String::new(),
            declaration_name: path,
        },
    }
}

fn change_to_dto(change: ChangeRecord, include_edit_source: bool) -> ChangeRecordDto {
    let ChangeRecord {
        name,
        project,
        repo,
        target_bookmark,
        base_revision,
        title,
        description,
        external_references,
        edits,
        created_by,
        status,
        validation,
        reviews,
        apply_attempt: _,
        apply_result,
        etag,
        create_time_unix_ms,
        update_time_unix_ms,
    } = change;
    ChangeRecordDto {
        name,
        project,
        repo,
        target_bookmark,
        base_revision,
        title,
        description,
        external_references,
        edits: edits
            .into_iter()
            .map(|edit| change_edit_to_dto(edit, include_edit_source))
            .collect(),
        created_by: change_actor_to_dto(created_by),
        status: change_status_to_string(status),
        validation: validation.map(change_validation_to_dto),
        reviews: reviews.into_iter().map(change_review_to_dto).collect(),
        apply_result: apply_result.map(change_apply_result_to_dto),
        etag,
        create_time_unix_ms,
        update_time_unix_ms,
    }
}

fn change_actor_to_dto(actor: ChangeActor) -> ChangeActorDto {
    ChangeActorDto {
        identity: actor.identity,
        kind: format!("{:?}", actor.kind).to_ascii_lowercase(),
        display_name: actor.display_name,
        delegated_by: actor.delegated_by,
    }
}

fn change_edit_to_dto(edit: ChangeEdit, include_source: bool) -> ChangeEditDto {
    let (kind, schema, format_id, source) = match edit {
        ChangeEdit::Mutation {
            schema, format_id, ..
        } => ("mutation", schema, format_id, None),
        ChangeEdit::ReplaceSource {
            schema,
            format_id,
            source,
        } => (
            "replace_source",
            schema,
            format_id,
            include_source.then_some(source),
        ),
        ChangeEdit::DeleteSchema { schema, format_id } => {
            ("delete_schema", schema, format_id, None)
        }
    };
    ChangeEditDto {
        kind: kind.to_string(),
        schema_path: schema.schema_name,
        format_id,
        source,
    }
}

fn change_status_to_string(status: ChangeRecordStatus) -> String {
    match status {
        ChangeRecordStatus::Draft => "draft",
        ChangeRecordStatus::Ready => "ready",
        ChangeRecordStatus::Applying => "applying",
        ChangeRecordStatus::Applied => "applied",
        ChangeRecordStatus::Rejected => "rejected",
        ChangeRecordStatus::Abandoned => "abandoned",
    }
    .to_string()
}

fn change_validation_to_dto(validation: ValidationResult) -> ChangeValidationDto {
    ChangeValidationDto {
        valid: validation.valid,
        resolved_base_commit: validation.resolved_base_commit,
        edit_digest: validation.edit_digest,
        issues: validation
            .issues
            .into_iter()
            .map(|issue| ValidationIssueDto {
                code: issue.code,
                message: issue.message,
                schema_name: issue.schema_name,
                declaration_name: issue.declaration_name,
            })
            .collect(),
        validated_at_unix_ms: validation.validated_at_unix_ms,
        validator_version: validation.validator_version,
    }
}

fn change_review_to_dto(review: ChangeReview) -> ChangeReviewDto {
    ChangeReviewDto {
        reviewer: change_actor_to_dto(review.reviewer),
        decision: match review.decision {
            ChangeReviewDecision::Approved => "approved",
            ChangeReviewDecision::Rejected => "rejected",
        }
        .to_string(),
        reason: review.reason,
        create_time_unix_ms: review.create_time_unix_ms,
    }
}

fn change_apply_result_to_dto(result: ApplyResult) -> ChangeApplyResultDto {
    ChangeApplyResultDto {
        commit_id: result.commit_id,
        change_id: result.change_id,
        operation_id: result.operation_id,
        conflicted_declarations: result.conflicted_declarations,
        artifact_digest: result.artifact_digest,
    }
}

fn decl_kind_to_string(kind: DeclKind) -> String {
    match kind {
        DeclKind::Message => "message",
        DeclKind::Enum | DeclKind::FbsEnum => "enum",
        DeclKind::Service => "service",
        DeclKind::Table => "table",
        DeclKind::Struct => "struct",
        DeclKind::Union => "union",
        DeclKind::PathItem => "path",
        DeclKind::ComponentSchema
        | DeclKind::ComponentParameter
        | DeclKind::ComponentResponse
        | DeclKind::ComponentRequestBody
        | DeclKind::DocumentMetadata => "schema",
    }
    .to_string()
}

fn diff_change_to_dto(schema_path: &str, change: DeclChange) -> DiffChangeDto {
    let (kind, declaration, summary) = match change {
        DeclChange::DeclarationAdded { name } => {
            ("added", name.clone(), format!("Added declaration {name}"))
        }
        DeclChange::DeclarationRemoved { name } => (
            "removed",
            name.clone(),
            format!("Removed declaration {name}"),
        ),
        DeclChange::DeclarationModified { name, .. } => (
            "modified",
            name.clone(),
            format!("Modified declaration {name}"),
        ),
    };
    DiffChangeDto {
        schema_path: schema_path.to_string(),
        declaration,
        kind: kind.to_string(),
        compatibility: "unknown".to_string(),
        summary,
    }
}

fn commit_to_dto(entry: &LogEntry) -> CommitEntryDto {
    CommitEntryDto {
        commit: entry.commit_id.clone(),
        change_id: entry.change_id.clone(),
        parents: entry.parents.clone(),
        author: entry.author.clone(),
        message: entry.message.clone(),
        timestamp: entry.timestamp.clone(),
    }
}

fn operation_to_dto(entry: &OperationRecord) -> OperationEntryDto {
    let action = entry
        .description
        .split_whitespace()
        .next()
        .unwrap_or("Operation")
        .to_string();
    OperationEntryDto {
        op_id: entry.op_id.clone(),
        author: entry.author.clone(),
        action,
        target: entry.description.clone(),
        before: None,
        after: None,
        timestamp: entry.timestamp.clone(),
    }
}

fn empty_commit() -> CommitEntryDto {
    CommitEntryDto {
        commit: String::new(),
        change_id: String::new(),
        parents: Vec::new(),
        author: String::new(),
        message: String::new(),
        timestamp: String::new(),
    }
}

fn empty_op() -> OperationEntryDto {
    OperationEntryDto {
        op_id: String::new(),
        author: String::new(),
        action: String::new(),
        target: String::new(),
        before: None,
        after: None,
        timestamp: String::new(),
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

#[derive(Serialize, ToSchema)]
struct ApiErrorDto {
    error: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        let status = crate::error::to_status(error);
        let http_status = match status.code() {
            tonic::Code::Ok => StatusCode::OK,
            tonic::Code::InvalidArgument | tonic::Code::OutOfRange => StatusCode::BAD_REQUEST,
            tonic::Code::Unauthenticated => StatusCode::UNAUTHORIZED,
            tonic::Code::PermissionDenied => StatusCode::FORBIDDEN,
            tonic::Code::NotFound => StatusCode::NOT_FOUND,
            tonic::Code::AlreadyExists => StatusCode::CONFLICT,
            tonic::Code::Aborted | tonic::Code::FailedPrecondition => {
                StatusCode::PRECONDITION_FAILED
            }
            tonic::Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
            tonic::Code::Cancelled | tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            tonic::Code::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
            tonic::Code::Unimplemented => StatusCode::NOT_IMPLEMENTED,
            tonic::Code::Unknown | tonic::Code::Internal | tonic::Code::DataLoss => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        Self {
            status: http_status,
            message: status.message().to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorDto {
                error: self.message,
            }),
        )
            .into_response()
    }
}
