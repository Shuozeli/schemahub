//! HTTP/JSON BFF for the SchemaHub web console.
//!
//! This is intentionally read-mostly and DTO-oriented. The browser should not
//! import Rust/protobuf internals; it talks to these stable UI shapes while the
//! BFF adapts them to `Core`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use schemahub_core::{detect_format_from_name, Core, CoreError, LogEntry, OperationRecord};
use schemahub_jj::{JjError, RefSpec};
use schemahub_types::{
    CodegenOptions, CompatibilityDirection, DeclChange, DeclKind, Language, SchemaPath,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct AppState {
    core: Arc<Core>,
    storage_backend: String,
}

pub async fn serve(
    core: Arc<Core>,
    storage_backend: String,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    let state = AppState {
        core,
        storage_backend,
    };
    let app = Router::new()
        .route("/api/projects", get(list_projects))
        .route(
            "/api/projects/:project/repos/:repo/dashboard",
            get(repo_dashboard),
        )
        .route(
            "/api/projects/:project/repos/:repo/schemas/*schema_path",
            get(schema_detail),
        )
        .route("/api/projects/:project/repos/:repo/diff", get(diff))
        .route("/api/projects/:project/repos/:repo/history", get(history))
        .route("/api/codegen/preview", post(preview_codegen))
        .route("/api/admin/config", get(server_config))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSummaryDto {
    name: String,
    visibility: String,
    role: String,
    repos: usize,
    last_operation: String,
    last_activity: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoSummaryDto {
    project: String,
    repo: String,
    default_branch: String,
    protected_branches: Vec<String>,
    compatibility: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaSummaryDto {
    path: String,
    format: String,
    declarations: usize,
    dependencies: usize,
    conflict_count: usize,
    last_commit: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoDashboardDto {
    repo: RepoSummaryDto,
    schemas: Vec<SchemaSummaryDto>,
    branches: Vec<String>,
    tags: Vec<String>,
    latest_commit: CommitEntryDto,
    latest_operation: OperationEntryDto,
    open_conflicts: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeclarationSummaryDto {
    name: String,
    kind: String,
    detail: String,
    refs: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DependencyDto {
    importing_schema: String,
    import_path: String,
    resolved_commit: String,
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaDetailDto {
    path: String,
    format: String,
    source: String,
    declarations: Vec<DeclarationSummaryDto>,
    dependencies: Vec<DependencyDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffResultDto {
    base: String,
    head: String,
    changes: Vec<DiffChangeDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffChangeDto {
    schema_path: String,
    declaration: String,
    kind: String,
    compatibility: String,
    summary: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryDto {
    commits: Vec<CommitEntryDto>,
    operations: Vec<OperationEntryDto>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CommitEntryDto {
    commit: String,
    change_id: String,
    parents: Vec<String>,
    author: String,
    message: String,
    timestamp: String,
}

#[derive(Serialize, Clone)]
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewCodegenRequestDto {
    project: String,
    repo: String,
    schema_path: String,
    r#ref: String,
    language: String,
    rust_pluggable_buffer: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CodegenPreviewDto {
    content: String,
    is_archive: bool,
    at_commit: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfigDto {
    storage_backend: String,
    auth_mode: String,
    max_ops_per_transaction: usize,
    max_schemas_per_transaction: usize,
    supported_formats: Vec<String>,
}

#[derive(Deserialize)]
struct RefQuery {
    #[serde(default = "default_ref")]
    r#ref: String,
}

#[derive(Deserialize)]
struct DiffQuery {
    #[serde(default = "default_ref")]
    base: String,
    #[serde(default = "default_ref")]
    head: String,
    #[serde(rename = "schemaPath")]
    schema_path: Option<String>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default = "default_ref")]
    r#ref: String,
    limit: Option<usize>,
}

fn default_ref() -> String {
    "main".to_string()
}

async fn list_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProjectSummaryDto>>, ApiError> {
    let token = auth_token(&headers);
    let projects = state.core.list_projects(token.as_deref())?;
    Ok(Json(
        projects
            .into_iter()
            .map(|project| ProjectSummaryDto {
                name: project.name,
                visibility: match project.visibility {
                    schemahub_types::Visibility::Public => "public".to_string(),
                    schemahub_types::Visibility::Private => "private".to_string(),
                },
                role: "Reader".to_string(),
                repos: 0,
                last_operation: String::new(),
                last_activity: String::new(),
            })
            .collect(),
    ))
}

async fn repo_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<RefQuery>,
) -> Result<Json<RepoDashboardDto>, ApiError> {
    let token = auth_token(&headers);
    let at = refspec(&query.r#ref);
    let schemas = state
        .core
        .list_schemas(&project, &repo, &at, token.as_deref())?;
    let commits = state
        .core
        .log(&project, &repo, Some(&at), Some(1), token.as_deref())
        .unwrap_or_default();
    let ops = state
        .core
        .op_log(&project, &repo, Some(1), token.as_deref())
        .unwrap_or_default();
    let branches = state
        .core
        .list_bookmarks(&project, &repo, token.as_deref())
        .unwrap_or_default()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let tags = state
        .core
        .list_tags(&project, &repo, token.as_deref())
        .unwrap_or_default()
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    let latest_commit_id = commits
        .first()
        .map(|c| c.commit_id.clone())
        .unwrap_or_default();
    let mut schema_summaries = Vec::new();
    for schema_name in schemas {
        let path = SchemaPath::new(&project, &repo, &schema_name);
        let declarations = state
            .core
            .list_declarations(&path, &at, token.as_deref())
            .unwrap_or_default();
        let dependencies = state
            .core
            .list_dependencies(&path, &at, false, token.as_deref())
            .unwrap_or_default();
        schema_summaries.push(SchemaSummaryDto {
            format: schema_format(&schema_name).to_string(),
            path: schema_name,
            declarations: declarations.len(),
            dependencies: dependencies.len(),
            conflict_count: 0,
            last_commit: latest_commit_id.clone(),
        });
    }

    Ok(Json(RepoDashboardDto {
        repo: RepoSummaryDto {
            project: project.clone(),
            repo: repo.clone(),
            default_branch: "main".to_string(),
            protected_branches: vec!["main".to_string()],
            compatibility: compatibility_to_string(CompatibilityDirection::Full),
        },
        schemas: schema_summaries,
        branches,
        tags,
        latest_commit: commits
            .first()
            .map(commit_to_dto)
            .unwrap_or_else(empty_commit),
        latest_operation: ops.first().map(operation_to_dto).unwrap_or_else(empty_op),
        open_conflicts: 0,
    }))
}

async fn schema_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo, schema_path)): Path<(String, String, String)>,
    Query(query): Query<RefQuery>,
) -> Result<Json<SchemaDetailDto>, ApiError> {
    let token = auth_token(&headers);
    let at = refspec(&query.r#ref);
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

async fn diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<DiffResultDto>, ApiError> {
    let token = auth_token(&headers);
    let base = refspec(&query.base);
    let head = refspec(&query.head);
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
        base: query.base,
        head: query.head,
        changes,
    }))
}

async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, repo)): Path<(String, String)>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<HistoryDto>, ApiError> {
    let token = auth_token(&headers);
    let at = refspec(&query.r#ref);
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

async fn preview_codegen(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PreviewCodegenRequestDto>,
) -> Result<Json<CodegenPreviewDto>, ApiError> {
    let token = auth_token(&headers);
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

async fn server_config(State(state): State<AppState>) -> Json<ServerConfigDto> {
    Json(ServerConfigDto {
        storage_backend: state.storage_backend,
        auth_mode: "noop".to_string(),
        max_ops_per_transaction: 100,
        max_schemas_per_transaction: 20,
        supported_formats: vec![
            "protobuf".to_string(),
            "flatbuffers".to_string(),
            "openapi".to_string(),
        ],
    })
}

fn auth_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim_start_matches("Bearer ").trim().to_string())
        .filter(|value| !value.is_empty())
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

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            error: String,
        }

        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}
