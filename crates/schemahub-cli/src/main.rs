mod client;
mod cmd;
mod config;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use cmd::{branch, codegen, field, log, schema, tag};

#[derive(Parser)]
#[command(name = "schemahub", about = "schemahub schema registry CLI", version)]
struct Cli {
    /// Server address (overrides config and SCHEMAHUB_SERVER)
    #[arg(long, env = "SCHEMAHUB_SERVER", global = true)]
    server: Option<String>,

    /// Auth token (overrides config and SCHEMAHUB_TOKEN)
    #[arg(long, env = "SCHEMAHUB_TOKEN", global = true)]
    token: Option<String>,

    /// Config profile to use
    #[arg(long, default_value = "default", global = true)]
    profile: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Schema file lifecycle operations
    Schema(schema::SchemaArgs),
    /// Field mutations on Protobuf schemas
    Field(field::FieldArgs),
    /// Branch management
    Branch(branch::BranchArgs),
    /// Tag management
    Tag(tag::TagArgs),
    /// Show commit history for a repo
    Log(log::LogArgs),
    /// Code generation
    Codegen(codegen::CodegenArgs),
    /// Print diff between two refs
    Diff {
        /// project/repo
        repo: String,
        /// base..head (e.g. "main..feature/xyz" or two branch names separated by "..")
        range: String,
        /// Optional: restrict diff to one schema file
        #[arg(long, default_value = "")]
        schema_path: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let cfg = config::Config::load(
        &cli.profile,
        cli.server.as_deref(),
        cli.token.as_deref(),
    )
    .context("loading config")?;

    match cli.command {
        Commands::Schema(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            schema::run(args, ch).await
        }
        Commands::Field(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            field::run(args, ch).await
        }
        Commands::Branch(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            branch::run(args, ch).await
        }
        Commands::Tag(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            tag::run(args, ch).await
        }
        Commands::Log(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            log::run(args, ch).await
        }
        Commands::Codegen(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            codegen::run(args, ch).await
        }
        Commands::Diff { repo, range, schema_path } => {
            let parts: Vec<&str> = repo.splitn(2, '/').collect();
            if parts.len() != 2 {
                bail!("repo must be 'project/repo'");
            }
            let (project, repo_name) = (parts[0].to_string(), parts[1].to_string());

            // Parse range as "base..head"
            let range_parts: Vec<&str> = range.splitn(2, "..").collect();
            if range_parts.len() != 2 {
                bail!("range must be 'base..head'");
            }
            let (base_ref, head_ref) = (range_parts[0].to_string(), range_parts[1].to_string());

            use schemahub_api::schemahub_v1::{
                ref_service_client::RefServiceClient, DiffRequest, VersionRef,
                version_ref::Ref as VersionRefKind,
            };
            let ch = client::build_channel(&cfg.server).await?;
            let mut client = RefServiceClient::new(ch);
            let resp = client
                .diff(DiffRequest {
                    project,
                    repo: repo_name,
                    base: Some(VersionRef { r#ref: Some(VersionRefKind::Branch(base_ref)) }),
                    head: Some(VersionRef { r#ref: Some(VersionRefKind::Branch(head_ref)) }),
                    schema_path,
                })
                .await
                .context("Diff RPC")?;

            let diffs = resp.into_inner().schema_diffs;
            if diffs.is_empty() {
                println!("(no changes)");
            }
            for schema_diff in diffs {
                println!("schema: {}", schema_diff.schema_path);
                for change in schema_diff.changes {
                    println!("  {} {}", change.change_type, change.decl_name);
                }
            }
            Ok(())
        }
    }
}
