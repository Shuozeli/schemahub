//! `schemahub project ...` — project + member management (design.md §6 RBAC).
//!
//! Lifecycle and membership commands:
//! - `schemahub project create <name> [--public]` — calls
//!   `ProjectService.CreateProject`. The caller (resolved by the server's
//!   `AuthnProvider`) becomes the project Owner.
//! - `get`, `list`, `set-visibility`, and `archive` expose durable project
//!   resources with ETags, pagination, and owner-only archive audit reads.
//! - `schemahub project member list|add|remove|set-role <project> [identity_id]
//!   [--role=Reader|Writer|Maintainer|Owner]` — wraps `AddMember`,
//!   `RemoveMember`, `UpdateMemberRole`, and bounded `ListMembers`.
//!
//! The bearer token comes from `--token` / `SCHEMAHUB_TOKEN` (already wired
//! globally in `main.rs`) and is attached as `Authorization: Bearer <token>`
//! on each request.

use anyhow::Context;
use clap::{Args, Subcommand};
use prost_types::FieldMask;
use schemahub_api::schemahub_v1::{self as pb, project_service_client::ProjectServiceClient};
use serde_json::{json, Value};
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
    /// List immutable project/member/repository administrative events.
    Audit {
        project: String,
        /// Number of events fetched per RPC.
        #[arg(long, default_value_t = 50)]
        page_size: i32,
        /// Emit one stable JSON document for agents and automation.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum MemberAction {
    /// List active project members in stable identity order.
    List {
        project: String,
        /// Number of members fetched per RPC.
        #[arg(long, default_value_t = 50)]
        page_size: i32,
        /// Emit one stable JSON document for agents and automation.
        #[arg(long)]
        json: bool,
    },
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
            MemberAction::List {
                project,
                page_size,
                json,
            } => {
                let mut page_token = String::new();
                let mut members = Vec::new();
                loop {
                    let response = client
                        .list_members(bearer(
                            pb::ListMembersRequest {
                                project: project.clone(),
                                page_size,
                                page_token,
                            },
                            token,
                        )?)
                        .await
                        .context("ListMembers RPC")?
                        .into_inner();
                    members.extend(response.members);
                    if response.next_page_token.is_empty() {
                        break;
                    }
                    page_token = response.next_page_token;
                }
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &members.iter().map(member_json).collect::<Vec<_>>()
                        )
                        .context("encode project member JSON")?
                    );
                } else {
                    for member in members {
                        let role = pb::Role::try_from(member.role)
                            .map(|role| format!("{role:?}"))
                            .unwrap_or_else(|_| "Unspecified".to_string());
                        println!("{}\t{}", member.identity, role);
                    }
                }
            }
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
        ProjectAction::Audit {
            project,
            page_size,
            json,
        } => {
            let mut page_token = String::new();
            let mut events = Vec::new();
            loop {
                let response = client
                    .list_control_plane_audit_events(bearer(
                        pb::ListControlPlaneAuditEventsRequest {
                            parent: format!("projects/{project}"),
                            page_size,
                            page_token,
                        },
                        token,
                    )?)
                    .await
                    .context("ListControlPlaneAuditEvents RPC")?
                    .into_inner();
                events.extend(response.audit_events);
                if response.next_page_token.is_empty() {
                    break;
                }
                page_token = response.next_page_token;
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &events.iter().map(audit_event_json).collect::<Vec<_>>()
                    )
                    .context("encode control-plane audit JSON")?
                );
            } else {
                for event in events {
                    let action = pb::ControlPlaneAuditAction::try_from(event.action)
                        .map(|action| action.as_str_name())
                        .unwrap_or("CONTROL_PLANE_AUDIT_ACTION_UNSPECIFIED");
                    let timestamp = event
                        .event_time
                        .as_ref()
                        .map(|value| format!("{}.{:09}Z", value.seconds, value.nanos))
                        .unwrap_or_else(|| "-".to_string());
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        event.name, action, event.actor, event.resource_name, timestamp
                    );
                }
            }
        }
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

fn audit_event_json(event: &pb::ControlPlaneAuditEvent) -> Value {
    json!({
        "name": event.name,
        "event_id": event.event_id,
        "project": event.project,
        "resource_name": event.resource_name,
        "action": pb::ControlPlaneAuditAction::try_from(event.action)
            .map(|action| action.as_str_name())
            .unwrap_or("CONTROL_PLANE_AUDIT_ACTION_UNSPECIFIED"),
        "actor": event.actor,
        "event_time": event.event_time.as_ref().map(timestamp_json),
        "before": event.before.as_ref().map(audit_snapshot_json),
        "after": event.after.as_ref().map(audit_snapshot_json),
    })
}

