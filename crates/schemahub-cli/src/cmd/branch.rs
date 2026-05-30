use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    ref_service_client::RefServiceClient, CreateBranchRequest, DeleteBranchRequest,
    ListBranchesRequest, MergeRequest, VersionRef,
};
use tonic::transport::Channel;
use uuid::Uuid;

use crate::cmd::bearer;

#[derive(Args)]
pub struct BranchArgs {
    #[command(subcommand)]
    pub action: BranchAction,
}

#[derive(Subcommand)]
pub enum BranchAction {
    /// Create a new branch
    Create {
        /// project/repo
        repo: String,
        name: String,
        #[arg(long, default_value = "main")]
        from: String,
    },
    /// Delete a branch
    Delete { repo: String, name: String },
    /// List branches
    List {
        repo: String,
        #[arg(long, default_value = "")]
        prefix: String,
    },
    /// Fast-forward merge a branch into a target branch
    Merge {
        /// project/repo
        repo: String,
        /// Source branch to merge from
        source: String,
        /// Target branch to merge into
        #[arg(long, default_value = "main")]
        into: String,
        /// Expected current HEAD of the target branch (for optimistic concurrency).
        /// If omitted the server uses whatever HEAD is current.
        #[arg(long, default_value = "")]
        base_revision: String,
        /// Optional commit message recorded on the merge commit
        #[arg(long, default_value = "")]
        message: String,
    },
}

pub async fn run(args: BranchArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    match args.action {
        BranchAction::Create { repo, name, from } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            let resp = client
                .create_branch(bearer(
                    CreateBranchRequest {
                        project,
                        repo: repo_name,
                        name,
                        from: Some(VersionRef {
                            r#ref: Some(schemahub_api::schemahub_v1::version_ref::Ref::Branch(
                                from,
                            )),
                        }),
                    },
                    token,
                )?)
                .await
                .context("CreateBranch RPC")?;

            let branch = resp.into_inner().branch.unwrap_or_default();
            println!("Created branch '{}' at {}", branch.name, branch.head_commit);
        }
        BranchAction::Delete { repo, name } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            client
                .delete_branch(bearer(
                    DeleteBranchRequest {
                        project,
                        repo: repo_name,
                        name: name.clone(),
                    },
                    token,
                )?)
                .await
                .context("DeleteBranch RPC")?;
            println!("Deleted branch '{name}'");
        }
        BranchAction::List { repo, prefix } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            let resp = client
                .list_branches(bearer(
                    ListBranchesRequest {
                        project,
                        repo: repo_name,
                        name_prefix: prefix,
                    },
                    token,
                )?)
                .await
                .context("ListBranches RPC")?;

            for branch in resp.into_inner().branches {
                let protected = if branch.protected { " [protected]" } else { "" };
                println!("  {}{} → {}", branch.name, protected, branch.head_commit);
            }
        }
        BranchAction::Merge {
            repo,
            source,
            into,
            base_revision,
            message,
        } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            let resp = client
                .merge(bearer(
                    MergeRequest {
                        project,
                        repo: repo_name,
                        source_branch: source.clone(),
                        target_branch: into.clone(),
                        base_revision,
                        idempotency_key: Uuid::new_v4().to_string(),
                        message,
                    },
                    token,
                )?)
                .await
                .context("Merge RPC")?;

            println!(
                "Merged '{}' into '{}': {}",
                source,
                into,
                resp.into_inner().new_commit,
            );
        }
    }
    Ok(())
}

fn parse_repo(s: &str) -> anyhow::Result<(String, String)> {
    let parts: Vec<&str> = s.splitn(2, '/').collect();
    if parts.len() != 2 {
        bail!("repo must be 'project/repo', got: {s}");
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}
