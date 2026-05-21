use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    schema_service_client::SchemaServiceClient, CreateSchemaRequest, SchemaFormat,
};
use std::path::PathBuf;
use tonic::transport::Channel;
use ::uuid::Uuid;

#[derive(Args)]
pub struct SchemaArgs {
    #[command(subcommand)]
    pub action: SchemaAction,
}

#[derive(Subcommand)]
pub enum SchemaAction {
    /// Create a new schema from a local file
    Create {
        /// Path to the local schema file (e.g. user.proto)
        file: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        repo: String,
        #[arg(long, default_value = "main")]
        branch: String,
        /// Override the schema name (default: file basename)
        #[arg(long)]
        name: Option<String>,
        /// Current branch HEAD commit (for optimistic concurrency)
        #[arg(long, default_value = "")]
        base_revision: String,
    },
    /// Update an existing schema from a local file
    Update {
        file: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        repo: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "")]
        base_revision: String,
        #[arg(long)]
        force: bool,
    },
    /// Pull (print) a schema from the registry
    Pull {
        /// project/repo/schema_name
        schema_path: String,
        #[arg(long, default_value = "main")]
        branch: String,
    },
    /// Delete a schema from a branch
    Delete {
        schema_path: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long, default_value = "")]
        base_revision: String,
        #[arg(long)]
        force: bool,
    },
}

fn detect_format(path: &std::path::Path) -> anyhow::Result<i32> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("proto") => Ok(SchemaFormat::Protobuf as i32),
        Some("fbs") => Ok(SchemaFormat::Flatbuffers as i32),
        Some("yaml") | Some("yml") | Some("json") => Ok(SchemaFormat::Openapi as i32),
        _ => bail!(
            "Cannot detect format from extension for {:?}. Use a .proto, .fbs, .yaml, or .json file.",
            path
        ),
    }
}

pub async fn run(args: SchemaArgs, channel: Channel) -> anyhow::Result<()> {
    match args.action {
        SchemaAction::Create {
            file,
            project,
            repo,
            branch,
            name,
            base_revision,
        } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let format_i32 = detect_format(&file)?;
            let schema_name = name
                .or_else(|| file.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                .context("cannot determine schema name")?;

            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .create_schema(CreateSchemaRequest {
                    project,
                    repo,
                    branch,
                    schema_name,
                    format: format_i32,
                    source,
                    base_revision,
                    idempotency_key: Uuid::new_v4().to_string(),
                })
                .await
                .context("CreateSchema RPC")?;

            println!("Created commit: {}", resp.into_inner().new_commit);
        }
        SchemaAction::Update {
            file,
            project,
            repo,
            branch,
            name,
            base_revision,
            force,
        } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let schema_name = name
                .or_else(|| file.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                .context("cannot determine schema name")?;

            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .update_schema(schemahub_api::schemahub_v1::UpdateSchemaRequest {
                    project,
                    repo,
                    branch,
                    schema_name,
                    source,
                    base_revision,
                    idempotency_key: Uuid::new_v4().to_string(),
                    force,
                })
                .await
                .context("UpdateSchema RPC")?;

            println!("Updated commit: {}", resp.into_inner().new_commit);
        }
        SchemaAction::Pull { schema_path, branch } => {
            println!("TODO: pull {schema_path} from branch {branch}");
        }
        SchemaAction::Delete {
            schema_path,
            branch,
            base_revision,
            force,
        } => {
            let parts = parse_schema_path_3(&schema_path)?;
            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .delete_schema(schemahub_api::schemahub_v1::DeleteSchemaRequest {
                    project: parts.0,
                    repo: parts.1,
                    schema_name: parts.2,
                    branch,
                    base_revision,
                    idempotency_key: Uuid::new_v4().to_string(),
                    force,
                })
                .await
                .context("DeleteSchema RPC")?;
            println!("Deleted. New commit: {}", resp.into_inner().new_commit);
        }
    }
    Ok(())
}

/// Parse "project/repo/schema_name" into three parts.
pub fn parse_schema_path_3(s: &str) -> anyhow::Result<(String, String, String)> {
    let parts: Vec<&str> = s.splitn(3, '/').collect();
    if parts.len() != 3 {
        bail!("schema_path must be 'project/repo/schema_name', got: {s}");
    }
    Ok((parts[0].to_string(), parts[1].to_string(), parts[2].to_string()))
}


