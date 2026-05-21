use clap::{Args, Subcommand};
use tonic::transport::Channel;

#[derive(Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub action: TagAction,
}

#[derive(Subcommand)]
pub enum TagAction {
    /// Create a tag
    Create {
        /// project/repo
        repo: String,
        name: String,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        message: Option<String>,
    },
    /// List tags
    List { repo: String },
}

pub async fn run(args: TagArgs, _channel: Channel) -> anyhow::Result<()> {
    match args.action {
        TagAction::Create { repo, name, commit, message: _ } => {
            println!("TODO: create tag '{name}' on {repo} at {:?}", commit);
        }
        TagAction::List { repo } => {
            println!("TODO: list tags for {repo}");
        }
    }
    Ok(())
}
