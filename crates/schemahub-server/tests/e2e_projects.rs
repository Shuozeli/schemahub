//! End-to-end project resource lifecycle, ETag, pagination, and archive tests.

mod common;

use std::collections::HashMap;

use common::*;
use schemahub_api::schemahub_v1 as pb;
use schemahub_core::{FileProjectStore, FileRoleStore, ProjectMeta, ProjectStore, RoleStore};
use schemahub_server::config::{AuthConfig, Config, TokenIdentity};
use schemahub_types::{IdentityKind, Role, Visibility};
use tonic::metadata::MetadataValue;
use tonic::Request;

fn project_config() -> Config {
    Config {
        auth: AuthConfig {
            data_dir: tempfile::tempdir()
                .expect("auth tempdir")
                .path()
                .to_string_lossy()
                .to_string(),
            tokens: HashMap::from([
                (
                    "owner-token".to_string(),
                    TokenIdentity {
                        id: "alice".to_string(),
                        display: Some("Alice".to_string()),
                        kind: IdentityKind::Human,
                        delegated_by: None,
                    },
                ),
                (
                    "stranger-token".to_string(),
                    TokenIdentity {
                        id: "stranger".to_string(),
                        display: None,
                        kind: IdentityKind::Human,
                        delegated_by: None,
                    },
                ),
            ]),
            jwt: None,
        },
        ..Config::default()
    }
}

fn with_token<T>(mut request: Request<T>, token: &str) -> Request<T> {
    let value: MetadataValue<_> = format!("Bearer {token}").parse().expect("metadata");
    request.metadata_mut().insert("authorization", value);
    request
}

async fn create_project(
    client: &mut pb::project_service_client::ProjectServiceClient<tonic::transport::Channel>,
    name: &str,
) -> pb::ProjectInfo {
    client
        .create_project(with_token(
            Request::new(pb::CreateProjectRequest {
                name: name.to_string(),
                is_public: false,
            }),
            "owner-token",
        ))
        .await
        .expect("create project")
        .into_inner()
        .project
        .expect("project resource")
}

async fn create_repo(
    client: &mut pb::project_service_client::ProjectServiceClient<tonic::transport::Channel>,
    project: &str,
) {
    client
        .create_repo(with_token(
            Request::new(pb::CreateRepoRequest {
                project: project.to_string(),
                name: "commerce".to_string(),
                default_branch: String::new(),
                compatibility_direction: pb::CompatibilityDirection::Unspecified as i32,
                protected_branches: Vec::new(),
                review_policy: None,
                serving_policy: None,
            }),
            "owner-token",
        ))
        .await
        .expect("create repository");
}

#[tokio::test]
async fn project_update_uses_field_mask_and_advances_etag() {
    // Arrange
    let url = start_server_with(project_config()).await;
    let mut clients = clients(&url).await;
    let created = create_project(&mut clients.project, "acme").await;
    let mut replacement = created.clone();
    replacement.is_public = true;

    // Act
    let updated = clients
        .project
        .update_project(with_token(
            Request::new(pb::UpdateProjectRequest {
                project: Some(replacement),
                update_mask: Some(prost_types::FieldMask {
                    paths: vec!["is_public".to_string()],
                }),
            }),
            "owner-token",
        ))
        .await
        .expect("update project")
        .into_inner()
        .project
        .expect("updated project");

    // Assert
    assert_eq!(created.etag, "v1");
    assert_eq!(updated.etag, "v2");
    assert!(updated.is_public);
    assert_eq!(updated.create_time, created.create_time);
    let created_update = created.update_time.expect("create update timestamp");
    let updated_update = updated.update_time.expect("updated timestamp");
    assert!(
        (updated_update.seconds, updated_update.nanos)
            >= (created_update.seconds, created_update.nanos)
    );
}

#[tokio::test]
async fn project_update_rejects_a_stale_etag() {
    // Arrange
    let url = start_server_with(project_config()).await;
    let mut clients = clients(&url).await;
    let created = create_project(&mut clients.project, "acme").await;
    let mut winner = created.clone();
    winner.is_public = true;
    clients
        .project
        .update_project(with_token(
            Request::new(pb::UpdateProjectRequest {
                project: Some(winner),
                update_mask: Some(prost_types::FieldMask {
                    paths: vec!["is_public".to_string()],
                }),
            }),
            "owner-token",
        ))
        .await
        .expect("winning update");
    let mut stale = created;
    stale.is_public = false;

    // Act
    let result = clients
        .project
        .update_project(with_token(
            Request::new(pb::UpdateProjectRequest {
                project: Some(stale),
                update_mask: Some(prost_types::FieldMask {
                    paths: vec!["is_public".to_string()],
                }),
            }),
            "owner-token",
        ))
        .await;

    // Assert
    assert_eq!(
        result.expect_err("stale update must fail").code(),
        tonic::Code::Aborted
    );
}

