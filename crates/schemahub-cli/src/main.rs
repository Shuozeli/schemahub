mod client;
mod cmd;
mod config;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use cmd::{
    artifact, branch, capabilities, change, codegen, field, history, log, project, repo, schema,
    tag,
};
use serde_json::json;
use std::process::ExitCode;

const BUILD_VERSION: &str = match option_env!("SCHEMAHUB_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "schemahub",
    about = "schemahub schema registry CLI",
    version = BUILD_VERSION
)]
struct Cli {
    /// Server address (overrides config and SCHEMAHUB_SERVER)
    #[arg(long, env = "SCHEMAHUB_SERVER", global = true)]
    server: Option<String>,

    /// Auth token (overrides config and SCHEMAHUB_TOKEN)
    #[arg(long, env = "SCHEMAHUB_TOKEN", global = true)]
    token: Option<String>,

    /// Config profile to use
    #[arg(long, default_value = "default", global = true)]
    profile: String,

    /// Emit a single structured JSON object on stderr when the command fails.
    #[arg(long, env = "SCHEMAHUB_JSON_ERRORS", global = true)]
    json_errors: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a project and repo on the server
    Repo(repo::RepoArgs),
    /// Project + member management (design.md §6 RBAC)
    Project(project::ProjectArgs),
    /// Schema file lifecycle operations
    Schema(schema::SchemaArgs),
    /// Record and inspect human/agent schema-change intent
    Change(change::ChangeArgs),
    /// Field mutations on Protobuf schemas
    Field(field::FieldArgs),
    /// Branch management
    Branch(branch::BranchArgs),
    /// Tag management
    Tag(tag::TagArgs),
    /// Show commit history for a repo
    Log(log::LogArgs),
    /// Operation log (jj-style audit record): `op log <project/repo>`
    Op(history::OpArgs),
    /// Undo the last operation on a repo
    Undo(history::UndoArgs),
    /// Render or resolve a conflicted declaration
    Resolve(history::ResolveArgs),
    /// Code generation
    Codegen(codegen::CodegenArgs),
    /// Resolve immutable revisions and fetch/verify served artifacts
    Artifact(artifact::ArtifactArgs),
    /// Inspect the server's versioned per-format support contract
    Capabilities(capabilities::CapabilitiesArgs),
    /// Print diff between two refs
    Diff {
        /// project/repo
        repo: String,
        /// base..head (e.g. "main..feature/xyz" or two branch names separated by "..")
        range: String,
        /// Optional: restrict diff to one schema file
        #[arg(long, default_value = "")]
        schema_path: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_errors = cli.json_errors;
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let classification = classify_error(&error);
            if json_errors {
                eprintln!(
                    "{}",
                    serde_json::to_string(&json!({
                        "error": {
                            "exit_code": classification.exit_code,
                            "kind": classification.kind,
                            "grpc_code": classification.grpc_code,
                            "message": format!("{error:#}"),
                        }
                    }))
                    .expect("error JSON is serializable")
                );
            } else {
                eprintln!("schemahub: {error:#}");
            }
            ExitCode::from(classification.exit_code)
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let cfg = config::Config::load(&cli.profile, cli.server.as_deref(), cli.token.as_deref())
        .context("loading config")?;

    match cli.command {
        Commands::Repo(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            repo::run(args, ch, &cfg.token).await
        }
        Commands::Project(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            project::run(args, ch, &cfg.token).await
        }
        Commands::Schema(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            schema::run(args, ch, &cfg.token).await
        }
        Commands::Change(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            change::run(args, ch, &cfg.token).await
        }
        Commands::Field(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            field::run(args, ch, &cfg.token).await
        }
        Commands::Branch(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            branch::run(args, ch, &cfg.token).await
        }
        Commands::Tag(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            tag::run(args, ch, &cfg.token).await
        }
        Commands::Log(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            log::run(args, ch, &cfg.token).await
        }
        Commands::Op(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            history::run_op(args, ch, &cfg.token).await
        }
        Commands::Undo(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            history::run_undo(args, ch, &cfg.token).await
        }
        Commands::Resolve(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            history::run_resolve(args, ch, &cfg.token).await
        }
        Commands::Codegen(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            codegen::run(args, ch, &cfg.token).await
        }
        Commands::Artifact(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            artifact::run(args, ch, &cfg.token).await
        }
        Commands::Capabilities(args) => {
            let ch = client::build_channel(&cfg.server).await?;
            capabilities::run(args, ch, &cfg.token).await
        }
        Commands::Diff {
            repo,
            range,
            schema_path,
        } => {
            let parts: Vec<&str> = repo.splitn(2, '/').collect();
            if parts.len() != 2 {
                bail!("repo must be 'project/repo'");
            }
            let (project, repo_name) = (parts[0].to_string(), parts[1].to_string());

            // Parse range as "base..head"
            let range_parts: Vec<&str> = range.splitn(2, "..").collect();
            if range_parts.len() != 2 {
                bail!("range must be 'base..head'");
            }
            let (base_str, head_str) = (range_parts[0], range_parts[1]);

            use schemahub_api::schemahub_v1::{
                ref_service_client::RefServiceClient, DiffRequest, VersionRef,
            };

            let ch = client::build_channel(&cfg.server).await?;
            let mut client = RefServiceClient::new(ch);
            let resp = client
                .diff(cmd::bearer(
                    DiffRequest {
                        project,
                        repo: repo_name,
                        base: Some(VersionRef {
                            r#ref: Some(cmd::parse_ref(base_str)),
                        }),
                        head: Some(VersionRef {
                            r#ref: Some(cmd::parse_ref(head_str)),
                        }),
                        schema_path,
                    },
                    &cfg.token,
                )?)
                .await
                .context("Diff RPC")?;

            let diffs = resp.into_inner().schema_diffs;
            if diffs.is_empty() {
                println!("(no changes)");
            }
            for schema_diff in diffs {
                println!("schema: {}", schema_diff.schema_path);
                for change in schema_diff.changes {
                    println!("  {} {}", change.change_type, change.decl_name);
                }
            }
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ErrorClassification {
    exit_code: u8,
    kind: &'static str,
    grpc_code: Option<&'static str>,
}

fn classify_error(error: &anyhow::Error) -> ErrorClassification {
    if let Some(status) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tonic::Status>())
    {
        return classify_grpc_code(status.code());
    }
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<tonic::transport::Error>().is_some())
    {
        return ErrorClassification {
            exit_code: 20,
            kind: "transport_unavailable",
            grpc_code: None,
        };
    }
    ErrorClassification {
        exit_code: 1,
        kind: "local_error",
        grpc_code: None,
    }
}

fn classify_grpc_code(code: tonic::Code) -> ErrorClassification {
    use tonic::Code;

    let (exit_code, kind, grpc_code) = match code {
        Code::InvalidArgument => (2, "invalid_argument", "INVALID_ARGUMENT"),
        Code::OutOfRange => (2, "invalid_argument", "OUT_OF_RANGE"),
        Code::Unauthenticated => (10, "unauthenticated", "UNAUTHENTICATED"),
        Code::PermissionDenied => (11, "permission_denied", "PERMISSION_DENIED"),
        Code::NotFound => (12, "not_found", "NOT_FOUND"),
        Code::AlreadyExists => (13, "already_exists", "ALREADY_EXISTS"),
        Code::FailedPrecondition => (14, "state_conflict", "FAILED_PRECONDITION"),
        Code::Aborted => (14, "state_conflict", "ABORTED"),
        Code::Cancelled => (20, "temporarily_unavailable", "CANCELLED"),
        Code::DeadlineExceeded => (20, "temporarily_unavailable", "DEADLINE_EXCEEDED"),
        Code::Unavailable => (20, "temporarily_unavailable", "UNAVAILABLE"),
        Code::ResourceExhausted => (21, "resource_exhausted", "RESOURCE_EXHAUSTED"),
        Code::Unimplemented => (22, "unimplemented", "UNIMPLEMENTED"),
        Code::Unknown => (22, "server_error", "UNKNOWN"),
        Code::Internal => (22, "server_error", "INTERNAL"),
        Code::DataLoss => (22, "server_error", "DATA_LOSS"),
        Code::Ok => (0, "ok", "OK"),
    };
    ErrorClassification {
        exit_code,
        kind,
        grpc_code: Some(grpc_code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_dependents_accepts_agent_json_output() {
        // Arrange
        let arguments = [
            "schemahub",
            "schema",
            "dependents",
            "acme/provider/types.proto",
            "--json",
        ];

        // Act
        let cli = Cli::try_parse_from(arguments).expect("parse dependents command");

        // Assert
        let Commands::Schema(args) = cli.command else {
            panic!("expected schema command");
        };
        let schema::SchemaAction::Dependents { schema_path, json } = args.action else {
            panic!("expected dependents action");
        };
        assert_eq!(schema_path, "acme/provider/types.proto");
        assert!(json);
    }

    #[test]
    fn grpc_errors_have_stable_agent_exit_codes() {
        // Arrange
        let cases = [
            (tonic::Code::Unauthenticated, 10, "unauthenticated"),
            (tonic::Code::PermissionDenied, 11, "permission_denied"),
            (tonic::Code::NotFound, 12, "not_found"),
            (tonic::Code::AlreadyExists, 13, "already_exists"),
            (tonic::Code::Aborted, 14, "state_conflict"),
            (tonic::Code::Unavailable, 20, "temporarily_unavailable"),
            (tonic::Code::ResourceExhausted, 21, "resource_exhausted"),
            (tonic::Code::Internal, 22, "server_error"),
        ];

        // Act
        let classified: Vec<_> = cases
            .iter()
            .map(|(code, _, _)| classify_grpc_code(*code))
            .collect();

        // Assert
        for (classification, (_, exit_code, kind)) in classified.iter().zip(cases) {
            assert_eq!(classification.exit_code, exit_code);
            assert_eq!(classification.kind, kind);
        }
    }

    #[test]
    fn wrapped_status_is_discovered_through_anyhow_context() {
        // Arrange
        let error = anyhow::Error::new(tonic::Status::failed_precondition("stale etag"))
            .context("ApplyChange RPC");

        // Act
        let classification = classify_error(&error);

        // Assert
        assert_eq!(classification.exit_code, 14);
        assert_eq!(classification.kind, "state_conflict");
        assert_eq!(classification.grpc_code, Some("FAILED_PRECONDITION"));
    }
}
