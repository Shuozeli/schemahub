//! Immutable schema revision and artifact commands.

use std::path::PathBuf;

use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    self as pb, serving_service_client::ServingServiceClient, GetSchemaArtifactRequest,
    ResolveRevisionRequest, VersionRef,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tonic::transport::Channel;

use super::bearer;

#[derive(Args)]
pub struct ArtifactArgs {
    /// Emit stable machine-readable metadata JSON.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub action: ArtifactAction,
}

#[derive(Subcommand)]
pub enum ArtifactAction {
    /// Resolve a branch/tag/commit to an immutable revision resource.
    Resolve {
        /// Repository in project/repo form.
        repo: String,
        /// Branch by default; use tag:<name> or @<commit> for other refs.
        #[arg(long, default_value = "main")]
        at: String,
    },
    /// Fetch an artifact from an immutable revision.
    Fetch {
        /// projects/{project}/repos/{repo}/revisions/{commit}
        revision: String,
        #[arg(long)]
        schema_path: String,
        /// source, descriptors, or generated-code.
        #[arg(long, default_value = "source")]
        kind: String,
        /// Required for generated-code (rust, go, typescript, python, java).
        #[arg(long, default_value = "")]
        language: String,
        #[arg(long)]
        rust_pluggable_buffer: bool,
        /// Write artifact bytes to this path. Binary artifacts require it.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Return metadata only when this digest still matches.
        #[arg(long, default_value = "")]
        if_none_match: String,
    },
    /// Fetch bytes and verify their SHA-256 digest locally.
    Verify {
        revision: String,
        #[arg(long)]
        schema_path: String,
        #[arg(long, default_value = "source")]
        kind: String,
        #[arg(long, default_value = "")]
        language: String,
        #[arg(long)]
        rust_pluggable_buffer: bool,
        /// Expected digest, with or without the `sha256:` prefix.
        #[arg(long)]
        digest: String,
    },
}

pub async fn run(args: ArtifactArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    let mut client = ServingServiceClient::new(channel);
    match args.action {
        ArtifactAction::Resolve { repo, at } => {
            let (project, repo) = parse_repo(&repo)?;
            let revision = client
                .resolve_revision(bearer(
                    ResolveRevisionRequest {
                        parent: format!("projects/{project}/repos/{repo}"),
                        at: Some(VersionRef {
                            r#ref: Some(super::parse_ref(&at)),
                        }),
                    },
                    token,
                )?)
                .await
                .context("ResolveRevision RPC")?
                .into_inner();
            if args.json {
                print_json(&revision_json(&revision))?;
            } else {
                println!("{}", revision.name);
                println!("  commit: {}", revision.commit_id);
                println!("  resolved from: {}", revision.resolved_from);
            }
        }
        ArtifactAction::Fetch {
            revision,
            schema_path,
            kind,
            language,
            rust_pluggable_buffer,
            output,
            if_none_match,
        } => {
            let kind = parse_kind(&kind)?;
            let artifact = client
                .get_schema_artifact(bearer(
                    artifact_request(
                        revision,
                        schema_path,
                        kind,
                        &language,
                        rust_pluggable_buffer,
                        if_none_match,
                    )?,
                    token,
                )?)
                .await
                .context("GetSchemaArtifact RPC")?
                .into_inner();
            if let Some(path) = &output {
                if !artifact.not_modified {
                    std::fs::write(path, &artifact.content)
                        .with_context(|| format!("write artifact {}", path.display()))?;
                }
            } else if !args.json && !artifact.not_modified {
                if kind == pb::SchemaArtifactKind::Descriptors {
                    bail!("descriptor artifacts are binary; pass --output <path>");
                }
                let text = String::from_utf8(artifact.content.clone())
                    .context("artifact is not valid UTF-8; pass --output <path>")?;
                print!("{text}");
            }
            if args.json {
                print_json(&artifact_json(&artifact))?;
            } else if artifact.not_modified {
                println!("not modified: {}", artifact.artifact_digest);
            } else if output.is_some() {
                println!("{}", artifact.artifact_digest);
            }
        }
        ArtifactAction::Verify {
            revision,
            schema_path,
            kind,
            language,
            rust_pluggable_buffer,
            digest,
        } => {
            let kind = parse_kind(&kind)?;
            let artifact = client
                .get_schema_artifact(bearer(
                    artifact_request(
                        revision,
                        schema_path,
                        kind,
                        &language,
                        rust_pluggable_buffer,
                        String::new(),
                    )?,
                    token,
                )?)
                .await
                .context("GetSchemaArtifact RPC")?
                .into_inner();
            let computed = sha256_digest(&artifact.content);
            let expected = normalize_digest(&digest)?;
            if computed != artifact.artifact_digest {
                bail!(
                    "server digest {} does not match downloaded bytes {}",
                    artifact.artifact_digest,
                    computed
                );
            }
            if computed != expected {
                bail!("artifact digest mismatch: expected {expected}, got {computed}");
            }
            if args.json {
                print_json(&json!({
                    "valid": true,
                    "artifact_digest": computed,
                    "closure_digest": artifact.closure_digest,
                    "revision": artifact.revision,
                    "schema_path": artifact.schema_path,
                }))?;
            } else {
                println!("verified {computed}");
            }
        }
    }
    Ok(())
}