#[tokio::test]
async fn project_listing_uses_stable_cursor_pagination() {
    // Arrange
    let url = start_server_with(project_config()).await;
    let mut clients = clients(&url).await;
    create_project(&mut clients.project, "charlie").await;
    create_project(&mut clients.project, "alpha").await;
    create_project(&mut clients.project, "bravo").await;

    // Act
    let first = clients
        .project
        .list_projects(with_token(
            Request::new(pb::ListProjectsRequest {
                name_prefix: String::new(),
                page_size: 2,
                page_token: String::new(),
                include_archived: false,
            }),
            "owner-token",
        ))
        .await
        .expect("first project page")
        .into_inner();
    let second = clients
        .project
        .list_projects(with_token(
            Request::new(pb::ListProjectsRequest {
                name_prefix: String::new(),
                page_size: 2,
                page_token: first.next_page_token.clone(),
                include_archived: false,
            }),
            "owner-token",
        ))
        .await
        .expect("second project page")
        .into_inner();

    // Assert
    assert_eq!(
        first
            .projects
            .iter()
            .map(|project| project.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "bravo"]
    );
    assert!(!first.next_page_token.is_empty());
    assert_eq!(second.projects[0].name, "charlie");
    assert!(second.next_page_token.is_empty());
}

#[tokio::test]
async fn project_archive_requires_force_when_repositories_exist() {
    // Arrange
    let url = start_server_with(project_config()).await;
    let mut clients = clients(&url).await;
    let project = create_project(&mut clients.project, "acme").await;
    create_repo(&mut clients.project, "acme").await;

    // Act
    let result = clients
        .project
        .delete_project(with_token(
            Request::new(pb::DeleteProjectRequest {
                name: "acme".to_string(),
                force: false,
                etag: project.etag,
            }),
            "owner-token",
        ))
        .await;

    // Assert
    assert_eq!(
        result.expect_err("non-forced archive must fail").code(),
        tonic::Code::FailedPrecondition
    );
}

#[tokio::test]
async fn forced_project_archive_is_owner_auditable_and_runtime_inert() {
    // Arrange
    let url = start_server_with(project_config()).await;
    let mut clients = clients(&url).await;
    let project = create_project(&mut clients.project, "acme").await;
    create_repo(&mut clients.project, "acme").await;
    clients
        .project
        .delete_project(with_token(
            Request::new(pb::DeleteProjectRequest {
                name: "acme".to_string(),
                force: true,
                etag: project.etag,
            }),
            "owner-token",
        ))
        .await
        .expect("force archive project");

    // Act
    let hidden = clients
        .project
        .get_project(with_token(
            Request::new(pb::GetProjectRequest {
                name: "acme".to_string(),
                include_archived: false,
            }),
            "owner-token",
        ))
        .await;
    let audit = clients
        .project
        .get_project(with_token(
            Request::new(pb::GetProjectRequest {
                name: "acme".to_string(),
                include_archived: true,
            }),
            "owner-token",
        ))
        .await
        .expect("owner audit read")
        .into_inner()
        .project
        .expect("archived project");
    let stranger = clients
        .project
        .get_project(with_token(
            Request::new(pb::GetProjectRequest {
                name: "acme".to_string(),
                include_archived: true,
            }),
            "stranger-token",
        ))
        .await;
    let runtime = clients
        .project
        .list_repos(with_token(
            Request::new(pb::ListReposRequest {
                project: "acme".to_string(),
                name_prefix: String::new(),
                page_size: 0,
                page_token: String::new(),
                include_archived: true,
            }),
            "owner-token",
        ))
        .await;

    // Assert
    assert_eq!(
        hidden.expect_err("archived project hidden").code(),
        tonic::Code::NotFound
    );
    assert!(audit.archived);
    assert!(audit.archive_time.is_some());
    assert_eq!(audit.etag, "v2");
    assert_eq!(
        stranger.expect_err("non-owner audit denied").code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        runtime.expect_err("archived runtime denied").code(),
        tonic::Code::FailedPrecondition
    );
}

#[tokio::test]
async fn legacy_json_projects_and_members_are_imported_into_object_db() {
    // Arrange
    let legacy_dir = tempfile::tempdir().expect("legacy access-store directory");
    FileProjectStore::open(legacy_dir.path().join("projects.json"))
        .expect("open legacy projects")
        .set(ProjectMeta::new(
            "legacy",
            Visibility::Private,
            "alice",
            1_000,
        ))
        .expect("write legacy project");
    let legacy_roles =
        FileRoleStore::open(legacy_dir.path().join("roles.json")).expect("open legacy roles");
    legacy_roles
        .set("legacy", "alice", Role::Owner)
        .expect("write legacy owner");
    legacy_roles
        .set("legacy", "agent", Role::Writer)
        .expect("write legacy agent");
    let mut config = project_config();
    config.auth.data_dir = legacy_dir.path().to_string_lossy().to_string();
    let url = start_server_with(config).await;
    let mut clients = clients(&url).await;

    // Act
    let mut members = clients
        .project
        .list_members(with_token(
            Request::new(pb::ListMembersRequest {
                project: "legacy".to_string(),
            }),
            "owner-token",
        ))
        .await
        .expect("list migrated members")
        .into_inner()
        .members;
    members.sort_by(|left, right| left.identity.cmp(&right.identity));

    // Assert
    assert_eq!(
        members
            .into_iter()
            .map(|member| (member.identity, member.role))
            .collect::<Vec<_>>(),
        [
            ("agent".to_string(), pb::Role::Writer as i32),
            ("alice".to_string(), pb::Role::Owner as i32),
        ]
    );
}
