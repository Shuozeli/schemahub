//! `schemahub-server` — the gRPC server and composition root
//! (crate-structure.md §3.6).
//!
//! Startup: load config (`schemahub.toml`, optional), open the configured
//! object store (redb embedded default, or Postgres behind the `postgres`
//! feature), build the `Core` over the three compilers, register every gRPC
//! service, and serve. Binds to `TAILSCALE_IP` (user infra convention) when set
//! and no explicit `--listen` is given, else `0.0.0.0`.

use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use schemahub_jj::{ObjectDb, RedbObjectDb};
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

use schemahub_server::{
    build_core, build_core_with_authn, build_router_with_health_and_metrics, config::Config, http,
    jwt_auth::JwtAuthRuntime, observability::ServerMetrics, BUILD_VERSION,
};
use tonic_health::ServingStatus;

#[derive(Parser, Debug)]
#[command(
    name = "schemahub-server",
    about = "schemahub gRPC and HTTP server",
    version = BUILD_VERSION
)]
struct Args {
    /// Print the generated OpenAPI 3.1 document for the HTTP/JSON API and exit.
    #[arg(long, conflicts_with = "check_ready")]
    print_openapi: bool,
    /// Check an HTTP readiness URL and exit without starting the server.
    /// Intended for distroless container health checks.
    #[arg(long, value_name = "URL")]
    check_ready: Option<String>,
    /// Listen address, e.g. "0.0.0.0:50051". Overrides config + TAILSCALE_IP.
    #[arg(long)]
    listen: Option<String>,
    /// Path to the redb database file. Overrides config `[storage].path`.
    /// Only honored when the configured backend is `redb`.
    #[arg(long)]
    db: Option<String>,
    /// Postgres connection URL. Overrides config `[storage].url`. Only honored
    /// when the configured backend is `postgres` (and the binary was built
    /// with `--features postgres`).
    #[arg(long)]
    db_url: Option<String>,
    /// Path to schemahub.toml. An explicitly supplied path must be readable.
    /// When omitted, ./schemahub.toml is loaded if present.
    #[arg(long)]
    config: Option<String>,
    /// Optional HTTP/JSON BFF listen address for the web console.
    #[arg(long)]
    http_listen: Option<String>,
    /// Production Vite bundle to serve from the HTTP listener. Overrides
    /// `[http].gui_dir` and requires `--http-listen`.
    #[arg(long, value_name = "DIRECTORY")]
    gui_dir: Option<PathBuf>,
    /// Maximum time to drain active gRPC and HTTP requests after shutdown.
    #[arg(long, env = "SCHEMAHUB_SHUTDOWN_TIMEOUT_SECONDS", default_value_t = 30)]
    shutdown_timeout_seconds: u64,
    /// Log encoding. JSON is intended for production collectors; pretty is
    /// convenient for interactive development.
    #[arg(long, env = "SCHEMAHUB_LOG_FORMAT", value_enum, default_value = "json")]
    log_format: LogFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LogFormat {
    Json,
    Pretty,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    if args.print_openapi {
        return match write_openapi(io::stdout().lock()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("schemahub could not render its OpenAPI document: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(url) = args.check_ready.as_deref() {
        return match check_readiness(url).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("schemahub readiness check failed: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    if let Err(error) = init_tracing(args.log_format) {
        eprintln!("schemahub-server could not initialize logging: {error:#}");
        return ExitCode::FAILURE;
    }

    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(
                event = "schemahub.server.failed",
                error = %format!("{error:#}"),
                "schemahub-server failed"
            );
            ExitCode::FAILURE
        }
    }
}

fn write_openapi(mut writer: impl Write) -> anyhow::Result<()> {
    writer
        .write_all(http::openapi_json_bytes())
        .context("writing generated OpenAPI document")?;
    Ok(())
}

async fn check_readiness(url: &str) -> anyhow::Result<()> {
    let url = validate_readiness_url(url)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(2))
        .build()
        .context("building readiness HTTP client")?;
    let response = client
        .get(url)
        .send()
        .await
        .context("requesting readiness URL")?;
    ensure_readiness_status(response.status())
}

fn validate_readiness_url(url: &str) -> anyhow::Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).context("parsing readiness URL")?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https"),
        "readiness URL scheme must be http or https"
    );
    anyhow::ensure!(
        parsed.host_str().is_some(),
        "readiness URL must include a host"
    );
    Ok(parsed)
}

