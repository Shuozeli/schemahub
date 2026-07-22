//! End-to-end durable repository resource, ETag, pagination, and archive tests.

mod common;

use std::collections::HashMap;

use common::*;
use schemahub_api::schemahub_v1 as pb;
use schemahub_server::config::{AuthConfig, Config, TokenIdentity};
use schemahub_types::IdentityKind;
use tonic::metadata::MetadataValue;
use tonic::Request;

fn repository_config() -> Config {
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

async fn create_repo(
    client: &mut pb::project_service_client::ProjectServiceClient<tonic::transport::Channel>,
    name: &str,
) -> pb::RepoConfig {
    client
        .create_repo(with_token(
            Request::new(pb::CreateRepoRequest {
                project: "acme".to_string(),
                name: name.to_string(),
                default_branch: String::new(),
                compatibility_direction: pb::CompatibilityDirection::Unspecified as i32,
                protected_branches: Vec::new(),
                review_policy: None,
                serving_policy: None,
            }),
            "owner-token",
        ))
        .await
        .expect("create repository")
        .into_inner()
        .repo
        .expect("repository resource")
}

#[tokio::test]
async fn repository_crud_uses_etags_pagination_and_soft_archive() {
    // Arrange
    let url = start_server_with(repository_config()).await;
    let mut clients = clients(&url).await;
    clients
        .project
        .create_project(with_token(
            Request::new(pb::CreateProjectRequest {
                name: "acme".to_string(),
                is_public: false,
            }),
            "owner-token",
        ))
        .await
        .expect("create project");
    let commerce = create_repo(&mut clients.project, "commerce").await;
    create_repo(&mut clients.project, "events").await;

    // Act: page through the stable name ordering.
    let first_page = clients
        .project
        .list_repos(with_token(
            Request::new(pb::ListReposRequest {
                project: "acme".to_string(),
                name_prefix: String::new(),
                page_size: 1,
                page_token: String::new(),
                include_archived: false,
            }),
            "owner-token",
        ))
        .await
        .expect("first repository page")
        .into_inner();
    let second_page = clients
        .project
        .list_repos(with_token(
            Request::new(pb::ListReposRequest {
                project: "acme".to_string(),
                name_prefix: String::new(),
                page_size: 1,
                page_token: first_page.next_page_token.clone(),
                include_archived: false,
            }),
            "owner-token",
        ))
        .await
        .expect("second repository page")
        .into_inner();

    // Assert pagination and create defaults.
    assert_eq!(first_page.repos[0].name, "commerce");
    assert!(!first_page.next_page_token.is_empty());
    assert_eq!(second_page.repos[0].name, "events");
    assert!(second_page.next_page_token.is_empty());
    assert_eq!(commerce.etag, "v1");
    assert_eq!(commerce.default_branch, "main");
    assert_eq!(commerce.protected_branches, ["main"]);
    assert!(commerce
        .serving_policy
        .as_ref()
        .is_some_and(|policy| policy.source && policy.descriptors && policy.generated_code));

    // Act: update one selected field with the current ETag.
    let mut replacement = commerce.clone();
    replacement.default_branch = "stable".to_string();
    let updated = clients
        .project
        .update_repo(with_token(
            Request::new(pb::UpdateRepoRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                compatibility_direction: 0,
                protected_branches: Vec::new(),
                default_branch: String::new(),
                repo_config: Some(replacement.clone()),
                update_mask: Some(prost_types::FieldMask {
                    paths: vec!["default_branch".to_string()],
                }),
            }),
            "owner-token",
        ))
        .await
        .expect("update repository")
        .into_inner()
        .repo
        .expect("updated repository");

    // Assert a stale writer is rejected atomically.
    assert_eq!(updated.default_branch, "stable");
    assert_eq!(updated.etag, "v2");
    let stale = clients
        .project
        .update_repo(with_token(
            Request::new(pb::UpdateRepoRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                compatibility_direction: 0,
                protected_branches: Vec::new(),
                default_branch: String::new(),
                repo_config: Some(replacement),
                update_mask: Some(prost_types::FieldMask {
                    paths: vec!["default_branch".to_string()],
                }),
            }),
            "owner-token",
        ))
        .await
        .expect_err("stale update must fail");
    assert_eq!(stale.code(), tonic::Code::Aborted);

    // Act: archive an empty repository and query both normal and audit views.
    clients
        .project
        .delete_repo(with_token(
            Request::new(pb::DeleteRepoRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                force: false,
                etag: updated.etag.clone(),
            }),
            "owner-token",
        ))
        .await
        .expect("archive repository");
    let hidden = clients
        .project
        .get_repo(with_token(
            Request::new(pb::GetRepoRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                include_archived: false,
            }),
            "owner-token",
        ))
        .await
        .expect_err("archived repository hidden by default");
    let archived = clients
        .project
        .get_repo(with_token(
            Request::new(pb::GetRepoRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                include_archived: true,
            }),
            "owner-token",
        ))
        .await
        .expect("archived audit read")
        .into_inner()
        .repo
        .expect("archived repository");

    // Assert
    assert_eq!(hidden.code(), tonic::Code::NotFound);
    assert!(archived.archived);
    assert!(archived.archive_time.is_some());
    assert_eq!(archived.etag, "v3");
}

