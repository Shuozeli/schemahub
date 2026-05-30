use anyhow::Context;
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    project_service_client::ProjectServiceClient,
    CreateProjectRequest, CreateRepoRequest,
};
use tonic::transport::Channel;

use crate::cmd::bearer;

#[derive(Args)]
pub struct RepoArgs {
    #[command(subcommand)]
    pub action: RepoAction,
}

#[derive(Subcommand)]
pub enum RepoAction {
    /// Initialize a new project and repo (creates both if they don't already exist)
    Init {
        /// project/repo  e.g. acme/platform
        repo: String,
        /// Mark the project as publicly readable
        #[arg(long)]
        public: bool,
        /// Default branch name
        #[arg(long, default_value = "main")]
        default_branch: String,
    },
}

pub async fn run(args: RepoArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    match args.action {
        RepoAction::Init { repo, public, default_branch } => {
            let parts: Vec<&str> = repo.splitn(2, '/').collect();
            if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                anyhow::bail!("repo must be 'project/repo', e.g. acme/platform");
            }
            let project = parts[0].to_string();
            let repo_name = parts[1].to_string();

            let mut client = ProjectServiceClient::new(channel);

            // Create the project — ignore AlreadyExists so `repo init` is idempotent.
            let proj_result = client
                .create_project(bearer(
                    CreateProjectRequest {
                        name: project.clone(),
                        is_public: public,
                    },
                    token,
                )?)
                .await;

            match proj_result {
                Ok(_) => println!("Created project '{project}'."),
                Err(status) if status.code() == tonic::Code::AlreadyExists => {
                    println!("Project '{project}' already exists, skipping.");
                }
                Err(e) => return Err(e).context("CreateProject RPC"),
            }

            // Create the repo.
            client
                .create_repo(bearer(
                    CreateRepoRequest {
                        project: project.clone(),
                        name: repo_name.clone(),
                        default_branch,
                        compatibility_direction: 0,
                        protected_branches: vec![],
                    },
                    token,
                )?)
                .await
                .context("CreateRepo RPC")?;

            println!("Created repo '{project}/{repo_name}'.");
            println!("Ready. Use `schemahub schema create --project {project} --repo {repo_name} <file>` to add schemas.");
        }
    }
    Ok(())
}
