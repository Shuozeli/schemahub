use std::collections::HashMap;
use std::sync::Arc;

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::{
    project_service_client::ProjectServiceClient, ref_service_client::RefServiceClient,
    schema_service_client::SchemaServiceClient,
};
use schemahub_jj::{MemoryObjectDb, ObjectDb};
use schemahub_server::config::{AuthConfig, Config, TokenIdentity};
use schemahub_server::{build_core, build_router};
use schemahub_types::IdentityKind;
use tokio::net::TcpListener;
use tonic::metadata::MetadataValue;
use tonic::Request;

const OWNER_TOKEN: &str = "owner-token";

async fn start_authenticated_server() -> String {
    let auth_dir = tempfile::tempdir().expect("authentication tempdir");
    let config = Config {
        auth: AuthConfig {
            data_dir: auth_dir.path().to_string_lossy().to_string(),
            tokens: HashMap::from([(
                OWNER_TOKEN.to_string(),
                TokenIdentity {
                    id: "alice".to_string(),
                    display: Some("Alice".to_string()),
                    kind: IdentityKind::Human,
                    delegated_by: None,
                },
            )]),
            jwt: None,
        },
        ..Config::default()
    };
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db, &config);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        build_router(core, "memory")
            .serve_with_incoming(incoming)
            .await
            .expect("serve CLI bearer test");
    });
    format!("http://{address}")
}

fn with_token<T>(body: T) -> Request<T> {
    let mut request = Request::new(body);
    let value: MetadataValue<_> = format!("Bearer {OWNER_TOKEN}")
        .parse()
        .expect("valid bearer metadata");
    request.metadata_mut().insert("authorization", value);
    request
}

async fn seed_repository(server: &str) {
    let mut client = ProjectServiceClient::connect(server.to_string())
        .await
        .expect("connect project client");
    client
        .create_project(with_token(pb::CreateProjectRequest {
            name: "acme".to_string(),
            is_public: false,
        }))
        .await
        .expect("create test project");
    client
        .create_repo(with_token(pb::CreateRepoRequest {
            project: "acme".to_string(),
            name: "api".to_string(),
            default_branch: "main".to_string(),
            compatibility_direction: pb::CompatibilityDirection::Full as i32,
            protected_branches: vec!["main".to_string()],
            review_policy: None,
            serving_policy: None,
        }))
        .await
        .expect("create test repository");
}

async fn seed_ref_namespace(server: &str) {
    seed_repository(server).await;
    let mut schemas = SchemaServiceClient::connect(server.to_string())
        .await
        .expect("connect schema client");
    schemas
        .create_schema(with_token(pb::CreateSchemaRequest {
            project: "acme".to_string(),
            repo: "api".to_string(),
            branch: "main".to_string(),
            schema_name: "ping.proto".to_string(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: "syntax = \"proto3\";\nmessage Ping {}\n".to_string(),
            base_revision: String::new(),
            idempotency_key: "seed-ref-namespace".to_string(),
        }))
        .await
        .expect("create seed schema");
    let mut refs = RefServiceClient::connect(server.to_string())
        .await
        .expect("connect ref client");
    for name in ["feature/b", "preview/a", "feature/a"] {
        refs.create_branch(with_token(pb::CreateBranchRequest {
            project: "acme".to_string(),
            repo: "api".to_string(),
            name: name.to_string(),
            from: Some(pb::VersionRef {
                r#ref: Some(pb::version_ref::Ref::Branch("main".to_string())),
            }),
        }))
        .await
        .expect("create branch");
    }
    for name in ["release/2", "preview/1", "release/1"] {
        refs.create_tag(with_token(pb::CreateTagRequest {
            project: "acme".to_string(),
            repo: "api".to_string(),
            name: name.to_string(),
            target: Some(pb::VersionRef {
                r#ref: Some(pb::version_ref::Ref::Branch("main".to_string())),
            }),
            message: String::new(),
        }))
        .await
        .expect("create tag");
    }
}

fn cli() -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_schemahub"));
    command
        .env_remove("SCHEMAHUB_SERVER")
        .env_remove("SCHEMAHUB_TOKEN");
    command
}

#[tokio::test]
async fn repo_init_forwards_the_configured_bearer_token() {
    // Arrange
    let server = start_authenticated_server().await;
    let mut command = cli();
    command.args([
        "--server",
        &server,
        "--token",
        OWNER_TOKEN,
        "repo",
        "init",
        "cli/api",
    ]);

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(
        output.status.success(),
        "repo init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Created project 'cli'."));
    assert!(stdout.contains("Created repo 'cli/api'."));
}