fn artifact_request(
    revision: String,
    schema_path: String,
    kind: pb::SchemaArtifactKind,
    language: &str,
    rust_pluggable_buffer: bool,
    if_none_match: String,
) -> anyhow::Result<GetSchemaArtifactRequest> {
    let language = if kind == pb::SchemaArtifactKind::GeneratedCode {
        parse_language(language)?
    } else {
        pb::Language::Unspecified
    };
    Ok(GetSchemaArtifactRequest {
        revision,
        schema_path,
        kind: kind as i32,
        language: language as i32,
        rust_pluggable_buffer,
        if_none_match,
    })
}

fn parse_repo(value: &str) -> anyhow::Result<(&str, &str)> {
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("repo must be project/repo, got {value:?}");
    }
    Ok((parts[0], parts[1]))
}

fn parse_kind(value: &str) -> anyhow::Result<pb::SchemaArtifactKind> {
    match value.to_ascii_lowercase().as_str() {
        "source" => Ok(pb::SchemaArtifactKind::Source),
        "descriptors" | "descriptor" => Ok(pb::SchemaArtifactKind::Descriptors),
        "generated-code" | "generated_code" | "code" => Ok(pb::SchemaArtifactKind::GeneratedCode),
        _ => bail!("artifact kind must be source, descriptors, or generated-code"),
    }
}

fn parse_language(value: &str) -> anyhow::Result<pb::Language> {
    match value.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Ok(pb::Language::Rust),
        "go" => Ok(pb::Language::Go),
        "typescript" | "ts" => Ok(pb::Language::Typescript),
        "python" | "py" => Ok(pb::Language::Python),
        "java" => Ok(pb::Language::Java),
        "" => bail!("generated-code artifacts require --language"),
        other => bail!("unsupported artifact language {other:?}"),
    }
}

fn normalize_digest(value: &str) -> anyhow::Result<String> {
    let hex_value = value.strip_prefix("sha256:").unwrap_or(value);
    if hex_value.len() != 64
        || !hex_value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("digest must be a lowercase 64-character SHA-256 hex value");
    }
    Ok(format!("sha256:{hex_value}"))
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn revision_json(revision: &pb::SchemaRevision) -> Value {
    json!({
        "name": revision.name,
        "project": revision.project,
        "repo": revision.repo,
        "commit_id": revision.commit_id,
        "resolved_from": revision.resolved_from,
    })
}

fn artifact_json(artifact: &pb::SchemaArtifact) -> Value {
    json!({
        "name": artifact.name,
        "revision": artifact.revision,
        "schema_path": artifact.schema_path,
        "kind": artifact_kind_name(artifact.kind),
        "format": schema_format_name(artifact.format),
        "media_type": artifact.media_type,
        "content_length": artifact.content.len(),
        "artifact_digest": artifact.artifact_digest,
        "closure_digest": artifact.closure_digest,
        "dependency_schemas": artifact.dependency_schemas,
        "is_archive": artifact.is_archive,
        "not_modified": artifact.not_modified,
    })
}

fn artifact_kind_name(value: i32) -> &'static str {
    match pb::SchemaArtifactKind::try_from(value) {
        Ok(pb::SchemaArtifactKind::Source) => "source",
        Ok(pb::SchemaArtifactKind::Descriptors) => "descriptors",
        Ok(pb::SchemaArtifactKind::GeneratedCode) => "generated_code",
        Ok(pb::SchemaArtifactKind::Unspecified) | Err(_) => "unspecified",
    }
}

fn schema_format_name(value: i32) -> &'static str {
    match pb::SchemaFormat::try_from(value) {
        Ok(pb::SchemaFormat::Protobuf) => "protobuf",
        Ok(pb::SchemaFormat::Flatbuffers) => "flatbuffers",
        Ok(pb::SchemaFormat::Openapi) => "openapi",
        Ok(pb::SchemaFormat::Unspecified) | Err(_) => "unspecified",
    }
}

fn print_json(value: &Value) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("encode artifact JSON")?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_normalization_requires_lowercase_sha256() {
        // Arrange
        let raw = "a".repeat(64);

        // Act
        let normalized = normalize_digest(&raw);

        // Assert
        assert_eq!(normalized.unwrap(), format!("sha256:{raw}"));
        assert!(normalize_digest("ABC").is_err());
        assert!(normalize_digest(&"A".repeat(64)).is_err());
    }

    #[test]
    fn artifact_kind_accepts_human_aliases() {
        // Arrange
        let aliases = ["source", "descriptor", "code"];

        // Act
        let kinds: Vec<_> = aliases
            .iter()
            .map(|alias| parse_kind(alias).expect("parse artifact kind"))
            .collect();

        // Assert
        assert_eq!(
            kinds,
            [
                pb::SchemaArtifactKind::Source,
                pb::SchemaArtifactKind::Descriptors,
                pb::SchemaArtifactKind::GeneratedCode,
            ]
        );
    }
}
