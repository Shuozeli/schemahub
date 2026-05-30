//! jj-style history & recovery commands (design.md §12): `op log`, `undo`,
//! `resolve`. All are pure gRPC clients over `HistoryService`.

use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    history_service_client::HistoryServiceClient, OpLogRequest, RenderConflictRequest,
    ResolveConflictRequest, UndoRequest, VersionRef,
};
use std::path::PathBuf;
use tonic::transport::Channel;

use super::schema::parse_schema_path_3;

fn parse_repo(s: &str) -> anyhow::Result<(String, String)> {
    let parts: Vec<&str> = s.splitn(2, '/').collect();
    if parts.len() != 2 {
        bail!("repo must be 'project/repo', got: {s}");
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

// ── op log ──────────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct OpArgs {
    #[command(subcommand)]
    pub action: OpAction,
}

#[derive(Subcommand)]
pub enum OpAction {
    /// Show the operation log (the audit record) for a repo
    Log {
        /// project/repo
        repo: String,
    },
}

pub async fn run_op(args: OpArgs, channel: Channel) -> anyhow::Result<()> {
    match args.action {
        OpAction::Log { repo } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = HistoryServiceClient::new(channel);
            let resp = client
                .op_log(OpLogRequest {
                    project,
                    repo: repo_name,
                })
                .await
                .context("OpLog RPC")?;
            let ops = resp.into_inner().operations;
            if ops.is_empty() {
                println!("(no operations)");
            }
            for op in ops {
                println!("op {}", op.op_id);
                println!("Author: {}", op.author);
                println!("Date:   {}", op.timestamp);
                println!("    {}", op.description);
                println!();
            }
            Ok(())
        }
    }
}

// ── undo ──────────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct UndoArgs {
    /// project/repo
    pub repo: String,
    /// Author recorded for the undo operation
    #[arg(long, default_value = "schemahub-cli")]
    pub author: String,
}

pub async fn run_undo(args: UndoArgs, channel: Channel) -> anyhow::Result<()> {
    let (project, repo) = parse_repo(&args.repo)?;
    let mut client = HistoryServiceClient::new(channel);
    let resp = client
        .undo(UndoRequest {
            project,
            repo,
            author: args.author,
        })
        .await
        .context("Undo RPC")?;
    println!("Undid operation {}", resp.into_inner().undone_op_id);
    Ok(())
}

// ── resolve ──────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct ResolveArgs {
    /// project/repo/schema_name
    pub schema_path: String,
    /// The conflicted declaration name (e.g. UserRequest)
    pub declaration: String,
    /// Bookmark/branch carrying the conflict
    #[arg(long, default_value = "main")]
    pub branch: String,
    /// File containing the resolved schema source (defines the resolved decl).
    /// Omit to only render the conflict's competing sides.
    #[arg(long)]
    pub from: Option<PathBuf>,
    #[arg(long, default_value = "schemahub-cli")]
    pub author: String,
    #[arg(long, default_value = "")]
    pub message: String,
}

pub async fn run_resolve(args: ResolveArgs, channel: Channel) -> anyhow::Result<()> {
    let (project, repo, schema_name) = parse_schema_path_3(&args.schema_path)?;
    let mut client = HistoryServiceClient::new(channel);

    match args.from {
        None => {
            // Render the conflict so the user can craft a resolution.
            let resp = client
                .render_conflict(RenderConflictRequest {
                    project,
                    repo,
                    schema_path: schema_name,
                    declaration_name: args.declaration,
                    at: Some(VersionRef {
                        r#ref: Some(super::parse_ref(&args.branch)),
                    }),
                })
                .await
                .context("RenderConflict RPC")?;
            println!("{}", resp.into_inner().rendered);
            Ok(())
        }
        Some(path) => {
            let resolved_source = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let resp = client
                .resolve_conflict(ResolveConflictRequest {
                    project,
                    repo,
                    bookmark: args.branch,
                    schema_path: schema_name,
                    declaration_name: args.declaration,
                    resolved_source,
                    author: args.author,
                    message: args.message,
                })
                .await
                .context("ResolveConflict RPC")?;
            println!("Resolved. New commit: {}", resp.into_inner().new_commit);
            Ok(())
        }
    }
}