fn ensure_readiness_status(status: reqwest::StatusCode) -> anyhow::Result<()> {
    anyhow::ensure!(
        status.is_success(),
        "readiness endpoint returned HTTP {status}"
    );
    Ok(())
}

async fn run(args: Args) -> anyhow::Result<()> {
    let config = match args.config.as_deref() {
        Some(path) => Config::load(path)?,
        None => Config::load_optional("schemahub.toml")?,
    };

    let db = open_object_db(&args, &config)?;
    let jwt_runtime = match config.auth.jwt.as_ref() {
        Some(jwt) => Some(JwtAuthRuntime::initialize(jwt).await?),
        None => None,
    };
    let core = match jwt_runtime.as_ref() {
        Some(runtime) => build_core_with_authn(db.clone(), &config, runtime.provider()),
        None => build_core(db.clone(), &config),
    };

    let addr = resolve_listen_addr(args.listen, &config)?;
    let http_addr = resolve_http_listen_addr(args.http_listen)?;
    let gui_dir_override = args.gui_dir.clone();
    let effective_gui_dir = gui_dir_override
        .as_deref()
        .or(config.http.gui_dir.as_deref());
    ensure_gui_has_http_listener(http_addr, effective_gui_dir)?;
    let shutdown_timeout = Duration::from_secs(args.shutdown_timeout_seconds);
    let readiness = http::Readiness::new(false);
    let metrics = ServerMetrics::default();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut servers = JoinSet::new();

    if let Some(jwt_runtime) = jwt_runtime {
        let jwt_shutdown = shutdown_rx.clone();
        let jwt_readiness = readiness.clone();
        servers.spawn(async move { jwt_runtime.run(jwt_shutdown, jwt_readiness).await });
    }

    // Surface a MagicDNS-friendly hint when bound to the Tailscale IP.
    tracing::info!(
        event = "schemahub.listener.started",
        protocol = "grpc",
        listen_addr = %addr,
        "gRPC listener starting"
    );
    let grpc_core = core.clone();
    let grpc_storage_backend = config.storage.backend.clone();
    let grpc_shutdown = shutdown_rx.clone();
    let (grpc_router, mut health_reporter) =
        build_router_with_health_and_metrics(grpc_core, grpc_storage_backend, metrics.clone());
    health_reporter
        .set_service_status("", ServingStatus::NotServing)
        .await;
    servers.spawn(async move {
        grpc_router
            .serve_with_shutdown(addr, wait_for_shutdown(grpc_shutdown))
            .await
            .context("serving gRPC")?;
        Ok("gRPC")
    });

    if let Some(addr) = http_addr {
        tracing::info!(
            event = "schemahub.listener.started",
            protocol = "http",
            listen_addr = %addr,
            "HTTP BFF listener starting"
        );
        let http_core = core.clone();
        let http_db = db.clone();
        let storage_backend = config.storage.backend.clone();
        let auth_mode = config.auth_mode().to_string();
        let http_readiness = readiness.clone();
        let http_shutdown = shutdown_rx.clone();
        let http_policy =
            http::HttpPolicy::from_config_with_gui_dir(&config.http, gui_dir_override)
                .context("building HTTP boundary policy")?;
        let http_app = http::router_with_metrics_and_policy(
            http_core,
            http_db,
            storage_backend,
            auth_mode,
            http_readiness,
            metrics,
            http_policy,
        );
        servers.spawn(async move {
            http::serve(http_app, addr, wait_for_shutdown(http_shutdown))
                .await
                .context("serving HTTP BFF")?;
            Ok("HTTP BFF")
        });
    }

    readiness.mark_ready();
    health_reporter
        .set_service_status("", ServingStatus::Serving)
        .await;

    let termination_error = tokio::select! {
        signal = shutdown_signal() => match signal {
            Ok(signal) => {
                tracing::info!(
                    event = "schemahub.shutdown.started",
                    signal,
                    grace_period_seconds = shutdown_timeout.as_secs(),
                    "shutdown signal received; draining requests"
                );
                None
            }
            Err(error) => Some(error),
        },
        completed = servers.join_next() => Some(unexpected_server_exit(completed)),
    };

    // Stop advertising readiness before notifying either listener. In-flight
    // work receives a bounded grace period; new orchestration traffic can move
    // elsewhere immediately.
    readiness.mark_draining();
    health_reporter
        .set_service_status("", ServingStatus::NotServing)
        .await;
    let _ = shutdown_tx.send(true);

    let drain = async {
        let mut errors = Vec::new();
        while let Some(completed) = servers.join_next().await {
            match completed {
                Ok(Ok(service)) => tracing::info!(
                    event = "schemahub.listener.drained",
                    service,
                    "listener drained"
                ),
                Ok(Err(error)) => errors.push(format!("{error:#}")),
                Err(error) if error.is_cancelled() => {}
                Err(error) => errors.push(format!("server task failed: {error}")),
            }
        }
        errors
    };

    let drain_errors = match tokio::time::timeout(shutdown_timeout, drain).await {
        Ok(errors) => errors,
        Err(_) => {
            servers.abort_all();
            while servers.join_next().await.is_some() {}
            vec![format!(
                "graceful shutdown exceeded {} seconds",
                shutdown_timeout.as_secs()
            )]
        }
    };

    if let Some(error) = termination_error {
        return Err(error);
    }
    if !drain_errors.is_empty() {
        anyhow::bail!("server shutdown failed: {}", drain_errors.join("; "));
    }
    Ok(())
}

