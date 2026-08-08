use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    ref_service_client::RefServiceClient, CreateTagRequest, DeleteTagRequest, ListTagsRequest,
    VersionRef,
};
use tonic::transport::Channel;

use crate::cmd::bearer;

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
        /// Number of tags fetched per RPC.
        #[arg(long, default_value_t = 50)]
        page_size: i32,
    },
}

fn parse_repo(s: &str) -> anyhow::Result<(String, String)> {
    let parts: Vec<&str> = s.splitn(2, '/').collect();
    if parts.len() != 2 {
        bail!("repo must be 'project/repo', got: {s}");
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

pub async fn run(args: TagArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    match args.action {
        TagAction::Create {
            repo,
            name,
            commit,
            branch,
            message: _,
        } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let target = Some(if let Some(commit_hex) = commit {
                VersionRef {
                    r#ref: Some(schemahub_api::schemahub_v1::version_ref::Ref::Commit(
                        commit_hex,
                    )),
                }
            } else {
                VersionRef {
                    r#ref: Some(schemahub_api::schemahub_v1::version_ref::Ref::Branch(
                        branch,
                    )),
                }
            });

            let mut client = RefServiceClient::new(channel);
            let resp = client
                .create_tag(bearer(
                    CreateTagRequest {
                        project,
                        repo: repo_name,
                        name: name.clone(),
                        target,
                        message: String::new(),
                    },
                    token,
                )?)
                .await
                .context("CreateTag RPC")?;

            let tag = resp.into_inner().tag.unwrap_or_default();
            println!("Created tag '{}' at {}", tag.name, tag.commit_hash);
        }
        TagAction::Delete { repo, name, force } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            client
                .delete_tag(bearer(
                    DeleteTagRequest {
                        project,
                        repo: repo_name,
                        name: name.clone(),
                        force,
                    },
                    token,
                )?)
                .await
                .context("DeleteTag RPC")?;
            println!("Deleted tag '{name}'");
        }
        TagAction::List {
            repo,
            prefix,
            page_size,
        } => {
            let (project, repo_name) = parse_repo(&repo)?;
            let mut client = RefServiceClient::new(channel);
            let mut page_token = String::new();
            let mut found = false;
            loop {
                let response = client
                    .list_tags(bearer(
                        ListTagsRequest {
                            project: project.clone(),
                            repo: repo_name.clone(),
                            name_prefix: prefix.clone(),
                            page_size,
                            page_token,
                        },
                        token,
                    )?)
                    .await
                    .context("ListTags RPC")?
                    .into_inner();
                for tag in response.tags {
                    found = true;
                    println!("  {} → {}", tag.name, tag.commit_hash);
                }
                if response.next_page_token.is_empty() {
                    break;
                }
                page_token = response.next_page_token;
            }
            if !found {
                println!("(no tags)");
            }
        }
    }
    Ok(())
}
