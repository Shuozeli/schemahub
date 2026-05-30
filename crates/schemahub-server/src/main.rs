//! `schemahub-server` — the gRPC server and composition root
//! (crate-structure.md §3.6).
//!
//! Startup: load config (`schemahub.toml`, optional), open the redb object
//! store, build the `Core` over the three compilers, register every gRPC
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
    /// Path to the redb database file. Overrides config.
    #[arg(long)]
    db: Option<String>,
    /// Path to schemahub.toml (optional).
    #[arg(long, default_value = "schemahub.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;

    let db_path = args.db.unwrap_or_else(|| config.storage.path.clone());
    let db: Arc<dyn ObjectDb> =
        Arc::new(RedbObjectDb::open(&db_path).context("opening redb object store")?);

    let core = build_core(db, &config);

    let addr = resolve_listen_addr(args.listen, &config)?;

    // Surface a MagicDNS-friendly hint when bound to the Tailscale IP.
    println!("schemahub-server listening on {addr}");

    build_router(core)
        .serve(addr)
        .await
        .context("serving gRPC")?;
    Ok(())
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