fn init_tracing(log_format: LogFormat) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("schemahub_server=info,schemahub_core=info,tower_http=info")
    });
    match log_format {
        LogFormat::Json => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_span_events(FmtSpan::CLOSE)
            .json()
            .flatten_event(true)
            .try_init()
            .map_err(|error| anyhow::anyhow!("installing JSON tracing subscriber: {error}")),
        LogFormat::Pretty => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_span_events(FmtSpan::CLOSE)
            .compact()
            .try_init()
            .map_err(|error| anyhow::anyhow!("installing pretty tracing subscriber: {error}")),
    }
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

async fn shutdown_signal() -> anyhow::Result<&'static str> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("registering SIGTERM handler")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("listening for Ctrl-C")?;
                Ok("SIGINT")
            }
            _ = terminate.recv() => Ok("SIGTERM"),
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("listening for Ctrl-C")?;
        Ok("Ctrl-C")
    }
}

fn unexpected_server_exit(
    completed: Option<Result<anyhow::Result<&'static str>, JoinError>>,
) -> anyhow::Error {
    match completed {
        Some(Ok(Ok(service))) => anyhow::anyhow!("{service} server stopped unexpectedly"),
        Some(Ok(Err(error))) => error,
        Some(Err(error)) => anyhow::anyhow!("server task failed: {error}"),
        None => anyhow::anyhow!("all server tasks stopped unexpectedly"),
    }
}

/// Pick the object-store backend from config and open it.
///
/// `redb`: opens the file at `--db` > `storage.path`.
///
/// `postgres` (feature-gated): opens a `PgObjectDb` against
/// `--db-url` > `storage.url`. Fails to compile into the call if the
/// `postgres` feature is off — `Config::load` already rejects the
/// configuration before we get here, but the `cfg` keeps the symbol off the
/// dependency graph entirely for slim builds.
fn open_object_db(args: &Args, config: &Config) -> anyhow::Result<Arc<dyn ObjectDb>> {
    match config.storage.backend.as_str() {
        "redb" => {
            let path = args
                .db
                .clone()
                .unwrap_or_else(|| config.storage.path.clone());
            Ok(Arc::new(
                RedbObjectDb::open(&path).context("opening redb object store")?,
            ))
        }
        "postgres" => open_postgres(args, config),
        // `Config::load` validates this, so this is defensive only.
        other => anyhow::bail!("unsupported storage.backend {other:?}"),
    }
}

