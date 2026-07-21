//! End-to-end immutable revision and artifact serving coverage.

mod common;

use common::*;
use schemahub_api::schemahub_v1 as pb;

const INITIAL: &str = "syntax = \"proto3\"; message User { string id = 1; }";
const UPDATED: &str = "syntax = \"proto3\"; message User { string id = 1; } message Later {}";

#[tokio::test]
async fn pinned_revision_and_conditional_artifact_remain_stable_after_main_moves() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let created = create_schema(
        &mut clients.schema,
        "acme",
        "commerce",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        INITIAL,
        "serve-create",
    )
    .await;
    let revision = clients
        .serving
        .resolve_revision(pb::ResolveRevisionRequest {
            parent: "projects/acme/repos/commerce".to_string(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("resolve revision")
        .into_inner();
    let first_response = clients
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: revision.name.clone(),
            schema_path: "user.proto".to_string(),
            kind: pb::SchemaArtifactKind::Source as i32,
            ..Default::default()
        })
        .await
        .expect("get source artifact");
    let metadata_digest = first_response
        .metadata()
        .get("x-schemahub-artifact-digest")
        .expect("digest metadata")
        .to_str()
        .expect("ASCII digest")
        .to_string();
    let first = first_response.into_inner();
    let descriptors = clients
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: revision.name.clone(),
            schema_path: "user.proto".to_string(),
            kind: pb::SchemaArtifactKind::Descriptors as i32,
            ..Default::default()
        })
        .await
        .expect("get descriptor artifact")
        .into_inner();
    let generated = clients
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: revision.name.clone(),
            schema_path: "user.proto".to_string(),
            kind: pb::SchemaArtifactKind::GeneratedCode as i32,
            language: pb::Language::Rust as i32,
            ..Default::default()
        })
        .await
        .expect("get generated-code artifact")
        .into_inner();
    clients
        .schema
        .update_schema(pb::UpdateSchemaRequest {
            project: "acme".to_string(),
            repo: "commerce".to_string(),
            branch: "main".to_string(),
            schema_name: "user.proto".to_string(),
            source: UPDATED.to_string(),
            base_revision: String::new(),
            idempotency_key: "serve-update".to_string(),
            force: false,
        })
        .await
        .expect("move main");

    // Act
    let conditional = clients
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: revision.name.clone(),
            schema_path: "user.proto".to_string(),
            kind: pb::SchemaArtifactKind::Source as i32,
            if_none_match: first.artifact_digest.clone(),
            ..Default::default()
        })
        .await
        .expect("conditional pinned read")
        .into_inner();
    let latest_revision = clients
        .serving
        .resolve_revision(pb::ResolveRevisionRequest {
            parent: "projects/acme/repos/commerce".to_string(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("resolve latest revision")
        .into_inner();
    let latest = clients
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: latest_revision.name.clone(),
            schema_path: "user.proto".to_string(),
            kind: pb::SchemaArtifactKind::Source as i32,
            ..Default::default()
        })
        .await
        .expect("get latest source")
        .into_inner();

    // Assert
    assert_eq!(revision.commit_id, created.new_commit);
    assert_eq!(metadata_digest, first.artifact_digest);
    assert!(first.artifact_digest.starts_with("sha256:"));
    assert!(first.closure_digest.starts_with("sha256:"));
    assert!(!descriptors.content.is_empty());
    assert_eq!(descriptors.closure_digest, first.closure_digest);
    assert!(String::from_utf8(generated.content)
        .unwrap()
        .contains("struct User"));
    assert_eq!(generated.closure_digest, first.closure_digest);
    assert!(String::from_utf8(first.content)
        .unwrap()
        .contains("message User"));
    assert!(conditional.not_modified);
    assert!(conditional.content.is_empty());
    assert_eq!(conditional.artifact_digest, first.artifact_digest);
    assert_ne!(latest_revision.commit_id, revision.commit_id);
    assert_ne!(latest.artifact_digest, first.artifact_digest);
    assert!(String::from_utf8(latest.content)
        .unwrap()
        .contains("message Later"));
}

#[tokio::test]
async fn artifact_revision_cannot_smuggle_commit_from_another_repository() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    create_schema(
        &mut clients.schema,
        "acme",
        "commerce",
        "main",
        "user.proto",
        pb::SchemaFormat::Protobuf,
        INITIAL,
        "serve-own",
    )
    .await;
    let foreign = create_schema(
        &mut clients.schema,
        "other",
        "private",
        "main",
        "foreign.proto",
        pb::SchemaFormat::Protobuf,
        "syntax = \"proto3\"; message Secret {}",
        "serve-foreign",
    )
    .await;

    // Act
    let error = clients
        .serving
        .get_schema_artifact(pb::GetSchemaArtifactRequest {
            revision: format!(
                "projects/acme/repos/commerce/revisions/{}",
                foreign.new_commit
            ),
            schema_path: "foreign.proto".to_string(),
            kind: pb::SchemaArtifactKind::Source as i32,
            ..Default::default()
        })
        .await
        .expect_err("foreign commit must not be readable through another repo");

    // Assert
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
}
