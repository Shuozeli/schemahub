use anyhow::Context;
use tonic::transport::Channel;

pub async fn build_channel(server: &str) -> anyhow::Result<Channel> {
    Channel::from_shared(server.to_string())
        .context("invalid server URL")?
        .connect()
        .await
        .context("connecting to server")
}

