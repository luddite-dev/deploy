//! Toy example: two Iroh endpoints on the local loopback exchange a ping/pong.
//!
//! This intentionally bypasses relay servers to prove direct-address dialing works.

use anyhow::Result;
use iroh::{endpoint::presets, Endpoint, EndpointAddr};

const ALPN: &[u8] = b"luddite/toy/1";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Listener: bound only to loopback, configured to accept our ALPN.
    let listener = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind_addr("127.0.0.1:0")?
        .bind()
        .await?;

    let listener_sock = listener
        .bound_sockets()
        .into_iter()
        .next()
        .expect("listener should be bound");
    let listener_addr = EndpointAddr::new(listener.id()).with_ip_addr(listener_sock);
    println!("listener id={} addr={listener_sock}", listener.id());

    // Spawn the echo side.
    let echo_task = tokio::spawn(run_echo(listener));

    // Dialer: no ALPN needed for outbound-only, but set it for symmetry.
    let dialer = Endpoint::bind(presets::N0).await?;
    println!("dialer id={}", dialer.id());

    let conn = dialer.connect(listener_addr, ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(b"ping").await?;
    send.finish()?;

    let mut buf = [0u8; 16];
    let n = recv.read(&mut buf).await?;
    let response = std::str::from_utf8(&buf[..n.unwrap_or(0)])?;
    println!("dialer received: {response}");

    assert_eq!(response, "pong");
    conn.close(0u32.into(), b"done");
    dialer.close().await;

    echo_task.await??;
    println!("direct ping OK");
    Ok(())
}

async fn run_echo(endpoint: Endpoint) -> Result<()> {
    let incoming = endpoint
        .accept()
        .await
        .expect("should get an incoming connection");
    let conn = incoming.await?;
    let (mut send, mut recv) = conn.accept_bi().await?;

    let mut buf = [0u8; 16];
    let n = recv.read(&mut buf).await?;
    let request = std::str::from_utf8(&buf[..n.unwrap_or(0)])?;
    println!("echo received: {request}");
    assert_eq!(request, "ping");

    send.write_all(b"pong").await?;
    send.finish()?;
    conn.closed().await;
    endpoint.close().await;
    Ok(())
}
