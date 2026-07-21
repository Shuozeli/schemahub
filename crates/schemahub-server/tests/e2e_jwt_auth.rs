//! End-to-end production JWT authentication through the public gRPC surface.

use std::sync::Arc;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use schemahub_api::schemahub_v1 as pb;
use schemahub_jj::{MemoryObjectDb, ObjectDb};
use schemahub_server::config::{AuthConfig, Config, JwtAuthConfig};
use schemahub_server::jwt_auth::{JwtAuthRuntime, JwtClock};
use schemahub_server::{build_core_with_authn, build_router};
use serde::Serialize;
use tokio::net::TcpListener;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::Request;

const TEST_PRIVATE_KEY_DER: &[u8] = &[
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    0x6a, 0xc3, 0xfd, 0xee, 0xee, 0x29, 0x8a, 0x92, 0x63, 0x8b, 0x70, 0x0c, 0x4b, 0x11, 0x7c, 0xc3,
    0x2e, 0x2d, 0x2a, 0xce, 0x0d, 0xfd, 0x78, 0x76, 0x94, 0xe2, 0x4c, 0xae, 0x8a, 0xd5, 0x82, 0x34,
];
const TEST_PUBLIC_KEY_X: &str = "2-Jj2UvNCvQiUPNYRgSi0cJSPiJI6Rs6D0UTeEpQVj8";

#[derive(Debug)]
struct FixedClock(u64);

impl JwtClock for FixedClock {
    fn now_unix_seconds(&self) -> anyhow::Result<u64> {
        Ok(self.0)
    }
}

#[derive(Serialize)]
struct Claims {
    iss: &'static str,
    sub: &'static str,
    aud: &'static str,
    exp: u64,
    nbf: u64,
    iat: u64,
    name: &'static str,
}

fn jwt_config(jwks_file: String) -> JwtAuthConfig {
    JwtAuthConfig {
        issuer: "https://identity.example.test".to_string(),
        audiences: vec!["schemahub".to_string()],
        algorithms: vec!["EdDSA".to_string()],
        token_type: "at+jwt".to_string(),
        identity_id_prefix: "oidc:".to_string(),
        jwks_url: None,
        jwks_file: Some(jwks_file),
        clock_skew_seconds: 5,
        refresh_interval_seconds: 60,
        max_stale_seconds: 600,
        request_timeout_seconds: 5,
        max_token_bytes: 8_192,
        max_jwks_bytes: 65_536,
    }
}

fn signed_token() -> String {
    let mut header = Header::new(Algorithm::EdDSA);
    header.typ = Some("at+jwt".to_string());
    header.kid = Some("acceptance-key".to_string());
    encode(
        &header,
        &Claims {
            iss: "https://identity.example.test",
            sub: "alice",
            aud: "schemahub",
            exp: 2_000,
            nbf: 900,
            iat: 900,
            name: "Alice",
        },
        &EncodingKey::from_ed_der(TEST_PRIVATE_KEY_DER),
    )
    .expect("sign acceptance token")
}

#[tokio::test]
async fn jwt_subject_becomes_the_project_owner_through_grpc() {
    // Arrange
    let temp = tempfile::tempdir().expect("JWT tempdir");
    let jwks_path = temp.path().join("jwks.json");
    std::fs::write(
        &jwks_path,
        serde_json::json!({
            "keys": [{
                "kty": "OKP",
                "use": "sig",
                "key_ops": ["verify"],
                "crv": "Ed25519",
                "x": TEST_PUBLIC_KEY_X,
                "kid": "acceptance-key",
                "alg": "EdDSA"
            }]
        })
        .to_string(),
    )
    .expect("write JWKS fixture");
    let jwt = jwt_config(jwks_path.to_string_lossy().to_string());
    let config = Config {
        auth: AuthConfig {
            data_dir: temp.path().join("access").to_string_lossy().to_string(),
            tokens: Default::default(),
            jwt: Some(jwt.clone()),
        },
        ..Config::default()
    };
    let runtime = JwtAuthRuntime::initialize_with_clock(&jwt, Arc::new(FixedClock(1_000)))
        .await
        .expect("initialize JWT runtime");
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core_with_authn(db, &config, runtime.provider());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind acceptance listener");
    let addr = listener.local_addr().expect("acceptance address");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        build_router(core, "memory")
            .serve_with_incoming(incoming)
            .await
            .expect("serve acceptance API");
    });
    let channel = Channel::from_shared(format!("http://{addr}"))
        .expect("acceptance endpoint")
        .connect()
        .await
        .expect("connect acceptance client");
    let mut client = pb::project_service_client::ProjectServiceClient::new(channel);
    let mut request = Request::new(pb::CreateProjectRequest {
        name: "acme".to_string(),
        is_public: false,
    });
    let authorization: MetadataValue<_> = format!("Bearer {}", signed_token())
        .parse()
        .expect("authorization metadata");
    request
        .metadata_mut()
        .insert("authorization", authorization);

    // Act
    let project = client
        .create_project(request)
        .await
        .expect("JWT-authenticated project create")
        .into_inner()
        .project
        .expect("project resource");

    // Assert
    assert_eq!(project.name, "acme");
    assert_eq!(project.owner, "oidc:alice");
}
