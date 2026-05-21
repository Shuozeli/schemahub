use clap::{Args, Subcommand};
use tonic::transport::Channel;

#[derive(Args)]
pub struct CodegenArgs {
    #[command(subcommand)]
    pub action: CodegenAction,
}

#[derive(Subcommand)]
pub enum CodegenAction {
    /// Download generated descriptors and write to a directory
    Get {
        schema_path: String,
        #[arg(long)]
        lang: String,
        #[arg(long, default_value = "./gen")]
        out: String,
    },
    /// Preview generated code without writing files
    Preview {
        schema_path: String,
        #[arg(long)]
        lang: String,
    },
}

pub async fn run(args: CodegenArgs, _channel: Channel) -> anyhow::Result<()> {
    match args.action {
        CodegenAction::Get { schema_path, lang, out } => {
            println!("TODO: codegen get {schema_path} --lang {lang} --out {out}");
        }
        CodegenAction::Preview { schema_path, lang } => {
            println!("TODO: codegen preview {schema_path} --lang {lang}");
        }
    }
    Ok(())
}
