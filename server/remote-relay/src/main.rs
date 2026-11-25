use std::net::SocketAddr;

use roalt_remote_relay::{RelayConfig, RelayState, serve};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let bind = std::env::var("RELAY_BIND").unwrap_or_else(|_| "0.0.0.0:8788".into());
    let addr: SocketAddr = bind.parse().expect("RELAY_BIND must be host:port (e.g. 0.0.0.0:8788)");
    serve(addr, RelayState::with_config(RelayConfig::from_env())).await
}