#[tokio::test]
async fn repository_listing_is_authorized_before_existence_is_revealed() {
    // Arrange
    let url = start_server_with(repository_config()).await;
    let mut clients = clients(&url).await;
    clients
        .project
        .create_project(with_token(
            Request::new(pb::CreateProjectRequest {
                name: "acme".to_string(),
                is_public: false,
            }),
            "owner-token",
        ))
        .await
        .expect("create private project");
    create_repo(&mut clients.project, "commerce").await;

    // Act
    let denied = clients
        .project
        .list_repos(with_token(
            Request::new(pb::ListReposRequest {
                project: "acme".to_string(),
                name_prefix: String::new(),
                page_size: 0,
                page_token: String::new(),
                include_archived: false,
            }),
            "stranger-token",
        ))
        .await
        .expect_err("non-member list must fail");

    // Assert
    assert_eq!(denied.code(), tonic::Code::PermissionDenied);
}

#[tokio::test]
async fn persisted_repository_policy_gates_direct_writes_and_artifact_kinds() {
    // Arrange
    let url = start_server_with(repository_config()).await;
    let mut clients = clients(&url).await;
    clients
        .project
        .create_project(with_token(
            Request::new(pb::CreateProjectRequest {
                name: "acme".to_string(),
                is_public: false,
            }),
            "owner-token",
        ))
        .await
        .expect("create project");
    let mut repository = create_repo(&mut clients.project, "commerce").await;
    clients
        .schema
        .create_schema(with_token(
            Request::new(pb::CreateSchemaRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                branch: "main".to_string(),
                schema_name: "order.proto".to_string(),
                format: pb::SchemaFormat::Protobuf as i32,
                source: "syntax = \"proto3\"; message Order { string id = 1; }".to_string(),
                base_revision: String::new(),
                idempotency_key: "policy-seed".to_string(),
            }),
            "owner-token",
        ))
        .await
        .expect("seed schema before restricting policy");
    repository.review_policy = Some(pb::ReviewPolicy {
        required_approvals: 1,
        require_change_record: true,
    });
    repository.serving_policy = Some(pb::ServingPolicy {
        source: false,
        descriptors: true,
        generated_code: true,
    });
    clients
        .project
        .update_repo(with_token(
            Request::new(pb::UpdateRepoRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                compatibility_direction: 0,
                protected_branches: Vec::new(),
                default_branch: String::new(),
                repo_config: Some(repository),
                update_mask: Some(prost_types::FieldMask {
                    paths: vec!["review_policy".to_string(), "serving_policy".to_string()],
                }),
            }),
            "owner-token",
        ))
        .await
        .expect("persist repository policies");

    // Act: attempt a direct publication, then fetch both disabled and enabled
    // artifact kinds through one immutable revision.
    let direct = clients
        .schema
        .create_schema(with_token(
            Request::new(pb::CreateSchemaRequest {
                project: "acme".to_string(),
                repo: "commerce".to_string(),
                branch: "main".to_string(),
                schema_name: "direct.proto".to_string(),
                format: pb::SchemaFormat::Protobuf as i32,
                source: "syntax = \"proto3\"; message Direct {}".to_string(),
                base_revision: String::new(),
                idempotency_key: "policy-direct".to_string(),
            }),
            "owner-token",
        ))
        .await
        .expect_err("direct write must be blocked");
    let revision = clients
        .serving
        .resolve_revision(with_token(
            Request::new(pb::ResolveRevisionRequest {
                parent: "projects/acme/repos/commerce".to_string(),
                at: Some(vref_branch("main")),
            }),
            "owner-token",
        ))
        .await
        .expect("resolve revision")
        .into_inner();
    let source = clients
        .serving
        .get_schema_artifact(with_token(
            Request::new(pb::GetSchemaArtifactRequest {
                revision: revision.name.clone(),
                schema_path: "order.proto".to_string(),
                kind: pb::SchemaArtifactKind::Source as i32,
                language: pb::Language::Unspecified as i32,
                rust_pluggable_buffer: false,
                if_none_match: String::new(),
            }),
            "owner-token",
        ))
        .await
        .expect_err("source serving must be blocked");
    let descriptors = clients
        .serving
        .get_schema_artifact(with_token(
            Request::new(pb::GetSchemaArtifactRequest {
                revision: revision.name,
                schema_path: "order.proto".to_string(),
                kind: pb::SchemaArtifactKind::Descriptors as i32,
                language: pb::Language::Unspecified as i32,
                rust_pluggable_buffer: false,
                if_none_match: String::new(),
            }),
            "owner-token",
        ))
        .await;

    // Assert
    assert_eq!(direct.code(), tonic::Code::FailedPrecondition);
    assert_eq!(source.code(), tonic::Code::FailedPrecondition);
    assert!(
        descriptors.is_ok(),
        "descriptor policy: {:?}",
        descriptors.err()
    );
}
