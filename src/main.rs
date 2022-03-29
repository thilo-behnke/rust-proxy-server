use crate::proxy_server::proxy_server::ProxyServer;

mod proxy_server;

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ProxyServer::create().run().await?;
    Ok(())
}
