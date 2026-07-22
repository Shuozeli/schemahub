use anyhow::Context;
use clap::Args;
use schemahub_api::schemahub_v1::{
    self as pb, admin_service_client::AdminServiceClient, GetFormatCapabilitiesRequest,
};
use serde_json::json;
use tonic::transport::Channel;

use crate::cmd::bearer;

#[derive(Args)]
pub struct CapabilitiesArgs {
    /// Emit the stable machine-readable capability document.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: CapabilitiesArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    let mut client = AdminServiceClient::new(channel);
    let response = client
        .get_format_capabilities(bearer(GetFormatCapabilitiesRequest {}, token)?)
        .await
        .context("GetFormatCapabilities RPC")?
        .into_inner();

    if args.json {
        let formats: Vec<_> = response
            .formats
            .iter()
            .map(|format| {
                let operations: Vec<_> = format
                    .operations
                    .iter()
                    .map(|operation| {
                        json!({
                            "operation": operation.operation,
                            "status": capability_status(operation.status),
                            "apply_mutation": operation.apply_mutation,
                            "apply_transaction": operation.apply_transaction,
                            "notes": operation.notes,
                        })
                    })
                    .collect();
                let generated_code_languages: Vec<_> = format
                    .generated_code_languages
                    .iter()
                    .map(|language| language_name(*language))
                    .collect();
                json!({
                    "format_id": format.format_id,
                    "parse_and_print": format.parse_and_print,
                    "compatibility": format.compatibility,
                    "conflict_resolution": format.conflict_resolution,
                    "descriptor_artifact": format.descriptor_artifact,
                    "generated_code_languages": generated_code_languages,
                    "operations": operations,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "matrix_version": response.matrix_version,
                "formats": formats,
            }))?
        );
        return Ok(());
    }

    println!("format capability matrix {}", response.matrix_version);
    for format in response.formats {
        let languages = format
            .generated_code_languages
            .iter()
            .map(|language| language_name(*language))
            .collect::<Vec<_>>()
            .join(", ");
        println!("\n{}", format.format_id);
        println!(
            "  parse/print={} compatibility={} conflicts={} descriptors={} codegen={}",
            yes_no(format.parse_and_print),
            yes_no(format.compatibility),
            yes_no(format.conflict_resolution),
            yes_no(format.descriptor_artifact),
            if languages.is_empty() {
                "none"
            } else {
                &languages
            }
        );
        for operation in format.operations {
            let surfaces = match (operation.apply_mutation, operation.apply_transaction) {
                (true, true) => "mutation, transaction",
                (true, false) => "mutation",
                (false, true) => "transaction",
                (false, false) => "none",
            };
            println!(
                "  {:<28} {:<10} {}",
                operation.operation,
                capability_status(operation.status),
                surfaces
            );
            if !operation.notes.is_empty() {
                println!("    {}", operation.notes);
            }
        }
    }
    Ok(())
}

fn capability_status(value: i32) -> &'static str {
    match pb::CapabilityStatus::try_from(value).unwrap_or(pb::CapabilityStatus::Unspecified) {
        pb::CapabilityStatus::Supported => "supported",
        pb::CapabilityStatus::Rejected => "rejected",
        pb::CapabilityStatus::Unspecified => "unspecified",
    }
}

fn language_name(value: i32) -> &'static str {
    match pb::Language::try_from(value).unwrap_or(pb::Language::Unspecified) {
        pb::Language::Rust => "rust",
        pb::Language::Go => "go",
        pb::Language::Typescript => "typescript",
        pb::Language::Python => "python",
        pb::Language::Java => "java",
        pb::Language::Unspecified => "unspecified",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
