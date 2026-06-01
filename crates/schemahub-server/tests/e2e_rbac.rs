//! E2E tests for the RBAC layer (design.md §6). Each test spins up an
//! in-process server with a tailored `Config` (via the shared
//! `start_server_with` harness) and exercises the policy surface end-to-end:
//! anonymous reads on public projects, role-gated writes, `--force` requires
//! Maintainer+, Owner-only member management with the zero-Owner guard,
//! `CreateProject` makes the caller the Owner, `ListProjects` filtering.
//!
//! All cases follow Arrange-Act-Assert. Token is sent via the standard
//! `Authorization: Bearer <token>` metadata header — the same surface the
//! `--token` CLI flag uses.

mod common;

use std::collections::HashMap;

use common::*;
use schemahub_api::schemahub_v1 as pb;
use schemahub_server::config::{AuthConfig, Config, ProjectSection, TokenIdentity};
use tonic::metadata::MetadataValue;
use tonic::Request;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build a `Config` with RBAC turned on. Five bearer tokens with stable
/// identity ids for the test cases.
fn config_with_auth() -> Config {
    Config {
        auth: AuthConfig {
            data_dir: tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .to_string(),
            tokens: HashMap::from([
                (
                    "owner-token".to_string(),
                    TokenIdentity {
                        id: "alice".to_string(),
                        display: Some("Alice".to_string()),
                    },
                ),
                (
                    "maintainer-token".to_string(),
                    TokenIdentity {
                        id: "maeve".to_string(),
                        display: None,
                    },
                ),
                (
                    "writer-token".to_string(),
                    TokenIdentity {
                        id: "wendy".to_string(),
                        display: None,
                    },
                ),
                (
                    "reader-token".to_string(),
                    TokenIdentity {
                        id: "ron".to_string(),
                        display: None,
                    },
                ),
                (
                    "stranger-token".to_string(),
                    TokenIdentity {
                        id: "stranger".to_string(),
                        display: None,
                    },
                ),
            ]),
        },
        ..Default::default()
    }
}

/// Add a project bootstrap to a config (visibility + an Owner identity).
fn with_project(mut cfg: Config, name: &str, visibility: &str, owner_id: &str) -> Config {
    cfg.projects.insert(
        name.to_string(),
        ProjectSection {
            visibility: Some(visibility.to_string()),
            owners: vec![owner_id.to_string()],
            members: HashMap::new(),
        },
    );
    cfg
}

/// Wrap a request with a `Authorization: Bearer <token>` header.
fn with_token<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let header: MetadataValue<_> = format!("Bearer {}", token).parse().unwrap();
    req.metadata_mut().insert("authorization", header);
    req
}

// ── Case 1: no `[auth]` → anonymous still works ───────────────────────────────

#[tokio::test]
async fn case1_noauth_keeps_anonymous_access() {
    // Arrange: default config has no [auth] / [projects] — Noop providers ship.
    let url = start_server_with(Config::default()).await;
    let mut c = clients(&url).await;

    // Act: a vanilla schema create with no token at all.
    let resp = c
        .schema
        .create_schema(pb::CreateSchemaRequest {
            project: "demo".into(),
            repo: "api".into(),
            branch: "main".into(),
            schema_name: "ping.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: "syntax = \"proto3\";\nmessage Ping {}\n".into(),
            base_revision: String::new(),
            idempotency_key: "k1".into(),
        })
        .await;

    // Assert: succeeds (today's behavior preserved when auth is off).
    assert!(resp.is_ok(), "got: {:?}", resp.err());
}

// ── Case 2: public project, anonymous read OK, anonymous write denied ────────

#[tokio::test]
async fn case2_public_project_anonymous_can_read_not_write() {
    // Arrange
    let cfg = with_project(config_with_auth(), "acme", "public", "alice");
    let url = start_server_with(cfg).await;
    let mut c = clients(&url).await;

    // Seed a schema as the Owner so there's something to read.
    let seed_req = with_token(
        Request::new(pb::CreateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "feature".into(), // unprotected — bypass compat gate
            schema_name: "ping.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: "syntax = \"proto3\";\nmessage Ping {}\n".into(),
            base_revision: String::new(),
            idempotency_key: "seed".into(),
        }),
        "owner-token",
    );
    c.schema.create_schema(seed_req).await.expect("seed write");

    // Act: anonymous read.
    let read = c
        .explore
        .list_declarations(pb::ListDeclarationsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "ping.proto".into(),
            at: Some(vref_branch("feature")),
            kind_filter: pb::DeclKind::Unspecified as i32,
        })
        .await;

    // Assert: read succeeds.
    assert!(
        read.is_ok(),
        "anonymous read on public project: {:?}",
        read.err()
    );

    // Act: anonymous write attempt.
    let write = c
        .schema
        .create_schema(pb::CreateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "feature".into(),
            schema_name: "pong.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: "syntax = \"proto3\";\nmessage Pong {}\n".into(),
            base_revision: String::new(),
            idempotency_key: "anon-write".into(),
        })
        .await;

    // Assert: write rejected.
    let err = write.expect_err("anonymous write should be denied");
    assert_eq!(err.code(), tonic::Code::PermissionDenied, "got: {err:?}");
}