#[cfg(feature = "postgres")]
fn open_postgres(args: &Args, config: &Config) -> anyhow::Result<Arc<dyn ObjectDb>> {
    use schemahub_jj::PgObjectDb;
    let url = args
        .db_url
        .clone()
        .or_else(|| config.storage.url.clone())
        .ok_or_else(|| anyhow::anyhow!("postgres backend requires --db-url or storage.url"))?;
    let db = PgObjectDb::connect(&url)
        .map_err(|e| anyhow::anyhow!("opening postgres object store: {e}"))?;
    Ok(Arc::new(db))
}

#[cfg(not(feature = "postgres"))]
fn open_postgres(_args: &Args, _config: &Config) -> anyhow::Result<Arc<dyn ObjectDb>> {
    anyhow::bail!(
        "storage.backend = \"postgres\" requires building schemahub-server \
         with `--features postgres`; this binary was built without it"
    )
}

/// Determine the listen address: explicit `--listen` > `TAILSCALE_IP:50051` >
/// config `[listen].addr` > `0.0.0.0:50051`.
fn resolve_listen_addr(explicit: Option<String>, config: &Config) -> anyhow::Result<SocketAddr> {
    if let Some(a) = explicit {
        return a.parse().context("parsing --listen address");
    }
    if let Ok(ip) = std::env::var("TAILSCALE_IP") {
        if !ip.trim().is_empty() {
            let port = config.listen.addr.rsplit(':').next().unwrap_or("50051");
            return format!("{}:{}", ip.trim(), port)
                .parse()
                .context("parsing TAILSCALE_IP listen address");
        }
    }
    config
        .listen
        .addr
        .parse()
        .context("parsing config listen address")
}

fn resolve_http_listen_addr(explicit: Option<String>) -> anyhow::Result<Option<SocketAddr>> {
    let Some(addr) = explicit else {
        return Ok(None);
    };
    if addr.trim().is_empty() {
        return Ok(None);
    }
    addr.parse()
        .map(Some)
        .context("parsing --http-listen address")
}

fn ensure_gui_has_http_listener(
    http_addr: Option<SocketAddr>,
    gui_dir: Option<&Path>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        gui_dir.is_none() || http_addr.is_some(),
        "serving [http].gui_dir or --gui-dir requires --http-listen"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_url_accepts_http_endpoint() {
        // Arrange
        let url = "http://127.0.0.1:8080/readyz";

        // Act
        let parsed = validate_readiness_url(url).expect("HTTP readiness URL should be valid");

        // Assert
        assert_eq!(parsed.path(), "/readyz");
    }

    #[test]
    fn readiness_url_rejects_non_http_scheme() {
        // Arrange
        let url = "file:///tmp/readyz";

        // Act
        let error = validate_readiness_url(url).expect_err("file URL must be rejected");

        // Assert
        assert!(error.to_string().contains("scheme must be http or https"));
    }

    #[test]
    fn readiness_status_rejects_service_unavailable() {
        // Arrange
        let status = reqwest::StatusCode::SERVICE_UNAVAILABLE;

        // Act
        let error = ensure_readiness_status(status).expect_err("503 must fail readiness");

        // Assert
        assert!(error.to_string().contains("HTTP 503 Service Unavailable"));
    }

    #[test]
    fn generated_openapi_is_writable_json() {
        // Arrange
        let mut output = Vec::new();

        // Act
        write_openapi(&mut output).expect("OpenAPI document should serialize");

        // Assert
        let document: serde_json::Value =
            serde_json::from_slice(&output).expect("OpenAPI output should be JSON");
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(document["info"]["version"], BUILD_VERSION);
        assert_eq!(output, http::openapi_json_bytes());
    }

    #[test]
    fn gui_directory_requires_an_http_listener() {
        // Arrange
        let gui_dir = Path::new("/tmp/schemahub-gui");

        // Act
        let error = ensure_gui_has_http_listener(None, Some(gui_dir))
            .expect_err("GUI without HTTP listener must fail");

        // Assert
        assert!(error.to_string().contains("requires --http-listen"));
    }

    #[test]
    fn gui_directory_is_allowed_with_an_http_listener() {
        // Arrange
        let http_addr: SocketAddr = "127.0.0.1:8080".parse().expect("HTTP address");
        let gui_dir = Path::new("/tmp/schemahub-gui");

        // Act
        let result = ensure_gui_has_http_listener(Some(http_addr), Some(gui_dir));

        // Assert
        assert!(result.is_ok());
    }
}
