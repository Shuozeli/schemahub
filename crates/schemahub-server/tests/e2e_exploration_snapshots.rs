//! Public exploration/codegen snapshot and repository-ownership guarantees.

mod common;

use std::collections::HashMap;

use common::*;
use schemahub_api::schemahub_v1 as pb;
use schemahub_server::config::{Config, RepoSection};
use tokio_stream::StreamExt;
use tonic::Code;

const SOURCE_SCHEMA: &str = r#"syntax = "proto3";
package source.v1;

message Secret {
  string value = 1;
}
"#;

const TARGET_SCHEMA: &str = r#"syntax = "proto3";
package target.v1;

message PublicRecord {
  string id = 1;
}
"#;

#[tokio::test]
async fn reads_report_the_exact_snapshot_used_for_their_payload() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let created = create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "main",
        "shared.proto",
        pb::SchemaFormat::Protobuf,
        TARGET_SCHEMA,
        "snapshot-target-create",
    )
    .await;

    // Act
    let source = clients
        .explore
        .get_schema_source(pb::GetSchemaSourceRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("read source")
        .into_inner();
    let schemas = clients
        .explore
        .list_schemas(pb::ListSchemasRequest {
            project: "acme".into(),
            repo: "target".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("list schemas")
        .into_inner();
    let declarations = clients
        .explore
        .list_declarations(pb::ListDeclarationsRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            at: Some(vref_branch("main")),
            kind_filter: pb::DeclKind::Unspecified as i32,
        })
        .await
        .expect("list declarations")
        .into_inner();
    let declaration = clients
        .explore
        .get_declaration(pb::GetDeclarationRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            declaration_name: "PublicRecord".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("read declaration")
        .into_inner();
    let descriptors = clients
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            at: Some(vref_branch("main")),
        })
        .await
        .expect("read descriptors")
        .into_inner();
    let dependencies = clients
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            at: Some(vref_branch("main")),
            transitive: false,
        })
        .await
        .expect("list dependencies")
        .into_inner();
    let search = clients
        .explore
        .search(pb::SearchRequest {
            query: "Public".into(),
            project: "acme".into(),
            repo: "target".into(),
            kind: pb::DeclKind::Unspecified as i32,
            limit: 50,
            at: Some(vref_branch("main")),
        })
        .await
        .expect("search")
        .into_inner();

    // Assert
    assert_eq!(source.at_commit, created.new_commit);
    assert_eq!(schemas.at_commit, created.new_commit);
    assert_eq!(declarations.at_commit, created.new_commit);
    assert_eq!(declaration.at_commit, created.new_commit);
    assert_eq!(descriptors.at_commit, created.new_commit);
    assert_eq!(dependencies.at_commit, created.new_commit);
    assert_eq!(search.at_commit, created.new_commit);
    assert_eq!(schemas.schemas.len(), 1);
    assert_eq!(declarations.declarations.len(), 1);
    assert!(String::from_utf8(source.source)
        .expect("UTF-8 source")
        .contains("message PublicRecord"));
    assert_eq!(declaration.summary.expect("summary").name, "PublicRecord");
    assert!(String::from_utf8(declaration.detail)
        .expect("UTF-8 detail")
        .contains("message PublicRecord"));
    assert!(!descriptors.descriptor_bytes.is_empty());
    assert!(dependencies.dependencies.is_empty());
    assert_eq!(search.results.len(), 1);
}