// ── Case 3: private project, role-gated reads + writes ───────────────────────

#[tokio::test]
async fn case3_private_project_roles_gate_read_and_write() {
    // Arrange
    let cfg = with_project(config_with_auth(), "acme", "private", "alice");
    let url = start_server_with(cfg).await;
    let mut c = clients(&url).await;

    // Owner grants Reader to ron and Writer to wendy.
    let add_reader = with_token(
        Request::new(pb::AddMemberRequest {
            project: "acme".into(),
            identity: "ron".into(),
            role: pb::Role::Reader as i32,
        }),
        "owner-token",
    );
    c.project.add_member(add_reader).await.expect("add reader");
    let add_writer = with_token(
        Request::new(pb::AddMemberRequest {
            project: "acme".into(),
            identity: "wendy".into(),
            role: pb::Role::Writer as i32,
        }),
        "owner-token",
    );
    c.project.add_member(add_writer).await.expect("add writer");

    // Seed a schema as Owner.
    let seed = with_token(
        Request::new(pb::CreateSchemaRequest {
            project: "acme".into(),
            repo: "core".into(),
            branch: "feature".into(),
            schema_name: "ping.proto".into(),
            format: pb::SchemaFormat::Protobuf as i32,
            source: "syntax = \"proto3\";\nmessage Ping {}\n".into(),
            base_revision: String::new(),
            idempotency_key: "seed".into(),
        }),
        "owner-token",
    );
    c.schema.create_schema(seed).await.expect("seed write");

    // Act+Assert: anonymous read denied.
    let anon = c
        .explore
        .list_declarations(pb::ListDeclarationsRequest {
            project: "acme".into(),
            repo: "core".into(),
            schema_path: "ping.proto".into(),
            at: Some(vref_branch("feature")),
            kind_filter: pb::DeclKind::Unspecified as i32,
        })
        .await
        .expect_err("anon on private read");
    assert_eq!(anon.code(), tonic::Code::PermissionDenied);

    // Act+Assert: Reader read OK.
    let reader_read = c
        .explore
        .list_declarations(with_token(
            Request::new(pb::ListDeclarationsRequest {
                project: "acme".into(),
                repo: "core".into(),
                schema_path: "ping.proto".into(),
                at: Some(vref_branch("feature")),
                kind_filter: pb::DeclKind::Unspecified as i32,
            }),
            "reader-token",
        ))
        .await;
    assert!(reader_read.is_ok(), "reader read: {:?}", reader_read.err());

    // Act+Assert: Reader write denied.
    let reader_write = c
        .schema
        .create_schema(with_token(
            Request::new(pb::CreateSchemaRequest {
                project: "acme".into(),
                repo: "core".into(),
                branch: "feature".into(),
                schema_name: "evil.proto".into(),
                format: pb::SchemaFormat::Protobuf as i32,
                source: "syntax = \"proto3\";\nmessage Evil {}\n".into(),
                base_revision: String::new(),
                idempotency_key: "rdr-write".into(),
            }),
            "reader-token",
        ))
        .await
        .expect_err("reader write denied");
    assert_eq!(reader_write.code(), tonic::Code::PermissionDenied);

    // Act+Assert: Writer write OK.
    let writer_write = c
        .schema
        .create_schema(with_token(
            Request::new(pb::CreateSchemaRequest {
                project: "acme".into(),
                repo: "core".into(),
                branch: "feature".into(),
                schema_name: "pong.proto".into(),
                format: pb::SchemaFormat::Protobuf as i32,
                source: "syntax = \"proto3\";\nmessage Pong {}\n".into(),
                base_revision: String::new(),
                idempotency_key: "wri-write".into(),
            }),
            "writer-token",
        ))
        .await;
    assert!(
        writer_write.is_ok(),
        "writer write: {:?}",
        writer_write.err()
    );
}

// ── Case 4: --force on a protected branch requires Maintainer+ ───────────────

