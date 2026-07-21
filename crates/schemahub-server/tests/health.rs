//! Standard gRPC health-check contract for deployments without the HTTP BFF.

use std::sync::Arc;

use schemahub_jj::{MemoryObjectDb, ObjectDb};
use schemahub_server::{build_core, build_router, config::Config};
use tokio::net::TcpListener;
use tonic::transport::Channel;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;

#[tokio::test]
async fn grpc_overall_health_is_registered_as_serving() {
    // Arrange
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let core = build_core(db, &Config::default());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind health test listener");
    let addr = listener.local_addr().expect("health test address");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        build_router(core, "memory")
            .serve_with_incoming(incoming)
            .await
            .expect("serve health test");
    });
    let channel = Channel::from_shared(format!("http://{addr}"))
        .expect("health endpoint")
        .connect()
        .await
        .expect("connect health client");
    let mut client = HealthClient::new(channel);

    // Act
    let response = client
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await
        .expect("check overall gRPC health")
        .into_inner();

    // Assert
    assert_eq!(response.status, ServingStatus::Serving as i32);
}
