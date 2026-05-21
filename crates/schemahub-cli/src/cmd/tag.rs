use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    ref_service_client::RefServiceClient, CreateTagRequest, DeleteTagRequest, ListTagsRequest,
    VersionRef,
};
use tonic::transport::Channel;

#[derive(Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub action: TagAction,
}

#[derive(Subcommand)]
pub enum TagAction {
    /// Create a tag at a commit or branch HEAD
    Create {
        /// project/repo
        repo: String,
        name: String,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long, default_value = "")]
        message: String,
    },
    /// Delete a tag
    Delete {
        repo: String,
        name: String,
        #[arg(long)]
        force: bool,
    },
    /// List tags
    List {
        repo: String,
        #[arg(long, default_value = "")]
        prefix: String,
    },
}

fn parse_repo(s: &str) -> anyhow::Result<(String, String)> {
    let parts: Vec<&str> = s.splitn(2, '/').collect();
    if parts.len() != 2 {
        bail!("repo must be 'project/repo', got: {s}");
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

pub async fn run(args: TagArgs, channel: Channel) -> anyhow::Result<()> {
    match args.action {
        TagAction::Create { repo, name, commit, branch, message: _ } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let target = Some(if let Some(commit_hex) = commit {
                VersionRef {
                    r#ref: Some(schemahub_api::schemahub_v1::version_ref::Ref::Commit(commit_hex)),
                }
            } else {
                VersionRef {
                    r#ref: Some(schemahub_api::schemahub_v1::version_ref::Ref::Branch(branch)),
                }
            });

            let mut client = RefServiceClient::new(channel);
            let resp = client
                .create_tag(CreateTagRequest {
                    project,
                    repo: repo_name,
                    name: name.clone(),
                    target,
                    message: String::new(),
                })
                .await
                .context("CreateTag RPC")?;

            let tag = resp.into_inner().tag.unwrap_or_default();
            println!("Created tag '{}' at {}", tag.name, tag.commit_hash);
        }
        TagAction::Delete { repo, name, force } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            client
                .delete_tag(DeleteTagRequest {
                    project,
                    repo: repo_name,
                    name: name.clone(),
                    force,
                })
                .await
                .context("DeleteTag RPC")?;
            println!("Deleted tag '{name}'");
        }
        TagAction::List { repo, prefix } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            let resp = client
                .list_tags(ListTagsRequest {
                    project,
                    repo: repo_name,
                    name_prefix: prefix,
                })
                .await
                .context("ListTags RPC")?;

            let tags = resp.into_inner().tags;
            if tags.is_empty() {
                println!("(no tags)");
            }
            for tag in tags {
                println!("  {} → {}", tag.name, tag.commit_hash);
            }
        }
    }
    Ok(())
}