#[tokio::test]
async fn case4_force_on_protected_branch_requires_maintainer() {
    // Arrange: protect `main` (default repo config protects `main`); seed a
    // Maintainer + Writer; both are members of the private project.
    let cfg = with_project(config_with_auth(), "acme", "private", "alice");
    let url = start_server_with(cfg).await;
    let mut c = clients(&url).await;

    c.project
        .add_member(with_token(
            Request::new(pb::AddMemberRequest {
                project: "acme".into(),
                identity: "wendy".into(),
                role: pb::Role::Writer as i32,
            }),
            "owner-token",
        ))
        .await
        .expect("add writer");
    c.project
        .add_member(with_token(
            Request::new(pb::AddMemberRequest {
                project: "acme".into(),
                identity: "maeve".into(),
                role: pb::Role::Maintainer as i32,
            }),
            "owner-token",
        ))
        .await
        .expect("add maintainer");

    // Seed a schema on main as Owner first (so there's a base to mutate).
    c.schema
        .create_schema(with_token(
            Request::new(pb::CreateSchemaRequest {
                project: "acme".into(),
                repo: "core".into(),
                branch: "main".into(),
                schema_name: "ping.proto".into(),
                format: pb::SchemaFormat::Protobuf as i32,
                source: "syntax = \"proto3\";\nmessage Ping {\n  string m = 1;\n}\n".into(),
                base_revision: String::new(),
                idempotency_key: "seed".into(),
            }),
            "owner-token",
        ))
        .await
        .expect("seed main");

    // Act+Assert: Writer with --force=true on `main` → denied (needs Maintainer).
    let writer_force = c
        .schema
        .apply_mutation(with_token(
            Request::new(pb::ApplyMutationRequest {
                project: "acme".into(),
                repo: "core".into(),
                branch: "main".into(),
                operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                    pb::ProtobufMutation {
                        schema_path: "ping.proto".into(),
                        operation: Some(pb::protobuf_mutation::Operation::RenameField(
                            pb::ProtoRenameField {
                                message_name: "Ping".into(),
                                old_field_name: "m".into(),
                                new_field_name: "message".into(),
                            },
                        )),
                    },
                )),
                force: true,
                base_revision: String::new(),
                idempotency_key: "force-writer".into(),
            }),
            "writer-token",
        ))
        .await
        .expect_err("writer with --force on protected branch");
    assert_eq!(writer_force.code(), tonic::Code::PermissionDenied);

    // Act+Assert: Maintainer with --force on `main` → succeeds.
    let maint_force = c
        .schema
        .apply_mutation(with_token(
            Request::new(pb::ApplyMutationRequest {
                project: "acme".into(),
                repo: "core".into(),
                branch: "main".into(),
                operation: Some(pb::apply_mutation_request::Operation::ProtobufOp(
                    pb::ProtobufMutation {
                        schema_path: "ping.proto".into(),
                        operation: Some(pb::protobuf_mutation::Operation::RenameField(
                            pb::ProtoRenameField {
                                message_name: "Ping".into(),
                                old_field_name: "m".into(),
                                new_field_name: "message".into(),
                            },
                        )),
                    },
                )),
                force: true,
                base_revision: String::new(),
                idempotency_key: "force-maint".into(),
            }),
            "maintainer-token",
        ))
        .await;
    assert!(
        maint_force.is_ok(),
        "maintainer force: {:?}",
        maint_force.err()
    );
}

// ── Case 5: Owner-only member management; zero-Owner guard ───────────────────

#[tokio::test]
async fn case5_member_management_is_owner_only_with_zero_owner_guard() {
    // Arrange: single Owner = alice; Writer = wendy.
    let cfg = with_project(config_with_auth(), "acme", "private", "alice");
    let url = start_server_with(cfg).await;
    let mut c = clients(&url).await;

    c.project
        .add_member(with_token(
            Request::new(pb::AddMemberRequest {
                project: "acme".into(),
                identity: "wendy".into(),
                role: pb::Role::Writer as i32,
            }),
            "owner-token",
        ))
        .await
        .expect("seed writer");

    // Act+Assert: non-Owner trying AddMember → denied.
    let non_owner = c
        .project
        .add_member(with_token(
            Request::new(pb::AddMemberRequest {
                project: "acme".into(),
                identity: "stranger".into(),
                role: pb::Role::Reader as i32,
            }),
            "writer-token",
        ))
        .await
        .expect_err("writer can't add members");
    assert_eq!(non_owner.code(), tonic::Code::PermissionDenied);

    // Act+Assert: removing the only Owner → denied (zero-Owner guard).
    let solo_remove = c
        .project
        .remove_member(with_token(
            Request::new(pb::RemoveMemberRequest {
                project: "acme".into(),
                identity: "alice".into(),
            }),
            "owner-token",
        ))
        .await
        .expect_err("removing last owner");
    assert_eq!(solo_remove.code(), tonic::Code::PermissionDenied);

    // Act+Assert: downgrading the only Owner → denied.
    let downgrade = c
        .project
        .update_member_role(with_token(
            Request::new(pb::UpdateMemberRoleRequest {
                project: "acme".into(),
                identity: "alice".into(),
                new_role: pb::Role::Writer as i32,
            }),
            "owner-token",
        ))
        .await
        .expect_err("downgrading last owner");
    assert_eq!(downgrade.code(), tonic::Code::PermissionDenied);

    // Arrange: add a second Owner; now removal of alice is allowed.
    c.project
        .add_member(with_token(
            Request::new(pb::AddMemberRequest {
                project: "acme".into(),
                identity: "stranger".into(),
                role: pb::Role::Owner as i32,
            }),
            "owner-token",
        ))
        .await
        .expect("add second owner");

    // Act+Assert: remove alice now succeeds.
    let remove_alice = c
        .project
        .remove_member(with_token(
            Request::new(pb::RemoveMemberRequest {
                project: "acme".into(),
                identity: "alice".into(),
            }),
            "stranger-token",
        ))
        .await;
    assert!(
        remove_alice.is_ok(),
        "remove alice once 2 owners: {:?}",
        remove_alice.err()
    );
}

