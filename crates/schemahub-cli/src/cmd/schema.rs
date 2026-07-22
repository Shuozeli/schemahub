use ::uuid::Uuid;
use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    exploration_service_client::ExplorationServiceClient,
    schema_service_client::SchemaServiceClient, CreateSchemaRequest, GetSchemaSourceRequest,
    ListDependentsRequest, ListDependentsResponse, SchemaFormat, VersionRef,
};
use std::path::PathBuf;
use tonic::transport::Channel;

use crate::cmd::bearer;

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
    /// Find direct downstream imports across repositories visible to the caller
    Dependents {
        /// project/repo/schema_name
        schema_path: String,
        /// Emit stable machine-readable output for agents and automation
        #[arg(long)]
        json: bool,
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

pub async fn run(args: SchemaArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
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
                .or_else(|| {
                    file.file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                })
                .context("cannot determine schema name")?;

            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .create_schema(bearer(
                    CreateSchemaRequest {
                        project,
                        repo,
                        branch,
                        schema_name,
                        format: format_i32,
                        source,
                        base_revision,
                        idempotency_key: Uuid::new_v4().to_string(),
                    },
                    token,
                )?)
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
                .or_else(|| {
                    file.file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                })
                .context("cannot determine schema name")?;

            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .update_schema(bearer(
                    schemahub_api::schemahub_v1::UpdateSchemaRequest {
                        project,
                        repo,
                        branch,
                        schema_name,
                        source,
                        base_revision,
                        idempotency_key: Uuid::new_v4().to_string(),
                        force,
                    },
                    token,
                )?)
                .await
                .context("UpdateSchema RPC")?;

            println!("Updated commit: {}", resp.into_inner().new_commit);
        }
        SchemaAction::Pull {
            schema_path,
            branch,
        } => {
            // schema_path is "project/repo/schema_name"
            let parts = parse_schema_path_3(&schema_path)?;
            let mut client = ExplorationServiceClient::new(channel);
            let resp = client
                .get_schema_source(bearer(
                    GetSchemaSourceRequest {
                        project: parts.0,
                        repo: parts.1,
                        schema_path: parts.2,
                        at: Some(VersionRef {
                            r#ref: Some(super::parse_ref(&branch)),
                        }),
                    },
                    token,
                )?)
                .await
                .context("GetSchemaSource RPC")?;

            let source = String::from_utf8(resp.into_inner().source.to_vec())
                .context("schema source is not valid UTF-8")?;
            print!("{source}");
        }
        SchemaAction::Dependents { schema_path, json } => {
            let (project, repo, schema_name) = parse_schema_path_3(&schema_path)?;
            let mut client = ExplorationServiceClient::new(channel);
            let response = client
                .list_dependents(bearer(
                    ListDependentsRequest {
                        project,
                        repo,
                        schema_path: schema_name,
                    },
                    token,
                )?)
                .await
                .context("ListDependents RPC")?
                .into_inner();
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&dependents_json(&schema_path, &response))?
                );
            } else if response.dependents.is_empty() {
                println!(
                    "No visible direct dependents ({} schemas across {} snapshots scanned).",
                    response.schemas_scanned,
                    response.snapshots.len()
                );
            } else {
                for dependent in response.dependents {
                    let pin = if dependent.pinned {
                        format!("pinned {}", dependent.resolved_commit)
                    } else {
                        "live/unpinned".to_string()
                    };
                    println!(
                        "{}/{}/{} @{} ({}, snapshot {})",
                        dependent.importing_project,
                        dependent.importing_repo,
                        dependent.importing_schema,
                        dependent.importing_bookmark,
                        pin,
                        dependent.importing_commit,
                    );
                }
            }
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
                .delete_schema(bearer(
                    schemahub_api::schemahub_v1::DeleteSchemaRequest {
                        project: parts.0,
                        repo: parts.1,
                        schema_name: parts.2,
                        branch,
                        base_revision,
                        idempotency_key: Uuid::new_v4().to_string(),
                        force,
                    },
                    token,
                )?)
                .await
                .context("DeleteSchema RPC")?;
            println!("Deleted. New commit: {}", resp.into_inner().new_commit);
        }
    }
    Ok(())
}

fn dependents_json(target: &str, response: &ListDependentsResponse) -> serde_json::Value {
    let dependents: Vec<_> = response
        .dependents
        .iter()
        .map(|dependent| {
            serde_json::json!({
                "importingProject": dependent.importing_project,
                "importingRepo": dependent.importing_repo,
                "importingSchema": dependent.importing_schema,
                "importingDecl": dependent.importing_decl,
                "importingBookmark": dependent.importing_bookmark,
                "importingCommit": dependent.importing_commit,
                "importPath": dependent.import_path,
                "importedDecl": dependent.imported_decl,
                "resolvedCommit": dependent.resolved_commit,
                "pinned": dependent.pinned,
            })
        })
        .collect();
    let snapshots: Vec<_> = response
        .snapshots
        .iter()
        .map(|snapshot| {
            serde_json::json!({
                "project": snapshot.project,
                "repo": snapshot.repo,
                "bookmark": snapshot.bookmark,
                "commitId": snapshot.commit_id,
            })
        })
        .collect();
    serde_json::json!({
        "target": target,
        "schemasScanned": response.schemas_scanned,
        "snapshots": snapshots,
        "dependents": dependents,
    })
}

/// Parse "project/repo/schema_name" into three parts.
pub fn parse_schema_path_3(s: &str) -> anyhow::Result<(String, String, String)> {
    let parts: Vec<&str> = s.splitn(3, '/').collect();
    if parts.len() != 3 {
        bail!("schema_path must be 'project/repo/schema_name', got: {s}");
    }
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependent_json_preserves_snapshot_and_pin_metadata() {
        // Arrange
        let response = ListDependentsResponse {
            dependents: vec![schemahub_api::schemahub_v1::DependentEntry {
                importing_project: "billing".to_string(),
                importing_repo: "consumer".to_string(),
                importing_schema: "invoice.proto".to_string(),
                importing_decl: String::new(),
                importing_bookmark: "main".to_string(),
                importing_commit: "consumer-commit".to_string(),
                import_path: "acme/provider/types.proto".to_string(),
                imported_decl: "Shared".to_string(),
                resolved_commit: "provider-commit".to_string(),
                pinned: true,
            }],
            snapshots: vec![schemahub_api::schemahub_v1::DependencyScanSnapshot {
                project: "billing".to_string(),
                repo: "consumer".to_string(),
                bookmark: "main".to_string(),
                commit_id: "consumer-commit".to_string(),
            }],
            schemas_scanned: 1,
        };

        // Act
        let output = dependents_json("acme/provider/types.proto", &response);

        // Assert
        assert_eq!(output["target"], "acme/provider/types.proto");
        assert_eq!(output["schemasScanned"], 1);
        assert_eq!(output["dependents"][0]["pinned"], true);
        assert_eq!(output["dependents"][0]["resolvedCommit"], "provider-commit");
        assert_eq!(output["snapshots"][0]["commitId"], "consumer-commit");
    }
}
