use anyhow::Context;
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    codegen_service_client::CodegenServiceClient, GetDescriptorsRequest, VersionRef,
};
use tonic::transport::Channel;

use super::schema::parse_schema_path_3;

#[derive(Args)]
pub struct CodegenArgs {
    #[command(subcommand)]
    pub action: CodegenAction,
}

#[derive(Subcommand)]
pub enum CodegenAction {
    /// Download the reconstructed schema descriptor (source) and print to stdout
    Get {
        /// project/repo/schema_name
        schema_path: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long, default_value = "")]
        lang: String,
        #[arg(long, default_value = "./gen")]
        out: String,
    },
    /// Preview reconstructed schema source (prints to stdout)
    Preview {
        /// project/repo/schema_name
        schema_path: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long, default_value = "")]
        lang: String,
    },
}

pub async fn run(args: CodegenArgs, channel: Channel) -> anyhow::Result<()> {
    match args.action {
        CodegenAction::Get { schema_path, branch, lang: _, out: _ } => {
            let parts = parse_schema_path_3(&schema_path)?;
            let mut client = CodegenServiceClient::new(channel);
            let resp = client
                .get_descriptors(GetDescriptorsRequest {
                    project: parts.0,
                    repo: parts.1,
                    schema_path: parts.2,
                    at: Some(VersionRef {
                        r#ref: Some(schemahub_api::schemahub_v1::version_ref::Ref::Branch(branch)),
                    }),
                })
                .await
                .context("GetDescriptors RPC")?;

            let inner = resp.into_inner();
            let source = String::from_utf8(inner.descriptor_bytes.to_vec())
                .unwrap_or_else(|_| "(binary descriptor — not printable)".to_string());
            print!("{source}");
        }
        CodegenAction::Preview { schema_path, branch, lang: _ } => {
            let parts = parse_schema_path_3(&schema_path)?;
            let mut client = CodegenServiceClient::new(channel);
            let resp = client
                .get_descriptors(GetDescriptorsRequest {
                    project: parts.0,
                    repo: parts.1,
                    schema_path: parts.2,
                    at: Some(VersionRef {
                        r#ref: Some(schemahub_api::schemahub_v1::version_ref::Ref::Branch(branch)),
                    }),
                })
                .await
                .context("GetDescriptors RPC")?;

            let inner = resp.into_inner();
            let source = String::from_utf8(inner.descriptor_bytes.to_vec())
                .unwrap_or_else(|_| "(binary descriptor — not printable)".to_string());
            print!("{source}");
        }
    }
    Ok(())
}