// ── Case 6: CreateProject makes the caller the Owner ────────────────────────

#[tokio::test]
async fn case6_create_project_makes_caller_the_owner() {
    // Arrange: auth on, no bootstrap project. Reader-token's identity = ron.
    let url = start_server_with(config_with_auth()).await;
    let mut c = clients(&url).await;

    // Act: ron creates a new project.
    c.project
        .create_project(with_token(
            Request::new(pb::CreateProjectRequest {
                name: "ron-proj".into(),
                is_public: false,
            }),
            "reader-token",
        ))
        .await
        .expect("create_project as ron");

    // Assert: ron can now add a member (Owner-only RPC).
    let add = c
        .project
        .add_member(with_token(
            Request::new(pb::AddMemberRequest {
                project: "ron-proj".into(),
                identity: "wendy".into(),
                role: pb::Role::Writer as i32,
            }),
            "reader-token",
        ))
        .await;
    assert!(add.is_ok(), "ron should be Owner: {:?}", add.err());

    // And anonymous cannot create projects.
    let anon = c
        .project
        .create_project(pb::CreateProjectRequest {
            name: "ghost".into(),
            is_public: true,
        })
        .await
        .expect_err("anon create_project");
    assert_eq!(anon.code(), tonic::Code::PermissionDenied);
}

// ── Case 7: ListProjects returns only what the caller can Read ───────────────

#[tokio::test]
async fn case7_list_projects_filters_by_visibility_and_membership() {
    // Arrange: 3 projects:
    //   - "public-proj": public, owner alice
    //   - "alice-only":  private, owner alice
    //   - "ron-priv":    private, owner ron (reader-token)
    let mut cfg = config_with_auth();
    cfg = with_project(cfg, "public-proj", "public", "alice");
    cfg = with_project(cfg, "alice-only", "private", "alice");
    cfg = with_project(cfg, "ron-priv", "private", "ron");
    let url = start_server_with(cfg).await;
    let mut c = clients(&url).await;

    // Act: anonymous list.
    let anon_list = c
        .project
        .list_projects(pb::ListProjectsRequest {
            name_prefix: String::new(),
        })
        .await
        .expect("anonymous list")
        .into_inner();

    // Assert: only the public project is visible.
    let anon_names: Vec<String> = anon_list.projects.iter().map(|p| p.name.clone()).collect();
    assert_eq!(anon_names, vec!["public-proj".to_string()]);

    // Act: ron lists.
    let ron_list = c
        .project
        .list_projects(with_token(
            Request::new(pb::ListProjectsRequest {
                name_prefix: String::new(),
            }),
            "reader-token",
        ))
        .await
        .expect("ron list")
        .into_inner();

    // Assert: ron sees public-proj + ron-priv (his own), NOT alice-only.
    let mut ron_names: Vec<String> = ron_list.projects.iter().map(|p| p.name.clone()).collect();
    ron_names.sort();
    assert_eq!(
        ron_names,
        vec!["public-proj".to_string(), "ron-priv".to_string()]
    );

    // Act: alice lists.
    let alice_list = c
        .project
        .list_projects(with_token(
            Request::new(pb::ListProjectsRequest {
                name_prefix: String::new(),
            }),
            "owner-token",
        ))
        .await
        .expect("alice list")
        .into_inner();

    // Assert: alice sees public-proj + alice-only, NOT ron-priv.
    let mut alice_names: Vec<String> = alice_list.projects.iter().map(|p| p.name.clone()).collect();
    alice_names.sort();
    assert_eq!(
        alice_names,
        vec!["alice-only".to_string(), "public-proj".to_string()]
    );
}
