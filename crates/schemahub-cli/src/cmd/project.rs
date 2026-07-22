//! `schemahub project ...` — project + member management (design.md §6 RBAC).
//!
//! Lifecycle and membership commands:
//! - `schemahub project create <name> [--public]` — calls
//!   `ProjectService.CreateProject`. The caller (resolved by the server's
//!   `AuthnProvider`) becomes the project Owner.
//! - `get`, `list`, `set-visibility`, and `archive` expose durable project
//!   resources with ETags, pagination, and owner-only archive audit reads.
//! - `schemahub project member add|remove|set-role <project> <identity_id>
//!   [--role=Reader|Writer|Maintainer|Owner]` — wraps `AddMember`,
//!   `RemoveMember`, `UpdateMemberRole`. All three RPCs are Owner-only.
//!
//! The bearer token comes from `--token` / `SCHEMAHUB_TOKEN` (already wired
//! globally in `main.rs`) and is attached as `Authorization: Bearer <token>`
//! on each request.

use anyhow::Context;
use clap::{Args, Subcommand};
use prost_types::FieldMask;
use schemahub_api::schemahub_v1::{self as pb, project_service_client::ProjectServiceClient};
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
    /// Get a project resource.
    Get {
        name: String,
        /// Include an archived project (Owner-only).
        #[arg(long)]
        include_archived: bool,
    },
    /// List projects visible to the caller in stable paginated order.
    List {
        /// Filter by project-name prefix.
        #[arg(long, default_value = "")]
        prefix: String,
        /// Number of projects fetched per RPC.
        #[arg(long, default_value_t = 50)]
        page_size: i32,
        /// Include archived projects owned by the caller.
        #[arg(long)]
        include_archived: bool,
    },
    /// Change a project's visibility using its current ETag.
    SetVisibility {
        name: String,
        /// `public` or `private`.
        visibility: String,
        /// Current project ETag returned by create/get/list.
        #[arg(long)]
        etag: String,
    },
    /// Soft-delete a project while retaining repositories and schema history.
    Archive {
        name: String,
        /// Current project ETag returned by create/get/list.
        #[arg(long)]
        etag: String,
        /// Archive even when repository records exist.
        #[arg(long)]
        force: bool,
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
                "Created project '{}' ({}, ETag {}). You are the Owner.",
                proj.name,
                if proj.is_public { "public" } else { "private" },
                proj.etag
            );
        }
        ProjectAction::Get {
            name,
            include_archived,
        } => {
            let response = client
                .get_project(bearer(
                    pb::GetProjectRequest {
                        name,
                        include_archived,
                    },
                    token,
                )?)
                .await
                .context("GetProject RPC")?
                .into_inner();
            print_project(&response.project.unwrap_or_default());
        }
        ProjectAction::List {
            prefix,
            page_size,
            include_archived,
        } => {
            let mut page_token = String::new();
            loop {
                let response = client
                    .list_projects(bearer(
                        pb::ListProjectsRequest {
                            name_prefix: prefix.clone(),
                            page_size,
                            page_token,
                            include_archived,
                        },
                        token,
                    )?)
                    .await
                    .context("ListProjects RPC")?
                    .into_inner();
                for project in response.projects {
                    print_project(&project);
                }
                if response.next_page_token.is_empty() {
                    break;
                }
                page_token = response.next_page_token;
            }
        }
        ProjectAction::SetVisibility {
            name,
            visibility,
            etag,
        } => {
            let is_public = parse_visibility(&visibility)?;
            let response = client
                .update_project(bearer(
                    pb::UpdateProjectRequest {
                        project: Some(pb::ProjectInfo {
                            name,
                            is_public,
                            etag,
                            ..Default::default()
                        }),
                        update_mask: Some(FieldMask {
                            paths: vec!["is_public".to_string()],
                        }),
                    },
                    token,
                )?)
                .await
                .context("UpdateProject RPC")?
                .into_inner();
            print_project(&response.project.unwrap_or_default());
        }
        ProjectAction::Archive { name, etag, force } => {
            client
                .delete_project(bearer(
                    pb::DeleteProjectRequest { name, force, etag },
                    token,
                )?)
                .await
                .context("DeleteProject RPC")?;
            println!("Project archived; repositories and schema history were retained.");
        }
        ProjectAction::Member { action } => match action {
            MemberAction::Add {
                project,
                identity_id,
                role,
            } => {
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
            MemberAction::Remove {
                project,
                identity_id,
            } => {
                let req = bearer(
                    pb::RemoveMemberRequest {
                        project: project.clone(),
                        identity: identity_id.clone(),
                    },
                    token,
                )?;
                client
                    .remove_member(req)
                    .await
                    .context("RemoveMember RPC")?;
                println!("Removed '{identity_id}' from '{project}'.");
            }
            MemberAction::SetRole {
                project,
                identity_id,
                role,
            } => {
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

fn print_project(project: &pb::ProjectInfo) {
    println!(
        "{}\t{}\t{}\t{}",
        project.name,
        if project.is_public {
            "public"
        } else {
            "private"
        },
        if project.archived {
            "archived"
        } else {
            "active"
        },
        project.etag
    );
}

fn parse_visibility(value: &str) -> anyhow::Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "public" => Ok(true),
        "private" => Ok(false),
        _ => anyhow::bail!("visibility must be 'public' or 'private'"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_visibility_accepts_public_and_private_case_insensitively() {
        // Arrange
        let values = ["public", "PRIVATE"];

        // Act
        let parsed = values.map(parse_visibility);

        // Assert
        assert!(matches!(parsed, [Ok(true), Ok(false)]));
    }

    #[test]
    fn parse_visibility_rejects_unknown_values() {
        // Arrange
        let value = "internal";

        // Act
        let result = parse_visibility(value);

        // Assert
        assert_eq!(
            result
                .expect_err("unknown visibility must fail")
                .to_string(),
            "visibility must be 'public' or 'private'"
        );
    }
}