#[tokio::test]
async fn schema_create_forwards_the_configured_bearer_token() {
    // Arrange
    let server = start_authenticated_server().await;
    seed_repository(&server).await;
    let schema_dir = tempfile::tempdir().expect("schema tempdir");
    let schema_path = schema_dir.path().join("ping.proto");
    std::fs::write(&schema_path, "syntax = \"proto3\";\nmessage Ping {}\n")
        .expect("write test schema");
    let mut command = cli();
    command
        .arg("--server")
        .arg(&server)
        .arg("--token")
        .arg(OWNER_TOKEN)
        .arg("schema")
        .arg("create")
        .arg(&schema_path)
        .args(["--project", "acme", "--repo", "api"]);

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(
        output.status.success(),
        "schema create failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Created commit:"));
}

#[tokio::test]
async fn project_audit_emits_typed_agent_json() {
    // Arrange
    let server = start_authenticated_server().await;
    seed_repository(&server).await;
    let mut command = cli();
    command.args([
        "--server",
        &server,
        "--token",
        OWNER_TOKEN,
        "project",
        "audit",
        "acme",
        "--json",
    ]);

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(
        output.status.success(),
        "project audit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse project audit JSON");
    let events = events.as_array().expect("audit output is an array");
    assert_eq!(events.len(), 2);
    assert!(events
        .iter()
        .all(|event| event["actor"] == serde_json::json!("alice")));
    assert!(events.iter().any(|event| {
        event["action"] == serde_json::json!("CONTROL_PLANE_AUDIT_ACTION_PROJECT_CREATED")
            && event["after"]["resource_type"] == serde_json::json!("project")
    }));
    assert!(events.iter().any(|event| {
        event["action"] == serde_json::json!("CONTROL_PLANE_AUDIT_ACTION_REPOSITORY_CREATED")
            && event["after"]["resource_type"] == serde_json::json!("repository")
    }));
}

#[tokio::test]
async fn project_member_list_follows_pages_and_emits_stable_json() {
    // Arrange
    let server = start_authenticated_server().await;
    seed_repository(&server).await;
    let mut client = ProjectServiceClient::connect(server.clone())
        .await
        .expect("connect project client");
    client
        .add_member(with_token(pb::AddMemberRequest {
            project: "acme".to_string(),
            identity: "schema-agent".to_string(),
            role: pb::Role::Writer as i32,
        }))
        .await
        .expect("add agent");
    let mut command = cli();
    command.args([
        "--server",
        &server,
        "--token",
        OWNER_TOKEN,
        "project",
        "member",
        "list",
        "acme",
        "--page-size",
        "1",
        "--json",
    ]);

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(
        output.status.success(),
        "project member list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let members: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse member JSON");
    assert_eq!(
        members,
        serde_json::json!([
            {"identity": "alice", "role": "ROLE_OWNER"},
            {"identity": "schema-agent", "role": "ROLE_WRITER"}
        ])
    );
}

#[tokio::test]
async fn branch_list_follows_all_prefix_scoped_pages() {
    // Arrange
    let server = start_authenticated_server().await;
    seed_ref_namespace(&server).await;
    let mut command = cli();
    command.args([
        "--server",
        &server,
        "--token",
        OWNER_TOKEN,
        "branch",
        "list",
        "acme/api",
        "--prefix",
        "feature/",
        "--page-size",
        "1",
    ]);

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(
        output.status.success(),
        "branch list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.find("feature/a").expect("first branch");
    let second = stdout.find("feature/b").expect("second branch");
    assert!(first < second, "branches are not ordered: {stdout}");
    assert!(!stdout.contains("preview/a"));
}

#[tokio::test]
async fn tag_list_follows_all_prefix_scoped_pages() {
    // Arrange
    let server = start_authenticated_server().await;
    seed_ref_namespace(&server).await;
    let mut command = cli();
    command.args([
        "--server",
        &server,
        "--token",
        OWNER_TOKEN,
        "tag",
        "list",
        "acme/api",
        "--prefix",
        "release/",
        "--page-size",
        "1",
    ]);

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(
        output.status.success(),
        "tag list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.find("release/1").expect("first tag");
    let second = stdout.find("release/2").expect("second tag");
    assert!(first < second, "tags are not ordered: {stdout}");
    assert!(!stdout.contains("preview/1"));
}

#[tokio::test]
async fn malformed_config_fails_before_a_server_override_is_used() {
    // Arrange
    let home = tempfile::tempdir().expect("CLI home tempdir");
    let config_dir = home.path().join(".schemahub");
    std::fs::create_dir_all(&config_dir).expect("create config directory");
    std::fs::write(config_dir.join("config"), "[default\nserver = false")
        .expect("write malformed config");
    let mut command = cli();
    command.env("HOME", home.path()).args([
        "--server",
        "https://schemahub.example.com",
        "capabilities",
    ]);

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("parsing CLI config"), "stderr: {stderr}");
}

#[tokio::test]
async fn missing_server_has_no_process_level_loopback_fallback() {
    // Arrange
    let home = tempfile::tempdir().expect("CLI home tempdir");
    let mut command = cli();
    command.env("HOME", home.path()).arg("capabilities");

    // Act
    let output = command.output().await.expect("run schemahub CLI");

    // Assert
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("server address is required"),
        "stderr: {stderr}"
    );
}
