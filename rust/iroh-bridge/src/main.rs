use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use tokio::net::TcpListener;

use iroh_bridge::{http::router, network::Network, state::AppState};

#[tokio::main]
async fn main() -> Result<()> {
    let bind_addr: SocketAddr = std::env::var("LUDDITE_SIDECAR_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7777".to_string())
        .parse()?;
    let state = AppState::new(String::new());
    let network = Network::bind(state.clone()).await?;
    network.refresh_identity().await?;

    tokio::spawn({
        let network = network.clone();
        async move {
            loop {
                let _ = network.flush_outbound_once().await;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    });

    let listener = TcpListener::bind(bind_addr).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
