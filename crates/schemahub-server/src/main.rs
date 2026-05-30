//! `schemahub-server` — the gRPC server and composition root
//! (crate-structure.md §3.6).
//!
//! Startup: load config (`schemahub.toml`, optional), open the configured
//! object store (redb embedded default, or Postgres behind the `postgres`
//! feature), build the `Core` over the three compilers, register every gRPC
//! service, and serve. Binds to `TAILSCALE_IP` (user infra convention) when set
//! and no explicit `--listen` is given, else `0.0.0.0`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use schemahub_vcs::{ObjectDb, RedbObjectDb};

use schemahub_server::{build_core, build_router, config::Config};

#[derive(Parser, Debug)]
#[command(name = "schemahub-server", about = "schemahub gRPC server", version)]
struct Args {
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
    /// Path to schemahub.toml (optional).
    #[arg(long, default_value = "schemahub.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;

    let db = open_object_db(&args, &config)?;

    let core = build_core(db, &config);

    let addr = resolve_listen_addr(args.listen, &config)?;

    // Surface a MagicDNS-friendly hint when bound to the Tailscale IP.
    println!("schemahub-server listening on {addr}");

    build_router(core, config.storage.backend.clone())
        .serve(addr)
        .await
        .context("serving gRPC")?;
    Ok(())
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
    use schemahub_vcs::PgObjectDb;
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
            let port = config
                .listen
                .addr
                .rsplit(':')
                .next()
                .unwrap_or("50051");
            return format!("{}:{}", ip.trim(), port)
                .parse()
                .context("parsing TAILSCALE_IP listen address");
        }
    }
    config.listen.addr.parse().context("parsing config listen address")
}