#[tokio::test]
async fn raw_commit_references_reject_a_commit_owned_by_another_repository() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let source = create_schema(
        &mut clients.schema,
        "acme",
        "source",
        "main",
        "shared.proto",
        pb::SchemaFormat::Protobuf,
        SOURCE_SCHEMA,
        "foreign-source-create",
    )
    .await;
    create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "main",
        "shared.proto",
        pb::SchemaFormat::Protobuf,
        TARGET_SCHEMA,
        "foreign-target-create",
    )
    .await;

    // Act
    let source_error = clients
        .explore
        .get_schema_source(pb::GetSchemaSourceRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            at: Some(vref_commit(&source.new_commit)),
        })
        .await
        .expect_err("foreign source commit must be rejected");
    let codegen_error = clients
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            at: Some(vref_commit(&source.new_commit)),
        })
        .await
        .expect_err("foreign descriptor commit must be rejected");
    let commit_error = clients
        .refs
        .get_commit(pb::GetCommitRequest {
            project: "acme".into(),
            repo: "target".into(),
            commit: source.new_commit.clone(),
        })
        .await
        .expect_err("foreign commit lookup must be rejected");
    let diff_error = clients
        .refs
        .diff(pb::DiffRequest {
            project: "acme".into(),
            repo: "target".into(),
            base: Some(vref_branch("main")),
            head: Some(vref_commit(&source.new_commit)),
            schema_path: String::new(),
        })
        .await
        .expect_err("foreign diff endpoint must be rejected");
    let branch_error = clients
        .refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "target".into(),
            name: "smuggled".into(),
            from: Some(vref_commit(&source.new_commit)),
        })
        .await
        .expect_err("foreign branch target must be rejected");
    let tag_error = clients
        .refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "target".into(),
            name: "smuggled".into(),
            target: Some(vref_commit(&source.new_commit)),
            message: String::new(),
        })
        .await
        .expect_err("foreign tag target must be rejected");

    // Assert
    for error in [
        &source_error,
        &codegen_error,
        &commit_error,
        &diff_error,
        &branch_error,
        &tag_error,
    ] {
        assert_eq!(error.code(), Code::FailedPrecondition, "{error:?}");
        assert!(
            error.message().contains("not retained by repository"),
            "{error:?}"
        );
    }
}

#[tokio::test]
async fn omitted_read_refs_use_the_repository_configured_default() {
    // Arrange
    let config = Config {
        repos: HashMap::from([(
            "acme/target".to_string(),
            RepoSection {
                default_bookmark: Some("trunk".to_string()),
                ..RepoSection::default()
            },
        )]),
        ..Config::default()
    };
    let url = start_server_with(config).await;
    let mut clients = clients(&url).await;
    let created = create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "trunk",
        "shared.proto",
        pb::SchemaFormat::Protobuf,
        TARGET_SCHEMA,
        "configured-default-read",
    )
    .await;

    // Act
    let schemas = clients
        .explore
        .list_schemas(pb::ListSchemasRequest {
            project: "acme".into(),
            repo: "target".into(),
            at: None,
        })
        .await
        .expect("list schemas at configured default")
        .into_inner();
    let descriptors = clients
        .codegen
        .get_descriptors(pb::GetDescriptorsRequest {
            project: "acme".into(),
            repo: "target".into(),
            schema_path: "shared.proto".into(),
            at: None,
        })
        .await
        .expect("generate at configured default")
        .into_inner();
    let history = clients
        .history
        .log(pb::LogRequest {
            project: "acme".into(),
            repo: "target".into(),
            at: None,
            limit: 0,
        })
        .await
        .expect("log configured default")
        .into_inner();
    let revision = clients
        .serving
        .resolve_revision(pb::ResolveRevisionRequest {
            parent: "projects/acme/repos/target".into(),
            at: None,
        })
        .await
        .expect("resolve configured default")
        .into_inner();

    // Assert
    assert_eq!(schemas.at_commit, created.new_commit);
    assert_eq!(descriptors.at_commit, created.new_commit);
    assert_eq!(history.at_commit, created.new_commit);
    assert_eq!(revision.commit_id, created.new_commit);
    assert_eq!(revision.resolved_from, "branch:trunk");
}

#[tokio::test]
async fn omitted_branch_and_tag_targets_use_the_repository_configured_default() {
    // Arrange
    let config = Config {
        repos: HashMap::from([(
            "acme/target".to_string(),
            RepoSection {
                default_bookmark: Some("trunk".to_string()),
                ..RepoSection::default()
            },
        )]),
        ..Config::default()
    };
    let url = start_server_with(config).await;
    let mut clients = clients(&url).await;
    let created = create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "trunk",
        "shared.proto",
        pb::SchemaFormat::Protobuf,
        TARGET_SCHEMA,
        "configured-default-write",
    )
    .await;

    // Act
    let branch = clients
        .refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "target".into(),
            name: "feature".into(),
            from: None,
        })
        .await
        .expect("branch from configured default")
        .into_inner();
    let tag = clients
        .refs
        .create_tag(pb::CreateTagRequest {
            project: "acme".into(),
            repo: "target".into(),
            name: "v1".into(),
            target: None,
            message: String::new(),
        })
        .await
        .expect("tag configured default")
        .into_inner();

    // Assert
    assert_eq!(
        branch.branch.expect("branch").head_commit,
        created.new_commit
    );
    assert_eq!(tag.tag.expect("tag").commit_hash, created.new_commit);
}