fn member_json(member: &pb::MemberEntry) -> Value {
    json!({
        "identity": member.identity,
        "role": pb::Role::try_from(member.role)
            .map(|role| role.as_str_name())
            .unwrap_or("ROLE_UNSPECIFIED"),
    })
}

fn audit_snapshot_json(snapshot: &pb::ControlPlaneAuditSnapshot) -> Value {
    use pb::control_plane_audit_snapshot::Resource;

    match snapshot.resource.as_ref() {
        Some(Resource::Project(project)) => json!({
            "resource_type": "project",
            "resource": project_json(project),
        }),
        Some(Resource::Member(member)) => json!({
            "resource_type": "member",
            "resource": {
                "identity": member.identity,
                "role": pb::Role::try_from(member.role)
                    .map(|role| role.as_str_name())
                    .unwrap_or("ROLE_UNSPECIFIED"),
                "active": member.active,
            },
        }),
        Some(Resource::Repository(repository)) => json!({
            "resource_type": "repository",
            "resource": repository_json(repository),
        }),
        None => Value::Null,
    }
}

fn project_json(project: &pb::ProjectInfo) -> Value {
    json!({
        "name": project.name,
        "is_public": project.is_public,
        "owner": project.owner,
        "etag": project.etag,
        "create_time": project.create_time.as_ref().map(timestamp_json),
        "update_time": project.update_time.as_ref().map(timestamp_json),
        "archived": project.archived,
        "archive_time": project.archive_time.as_ref().map(timestamp_json),
    })
}

fn repository_json(repository: &pb::RepoConfig) -> Value {
    json!({
        "project": repository.project,
        "name": repository.name,
        "default_branch": repository.default_branch,
        "compatibility_direction": pb::CompatibilityDirection::try_from(
            repository.compatibility_direction
        )
        .map(|direction| direction.as_str_name())
        .unwrap_or("COMPATIBILITY_DIRECTION_UNSPECIFIED"),
        "protected_branches": repository.protected_branches,
        "review_policy": repository.review_policy.as_ref().map(|policy| json!({
            "required_approvals": policy.required_approvals,
            "require_change_record": policy.require_change_record,
        })),
        "serving_policy": repository.serving_policy.as_ref().map(|policy| json!({
            "source": policy.source,
            "descriptors": policy.descriptors,
            "generated_code": policy.generated_code,
        })),
        "etag": repository.etag,
        "create_time": repository.create_time.as_ref().map(timestamp_json),
        "update_time": repository.update_time.as_ref().map(timestamp_json),
        "archived": repository.archived,
        "archive_time": repository.archive_time.as_ref().map(timestamp_json),
    })
}

fn timestamp_json(timestamp: &prost_types::Timestamp) -> Value {
    json!({
        "seconds": timestamp.seconds,
        "nanos": timestamp.nanos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_json_preserves_identity_and_stable_role_name() {
        // Arrange
        let member = pb::MemberEntry {
            identity: "schema-agent".to_string(),
            role: pb::Role::Writer as i32,
        };

        // Act
        let output = member_json(&member);

        // Assert
        assert_eq!(output["identity"], json!("schema-agent"));
        assert_eq!(output["role"], json!("ROLE_WRITER"));
    }

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

    #[test]
    fn audit_event_json_preserves_typed_member_snapshot() {
        // Arrange
        let event = pb::ControlPlaneAuditEvent {
            name: "projects/acme/auditEvents/audit-1".to_string(),
            event_id: "audit-1".to_string(),
            project: "acme".to_string(),
            resource_name: "projects/acme/members/agent".to_string(),
            action: pb::ControlPlaneAuditAction::MemberAdded as i32,
            actor: "alice".to_string(),
            event_time: Some(prost_types::Timestamp {
                seconds: 1,
                nanos: 2,
            }),
            before: None,
            after: Some(pb::ControlPlaneAuditSnapshot {
                resource: Some(pb::control_plane_audit_snapshot::Resource::Member(
                    pb::MemberAuditSnapshot {
                        identity: "agent".to_string(),
                        role: pb::Role::Writer as i32,
                        active: true,
                    },
                )),
            }),
        };

        // Act
        let output = audit_event_json(&event);

        // Assert
        assert_eq!(
            output["action"],
            json!("CONTROL_PLANE_AUDIT_ACTION_MEMBER_ADDED")
        );
        assert_eq!(output["actor"], json!("alice"));
        assert_eq!(output["after"]["resource_type"], json!("member"));
        assert_eq!(output["after"]["resource"]["identity"], json!("agent"));
        assert_eq!(output["after"]["resource"]["role"], json!("ROLE_WRITER"));
    }
}
