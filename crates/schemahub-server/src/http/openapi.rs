//! OpenAPI-backed route assembly for the browser HTTP boundary.
//!
//! Every ordinary route is registered through `utoipa-axum`, so its runtime
//! handler and generated operation cannot drift apart. Axum 0.7 catch-all
//! syntax (`*schema_path`) cannot be represented directly by OpenAPI path
//! templates; those two routes are registered explicitly and their annotated
//! `{schema_path}` operations are merged into the document.

use axum::routing::get;
use axum::Router;
use utoipa::openapi::extensions::Extensions;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{Components, InfoBuilder, OpenApi, Paths, Tag};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::*;

pub(super) fn http_router() -> Router<AppState> {
    let router: Router<AppState> = documented_router().into();
    router
        .route(
            "/api/projects/:project/repos/:repo/schemas/*schema_path",
            get(schema_detail),
        )
        .route(
            "/api/projects/:project/repos/:repo/revisions/:commit/artifacts/*schema_path",
            get(schema_artifact),
        )
}

pub(super) fn build_document() -> OpenApi {
    let mut document = documented_router().into_openapi();
    document.merge(catch_all_document());
    classify_http_paths(&mut document);
    document
}

fn classify_http_paths(document: &mut OpenApi) {
    for (path, item) in &mut document.paths.paths {
        let (surface, compatibility) = match path.as_str() {
            path if path.starts_with("/api/") => (HTTP_API_SURFACE_GUI_BFF, "excluded"),
            "/healthz" | "/readyz" | "/metrics" => ("operations", "supported"),
            _ => continue,
        };
        let extensions = item.extensions.get_or_insert_default();
        extensions.insert("x-schemahub-api-surface".to_string(), surface.into());
        extensions.insert(
            "x-schemahub-compatibility-promise".to_string(),
            compatibility.into(),
        );
    }
}

fn documented_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(base_document())
        .routes(routes!(liveness))
        .routes(routes!(readiness_probe))
        .routes(routes!(prometheus_metrics))
        .routes(routes!(serve_openapi_document))
        .routes(routes!(list_projects))
        .routes(routes!(list_repos))
        .routes(routes!(list_changes, create_change))
        .routes(routes!(get_change))
        .routes(routes!(change_action))
        .routes(routes!(repo_dashboard))
        .routes(routes!(search_resources))
        .routes(routes!(list_conflicts))
        .routes(routes!(render_conflict))
        .routes(routes!(resolve_conflict))
        .routes(routes!(diff))
        .routes(routes!(history))
        .routes(routes!(preview_codegen))
        .routes(routes!(resolve_revision))
        .routes(routes!(server_config))
        .routes(routes!(session))
}

fn catch_all_document() -> OpenApi {
    OpenApiRouter::<AppState>::default()
        .routes(routes!(schema_detail))
        .routes(routes!(schema_artifact))
        .into_openapi()
}

fn base_document() -> OpenApi {
    let extensions: Extensions = [
        ("x-schemahub-public-api", "schemahub.v1"),
        ("x-schemahub-gui-bff-prefix", "/api/"),
    ]
    .into_iter()
    .collect();
    let info = InfoBuilder::new()
        .title("SchemaHub HTTP Interfaces")
        .version(crate::BUILD_VERSION)
        .description(Some(
            "Generated contract for SchemaHub's co-located HTTP interfaces. The unversioned /api/* routes are a browser backend-for-frontend outside the public API compatibility promise and may evolve with the bundled GUI. The public versioned 1.0 API is the schemahub.v1 gRPC/protobuf contract. /healthz, /readyz, and /metrics are separately supported operational interfaces. Authentication is deployment-configurable: callers may supply a bearer token, while noop deployments and operational discovery endpoints allow anonymous requests.",
        ))
        .extensions(Some(extensions))
        .build();
    let mut components = Components::new();
    components.add_security_scheme(
        "bearerAuth",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("JWT or configured opaque token")
                .description(Some(
                    "SchemaHub bearer identity. JWT, static-token, and noop behavior is selected by server configuration.",
                ))
                .build(),
        ),
    );

    let mut document = OpenApi::new(info, Paths::new());
    document.components = Some(components);
    document.security = Some(vec![
        SecurityRequirement::new("bearerAuth", Vec::<String>::new()),
        SecurityRequirement::default(),
    ]);
    document.tags = Some(
        [
            "operations",
            "discovery",
            "identity",
            "projects",
            "repositories",
            "changes",
            "schemas",
            "history",
            "conflicts",
            "artifacts",
        ]
        .into_iter()
        .map(Tag::new)
        .collect(),
    );
    document
}