#[tokio::test]
async fn omitted_change_target_uses_the_repository_configured_default() {
    // Arrange
    let config = Config {
        repos: HashMap::from([(
            "acme/target".to_string(),
            RepoSection {
                default_bookmark: Some("trunk".to_string()),
                ..RepoSection::default()
            },
        )]),
        ..Config::default()
    };
    let url = start_server_with(config).await;
    let mut clients = clients(&url).await;

    // Act
    let change = clients
        .change
        .create_change(pb::CreateChangeRequest {
            parent: "projects/acme/repos/target".into(),
            change: Some(pb::ChangeRecord {
                title: "Record intent at the repository default".into(),
                ..pb::ChangeRecord::default()
            }),
            change_id: "configured-default-change".into(),
        })
        .await
        .expect("create change at configured default")
        .into_inner();

    // Assert
    assert_eq!(change.target_bookmark, "trunk");
}

#[tokio::test]
async fn repository_diff_reports_exact_snapshots_and_added_schema_declarations() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let base = create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "main",
        "base.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Base {}",
        "diff-base",
    )
    .await;
    clients
        .refs
        .create_branch(pb::CreateBranchRequest {
            project: "acme".into(),
            repo: "target".into(),
            name: "feature".into(),
            from: Some(vref_branch("main")),
        })
        .await
        .expect("create feature branch");
    let head = create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "feature",
        "added.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Added {}",
        "diff-added",
    )
    .await;

    // Act
    let diff = clients
        .refs
        .diff(pb::DiffRequest {
            project: "acme".into(),
            repo: "target".into(),
            base: Some(vref_branch("main")),
            head: Some(vref_branch("feature")),
            schema_path: String::new(),
        })
        .await
        .expect("diff repository snapshots")
        .into_inner();

    // Assert
    assert_eq!(diff.base_commit, base.new_commit);
    assert_eq!(diff.head_commit, head.new_commit);
    assert_eq!(diff.schema_diffs.len(), 1);
    assert_eq!(diff.schema_diffs[0].schema_path, "added.proto");
    assert_eq!(diff.schema_diffs[0].changes.len(), 1);
    assert_eq!(diff.schema_diffs[0].changes[0].change_type, "added");
    assert_eq!(diff.schema_diffs[0].changes[0].decl_name, "Added");
}

#[tokio::test]
async fn commit_stream_honors_stop_and_schema_filter_and_reports_snapshot_metadata() {
    // Arrange
    let url = start_server().await;
    let mut clients = clients(&url).await;
    let base = create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "main",
        "base.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Base {}",
        "stream-base",
    )
    .await;
    let head = create_schema(
        &mut clients.schema,
        "acme",
        "target",
        "main",
        "added.proto",
        pb::SchemaFormat::Protobuf,
        "syntax=\"proto3\"; message Added {}",
        "stream-added",
    )
    .await;

    // Act
    let response = clients
        .refs
        .list_commits(pb::ListCommitsRequest {
            project: "acme".into(),
            repo: "target".into(),
            from: Some(vref_branch("main")),
            stop_at_commit: base.new_commit,
            schema_path: "added.proto".into(),
        })
        .await
        .expect("stream filtered commit range");
    let at_commit = response
        .metadata()
        .get("x-schemahub-at-commit")
        .expect("snapshot metadata")
        .to_str()
        .expect("ASCII commit")
        .to_string();
    let entries: Vec<_> = response
        .into_inner()
        .map(|entry| entry.expect("commit entry"))
        .collect()
        .await;

    // Assert
    assert_eq!(at_commit, head.new_commit);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hash, head.new_commit);
}
