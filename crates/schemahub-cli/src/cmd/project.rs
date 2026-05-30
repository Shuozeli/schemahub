//! `schemahub project ...` — project + member management (design.md §6 RBAC).
//!
//! Two subcommands:
//! - `schemahub project create <name> [--public]` — calls
//!   `ProjectService.CreateProject`. The caller (resolved by the server's
//!   `AuthnProvider`) becomes the project Owner.
//! - `schemahub project member add|remove|set-role <project> <identity_id>
//!   [--role=Reader|Writer|Maintainer|Owner]` — wraps `AddMember`,
//!   `RemoveMember`, `UpdateMemberRole`. All three RPCs are Owner-only.
//!
//! The bearer token comes from `--token` / `SCHEMAHUB_TOKEN` (already wired
//! globally in `main.rs`) and is attached as `Authorization: Bearer <token>`
//! on each request.

use anyhow::Context;
use clap::{Args, Subcommand};
use schemahub_api::schemahub_v1::{
    self as pb, project_service_client::ProjectServiceClient,
};
use tonic::transport::Channel;

use crate::cmd::bearer;

#[derive(Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub action: ProjectAction,
}

#[derive(Subcommand)]
pub enum ProjectAction {
    /// Create a new project. The caller becomes the Owner.
    Create {
        /// Project name (e.g. "acme").
        name: String,
        /// Mark the project as publicly readable (anonymous reads allowed).
        #[arg(long)]
        public: bool,
    },
    /// Member management — Owner-only.
    Member {
        #[command(subcommand)]
        action: MemberAction,
    },
}

#[derive(Subcommand)]
pub enum MemberAction {
    /// Add a member to a project.
    Add {
        project: String,
        identity_id: String,
        #[arg(long, default_value = "Reader")]
        role: String,
    },
    /// Remove a member from a project.
    Remove {
        project: String,
        identity_id: String,
    },
    /// Change an existing member's role.
    SetRole {
        project: String,
        identity_id: String,
        #[arg(long)]
        role: String,
    },
}

pub async fn run(args: ProjectArgs, channel: Channel, token: &str) -> anyhow::Result<()> {
    let mut client = ProjectServiceClient::new(channel);
    match args.action {
        ProjectAction::Create { name, public } => {
            let req = bearer(
                pb::CreateProjectRequest {
                    name: name.clone(),
                    is_public: public,
                },
                token,
            )?;
            let resp = client
                .create_project(req)
                .await
                .context("CreateProject RPC")?
                .into_inner();
            let proj = resp.project.unwrap_or_default();
            println!(
                "Created project '{}' ({}). You are the Owner.",
                proj.name,
                if proj.is_public { "public" } else { "private" }
            );
        }
        ProjectAction::Member { action } => match action {
            MemberAction::Add { project, identity_id, role } => {
                let role_pb = parse_role(&role)?;
                let req = bearer(
                    pb::AddMemberRequest {
                        project: project.clone(),
                        identity: identity_id.clone(),
                        role: role_pb as i32,
                    },
                    token,
                )?;
                client.add_member(req).await.context("AddMember RPC")?;
                println!("Added '{identity_id}' to '{project}' as {role:?}.");
            }
            MemberAction::Remove { project, identity_id } => {
                let req = bearer(
                    pb::RemoveMemberRequest {
                        project: project.clone(),
                        identity: identity_id.clone(),
                    },
                    token,
                )?;
                client.remove_member(req).await.context("RemoveMember RPC")?;
                println!("Removed '{identity_id}' from '{project}'.");
            }
            MemberAction::SetRole { project, identity_id, role } => {
                let role_pb = parse_role(&role)?;
                let req = bearer(
                    pb::UpdateMemberRoleRequest {
                        project: project.clone(),
                        identity: identity_id.clone(),
                        new_role: role_pb as i32,
                    },
                    token,
                )?;
                client
                    .update_member_role(req)
                    .await
                    .context("UpdateMemberRole RPC")?;
                println!("Set '{identity_id}' on '{project}' to {role:?}.");
            }
        },
    }
    Ok(())
}

/// Map a CLI role string (case-insensitive) to the proto enum.
fn parse_role(s: &str) -> anyhow::Result<pb::Role> {
    match s.to_ascii_lowercase().as_str() {
        "reader" => Ok(pb::Role::Reader),
        "writer" => Ok(pb::Role::Writer),
        "maintainer" => Ok(pb::Role::Maintainer),
        "owner" => Ok(pb::Role::Owner),
        other => anyhow::bail!(
            "unknown role {other:?}; expected one of Reader / Writer / Maintainer / Owner"
        ),
    }
}

