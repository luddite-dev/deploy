use std::net::SocketAddr;

use anyhow::Result;
use tokio::net::TcpListener;

use iroh_bridge::{http::router, state::AppState};

#[tokio::main]
async fn main() -> Result<()> {
    let bind_addr: SocketAddr = std::env::var("LUDDITE_SIDECAR_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7777".to_string())
        .parse()?;
    let identity = std::env::var("LUDDITE_ENDPOINT_ADDR_JSON").unwrap_or_default();
    let state = AppState::new(identity);

    let listener = TcpListener::bind(bind_addr).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
