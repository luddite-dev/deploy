//! Worker-side HTTP bridge handler.
//!
//! Runs on ALL nodes (even ingress nodes that also run workers).
//! Accepts incoming Iroh connections on `HTTP_PROXY_ALPN` and for
//! each bidi stream:
//! 1. Reads the 2-byte target host port prefix (big-endian u16)
//! 2. Reads the remaining bytes as a raw HTTP/1.1 request
//! 3. Connects to `127.0.0.1:<target_port>` (container host port)
//! 4. Writes the request bytes, shuts down the write half
//! 5. Reads the HTTP response from the container
//! 6. Sends the response back over the Iroh send stream
//! 7. Half-closes (finish) to signal response complete

use std::time::Duration;

use anyhow::Result;
use iroh::Endpoint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, warn};

use transport::iroh::endpoint::HTTP_PROXY_ALPN;

/// Start the HTTP forward handler on the given Iroh endpoint.
///
/// This function runs forever (until the endpoint is closed) and
/// spawns a tokio task per incoming connection and per bidi stream.
pub async fn start_forward_handler(endpoint: Endpoint) -> Result<()> {
  info!(
    "Starting HTTP forward handler (ALPN: {})",
    String::from_utf8_lossy(HTTP_PROXY_ALPN)
  );

  loop {
    let incoming = endpoint.accept().await;
    let Some(incoming) = incoming else {
      // Endpoint closed
      info!("HTTP forward handler: endpoint closed, exiting");
      break;
    };

    tokio::spawn(async move {
      let conn = match incoming.await {
        Ok(conn) => conn,
        Err(e) => {
          warn!("HTTP forward: failed to accept connection | {e:#}");
          return;
        }
      };

      // Verify the ALPN matches our HTTP proxy ALPN
      if conn.alpn() != HTTP_PROXY_ALPN {
        warn!(
          "HTTP forward: unexpected ALPN '{}', ignoring",
          String::from_utf8_lossy(conn.alpn())
        );
        return;
      }

      info!(
        "HTTP forward: accepted connection from {}",
        conn.remote_id()
      );

      // Per-connection loop: accept bidi streams
      loop {
        match conn.accept_bi().await {
          Ok((send, recv)) => {
            let endpoint_id = conn.remote_id().to_string();
            tokio::spawn(handle_stream(send, recv, endpoint_id));
          }
          Err(e) => {
            warn!("HTTP forward: accept_bi error | {e:#}");
            break;
          }
        }
      }
    });
  }

  Ok(())
}

/// Handle a single bidi stream: read port prefix + HTTP request,
/// forward to the container, pipe response back.
async fn handle_stream(
  mut send: iroh::endpoint::SendStream,
  mut recv: iroh::endpoint::RecvStream,
  endpoint_id: String,
) {
  // Read the 2-byte target port prefix (big-endian u16)
  let mut port_buf = [0u8; 2];
  if let Err(e) = recv.read_exact(&mut port_buf).await {
    warn!(
      "HTTP forward [{endpoint_id}]: failed to read port prefix | {e:#}"
    );
    return;
  }
  let target_port = u16::from_be_bytes(port_buf);

  // Read the rest of the stream as raw HTTP request bytes.
  // We use read_to_end with a generous size limit.
  let request_bytes = match recv.read_to_end(16 * 1024 * 1024).await {
    Ok(bytes) => bytes,
    Err(e) => {
      warn!(
        "HTTP forward [{endpoint_id}]: failed to read request body | {e:#}"
      );
      return;
    }
  };

  info!(
    "HTTP forward [{endpoint_id}]: {} bytes -> 127.0.0.1:{target_port}",
    request_bytes.len()
  );

  // Connect to the container's host port (with a 30s timeout so a
  // slow/unresponsive container can't stall the stream handler)
  let mut tcp = match tokio::time::timeout(
    Duration::from_secs(30),
    tokio::net::TcpStream::connect(format!(
      "127.0.0.1:{target_port}"
    )),
  )
  .await
  {
    Ok(Ok(stream)) => stream,
    Ok(Err(e)) => {
      error!(
        "HTTP forward [{endpoint_id}]: failed to connect to 127.0.0.1:{target_port} | {e:#}"
      );
      // Send a minimal 502 Bad Gateway response back
      let response = format!(
        "HTTP/1.1 502 Bad Gateway\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n"
      );
      let _ = send.write_all(response.as_bytes()).await;
      let _ = send.finish();
      return;
    }
    Err(_) => {
      error!(
        "HTTP forward [{endpoint_id}]: timed out connecting to 127.0.0.1:{target_port} after 30s"
      );
      let response = format!(
        "HTTP/1.1 504 Gateway Timeout\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n"
      );
      let _ = send.write_all(response.as_bytes()).await;
      let _ = send.finish();
      return;
    }
  };

  // Write the request to the container
  if let Err(e) = tcp.write_all(&request_bytes).await {
    warn!(
      "HTTP forward [{endpoint_id}]: failed to write request to container | {e:#}"
    );
    return;
  }

  // Shut down the write half to signal end of request
  let _ = tcp.shutdown().await;

  // Read the full response from the container and pipe it back.
  // We use a read loop since we don't know the response size ahead of time.
  // A 5-minute timeout prevents a hung container from stalling the
  // Iroh stream handler indefinitely.
  let response = match tokio::time::timeout(
    Duration::from_secs(300),
    read_response(&mut tcp),
  )
  .await
  {
    Ok(resp) => resp,
    Err(_) => {
      warn!(
        "HTTP forward [{endpoint_id}]: timed out reading response from 127.0.0.1:{target_port} after 300s"
      );
      // Send a 504 back so the ingress side gets a response instead of
      // hanging forever.
      let timeout_resp = format!(
        "HTTP/1.1 504 Gateway Timeout\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n"
      );
      let _ = send.write_all(timeout_resp.as_bytes()).await;
      let _ = send.finish();
      return;
    }
  };

  info!(
    "HTTP forward [{endpoint_id}]: {} bytes response <- 127.0.0.1:{target_port}",
    response.len()
  );

  // Send the response back over the Iroh stream
  if let Err(e) = send.write_all(&response).await {
    warn!(
      "HTTP forward [{endpoint_id}]: failed to write response to iroh stream | {e:#}"
    );
    return;
  }

  // Half-close to signal response complete
  let _ = send.finish();
}

/// Read the full HTTP response from a TCP stream until EOF.
async fn read_response(tcp: &mut tokio::net::TcpStream) -> Vec<u8> {
  let mut buf = Vec::with_capacity(8192);
  let mut chunk = [0u8; 8192];
  loop {
    match tcp.read(&mut chunk).await {
      Ok(0) => break, // EOF
      Ok(n) => buf.extend_from_slice(&chunk[..n]),
      Err(e) => {
        warn!(
          "HTTP forward: error reading container response | {e:#}"
        );
        break;
      }
    }
  }
  buf
}
