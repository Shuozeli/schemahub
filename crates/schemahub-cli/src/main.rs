mod client;
mod cmd;
mod config;

use anyhow::Context;
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
    /// Print diff between two refs (not yet implemented)
    Diff {
        repo: String,
        range: String,
    },
    /// Fast-forward merge a branch (not yet implemented)
    Merge {
        source: String,
        #[arg(long)]
        into: String,
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
        Commands::Diff { repo, range } => {
            println!("TODO: diff {repo} {range}");
            Ok(())
        }
        Commands::Merge { source, into } => {
            println!("TODO: merge {source} --into {into}");
            Ok(())
        }
    }
}
