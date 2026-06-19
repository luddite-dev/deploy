use std::{env, net::SocketAddr, time::Duration};

use anyhow::Result;
use tokio::net::TcpListener;
use tracing::info;

use iroh_bridge::{http::router, network::Network, state::AppState};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let bind_addr: SocketAddr = env::var("LUDDITE_SIDECAR_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7777".to_string())
        .parse()?;
    let initial_identity = env::var("LUDDITE_ENDPOINT_ADDR_JSON").unwrap_or_default();

    let state = AppState::new(initial_identity);
    let network = Network::bind(state.clone()).await?;

    // Periodically refresh the identity so other nodes can dial us, and flush
    // any queued outbound envelopes.
    tokio::spawn({
        let network = network.clone();
        async move {
            loop {
                if let Err(err) = network.refresh_identity().await {
                    tracing::warn!(?err, "refresh identity failed");
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    });

    tokio::spawn({
        let network = network.clone();
        async move {
            loop {
                if let Err(err) = network.flush_outbound_once().await {
                    tracing::warn!(?err, "flush outbound failed");
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    });

    info!(%bind_addr, "serving luddite iroh-bridge sidecar");
    let listener = TcpListener::bind(bind_addr).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
