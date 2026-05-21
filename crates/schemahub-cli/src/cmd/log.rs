use anyhow::{bail, Context};
use clap::Args;
use schemahub_api::schemahub_v1::{ref_service_client::RefServiceClient, ListCommitsRequest};
use tonic::transport::Channel;

#[derive(Args)]
pub struct LogArgs {
    /// project/repo
    pub repo: String,
    #[arg(long, default_value = "main")]
    pub branch: String,
    /// Max number of commits to show
    #[arg(long, default_value = "20")]
    pub limit: usize,
}

pub async fn run(args: LogArgs, channel: Channel) -> anyhow::Result<()> {
    let parts: Vec<&str> = args.repo.splitn(2, '/').collect();
    if parts.len() != 2 {
        bail!("repo must be 'project/repo'");
    }
    let (project, repo) = (parts[0].to_string(), parts[1].to_string());

    let mut client = RefServiceClient::new(channel);
    let mut stream = client
        .list_commits(ListCommitsRequest {
            project,
            repo,
            from: Some(schemahub_api::schemahub_v1::VersionRef {
                r#ref: Some(
                    schemahub_api::schemahub_v1::version_ref::Ref::Branch(args.branch),
                ),
            }),
            stop_at_commit: String::new(),
            schema_path: String::new(),
        })
        .await
        .context("ListCommits RPC")?
        .into_inner();

    let mut count = 0;
    loop {
        match stream.message().await.context("reading commit stream")? {
            None => break,
            Some(commit) => {
                println!("commit {}", commit.hash);
                println!("Author: {}", commit.author);
                println!("    {}", commit.message);
                println!();
                count += 1;
                if count >= args.limit {
                    break;
                }
            }
        }
    }

    if count == 0 {
        println!("(no commits)");
    }

    Ok(())
}
