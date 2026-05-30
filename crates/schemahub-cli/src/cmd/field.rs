use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    apply_mutation_request::Operation,
    schema_service_client::SchemaServiceClient,
    ApplyMutationRequest, ProtoAddField, ProtoRemoveField, ProtoRenameField, ProtobufMutation,
};
use tonic::transport::Channel;
use uuid::Uuid;

use crate::cmd::bearer;

#[derive(Args)]
pub struct FieldArgs {
    #[command(subcommand)]
    pub action: FieldAction,
}

#[derive(Subcommand)]
pub enum FieldAction {
    /// Add a field to a Protobuf message
    Add {
        /// project/repo/schema_name
        schema_path: String,
        message: String,
        /// Format: field_name:field_type:field_number  e.g. email:string:3
        field_spec: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long, default_value = "")]
        base_revision: String,
    },
    /// Remove a field from a Protobuf message (auto-reserves the number/name)
    Remove {
        schema_path: String,
        message: String,
        field: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long, default_value = "")]
        base_revision: String,
    },
    /// Rename a field in a Protobuf message
    Rename {
        schema_path: String,
        message: String,
        old_name: String,
        new_name: String,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long, default_value = "")]
        base_revision: String,
    },
}

pub async fn run(args: FieldArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    match args.action {
        FieldAction::Add {
            schema_path,
            message,
            field_spec,
            branch,
            base_revision,
        } => {
            let parts: Vec<&str> = schema_path.splitn(3, '/').collect();
            if parts.len() != 3 {
                bail!("schema_path must be project/repo/schema_name");
            }
            let (project, repo, schema_file) = (parts[0], parts[1], parts[2]);

            // Parse field_spec: name:type:number
            let spec_parts: Vec<&str> = field_spec.splitn(3, ':').collect();
            if spec_parts.len() != 3 {
                bail!("field_spec must be name:type:number e.g. email:string:3");
            }
            let field_name = spec_parts[0].to_string();
            let field_type = spec_parts[1].to_string();
            let field_number: u32 = spec_parts[2].parse().context("field number must be u32")?;

            let mutation = ProtobufMutation {
                schema_path: schema_file.to_string(),
                operation: Some(
                    schemahub_api::schemahub_v1::protobuf_mutation::Operation::AddField(
                        ProtoAddField {
                            message_name: message,
                            field_name,
                            field_type,
                            field_number,
                            repeated: false,
                            doc_comment: String::new(),
                        },
                    ),
                ),
            };

            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .apply_mutation(bearer(
                    ApplyMutationRequest {
                        project: project.to_string(),
                        repo: repo.to_string(),
                        branch,
                        base_revision,
                        idempotency_key: Uuid::new_v4().to_string(),
                        force: false,
                        operation: Some(Operation::ProtobufOp(mutation)),
                    },
                    token,
                )?)
                .await
                .context("ApplyMutation RPC")?;

            println!("Applied. New commit: {}", resp.into_inner().new_commit);
        }
        FieldAction::Remove {
            schema_path,
            message,
            field,
            branch,
            base_revision,
        } => {
            let parts: Vec<&str> = schema_path.splitn(3, '/').collect();
            if parts.len() != 3 { bail!("schema_path must be project/repo/schema_name"); }
            let (project, repo, schema_file) = (parts[0], parts[1], parts[2]);

            let mutation = ProtobufMutation {
                schema_path: schema_file.to_string(),
                operation: Some(
                    schemahub_api::schemahub_v1::protobuf_mutation::Operation::RemoveField(
                        ProtoRemoveField {
                            message_name: message,
                            field_name: field,
                        },
                    ),
                ),
            };

            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .apply_mutation(bearer(
                    ApplyMutationRequest {
                        project: project.to_string(),
                        repo: repo.to_string(),
                        branch,
                        base_revision,
                        idempotency_key: Uuid::new_v4().to_string(),
                        force: false,
                        operation: Some(Operation::ProtobufOp(mutation)),
                    },
                    token,
                )?)
                .await
                .context("ApplyMutation RPC")?;

            println!("Removed field. New commit: {}", resp.into_inner().new_commit);
        }
        FieldAction::Rename {
            schema_path,
            message,
            old_name,
            new_name,
            branch,
            base_revision,
        } => {
            let parts: Vec<&str> = schema_path.splitn(3, '/').collect();
            if parts.len() != 3 { bail!("schema_path must be project/repo/schema_name"); }
            let (project, repo, schema_file) = (parts[0], parts[1], parts[2]);

            let mutation = ProtobufMutation {
                schema_path: schema_file.to_string(),
                operation: Some(
                    schemahub_api::schemahub_v1::protobuf_mutation::Operation::RenameField(
                        ProtoRenameField {
                            message_name: message,
                            old_field_name: old_name,
                            new_field_name: new_name,
                        },
                    ),
                ),
            };

            let mut client = SchemaServiceClient::new(channel);
            let resp = client
                .apply_mutation(bearer(
                    ApplyMutationRequest {
                        project: project.to_string(),
                        repo: repo.to_string(),
                        branch,
                        base_revision,
                        idempotency_key: Uuid::new_v4().to_string(),
                        force: false,
                        operation: Some(Operation::ProtobufOp(mutation)),
                    },
                    token,
                )?)
                .await
                .context("ApplyMutation RPC")?;

            println!("Renamed field. New commit: {}", resp.into_inner().new_commit);
        }
    }
    Ok(())
}
