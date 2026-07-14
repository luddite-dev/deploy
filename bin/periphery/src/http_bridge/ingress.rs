//! Ingress-side HTTP bridge.
//!
//! Runs on ingress nodes only. Caddy reverse_proxies HTTP requests
//! to this axum listener on `127.0.0.1:<port>`.
//!
//! For each incoming HTTP request:
//! 1. Extract `X-Target-Endpoint` (worker EndpointId) and
//!    `X-Target-Port` (container host port) headers
//! 2. Get or create a pooled Iroh QUIC connection to the worker
//! 3. Open a bidi stream on `HTTP_PROXY_ALPN`
//! 4. Write `[u16 target_port][raw HTTP/1.1 request bytes]`
//! 5. Half-close (finish) the send side
//! 6. Read the raw HTTP response from the stream
//! 7. Parse it into status + headers + body and return as an axum Response

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
  Router,
  body::Body,
  extract::State,
  http::{HeaderName, HeaderValue, Request, Response, StatusCode},
  routing::any,
};
use iroh::{
  Endpoint, EndpointAddr, EndpointId, endpoint::Connection,
};
use tokio::sync::RwLock;
use tracing::{info, warn};

use transport::iroh::endpoint::HTTP_PROXY_ALPN;

/// Pooled Iroh connections keyed by worker EndpointId string.
type ConnPool = Arc<RwLock<HashMap<String, Connection>>>;

/// Shared state for the ingress bridge.
#[derive(Clone)]
struct BridgeState {
  /// Iroh endpoint used to dial workers.
  endpoint: Endpoint,
  /// Connection pool — one QUIC connection per worker endpoint.
  pool: ConnPool,
  /// Local EndpointId string — used to short-circuit when
  /// the target is the local Periphery itself (Iroh cannot
  /// connect to its own endpoint).
  local_endpoint_id: String,
}

/// Start the ingress HTTP bridge listener.
///
/// Caddy reverse_proxies to `127.0.0.1:{port}`. Each request is
/// forwarded over an Iroh bidi stream to the target worker.
pub async fn start_ingress_bridge(
  endpoint: Endpoint,
  port: u16,
) -> Result<()> {
  let local_id = endpoint.id();
  info!(
    "Starting HTTP ingress bridge on 127.0.0.1:{port} (EndpointId: {})",
    local_id
  );

  let state = BridgeState {
    endpoint,
    pool: Arc::new(RwLock::new(HashMap::new())),
    local_endpoint_id: local_id.to_string(),
  };

  let app = Router::new()
    .route("/{*path}", any(handle_request))
    .route("/", any(handle_request))
    .with_state(state);

  let listener =
    tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
      .await
      .with_context(|| format!("Failed to bind 127.0.0.1:{port}"))?;
  info!("HTTP ingress bridge listening on 127.0.0.1:{port}");

  axum::serve(listener, app)
    .await
    .context("HTTP ingress bridge server error")?;

  Ok(())
}

/// Handle a single HTTP request from Caddy.
async fn handle_request(
  State(state): State<BridgeState>,
  request: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
  // Extract target headers
  let headers = request.headers();
  let target_endpoint = headers
    .get("x-target-endpoint")
    .and_then(|v| v.to_str().ok())
    .map(|s| s.to_string())
    .ok_or_else(|| {
      (
        StatusCode::BAD_REQUEST,
        "Missing X-Target-Endpoint header".to_string(),
      )
    })?;
  let target_port: u16 = headers
    .get("x-target-port")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.parse().ok())
    .ok_or_else(|| {
      (
        StatusCode::BAD_REQUEST,
        "Missing or invalid X-Target-Port header".to_string(),
      )
    })?;

  // Reconstruct raw HTTP/1.1 request bytes from the axum request
  let raw_request = match reconstruct_http_request(request).await {
    Ok(bytes) => bytes,
    Err(e) => {
      warn!("Failed to reconstruct HTTP request | {e:#}");
      return Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        "Failed to reconstruct request".to_string(),
      ));
    }
  };

  // Short-circuit: if the target is the local Periphery itself,
  // connect directly via TCP (Iroh cannot connect to its own
  // endpoint).
  if target_endpoint == state.local_endpoint_id {
    return proxy_to_local(target_port, &raw_request).await;
  }

  // Get or create a pooled Iroh connection to the worker
  let conn = match get_or_create_connection(&state, &target_endpoint)
    .await
  {
    Ok(conn) => conn,
    Err(e) => {
      warn!("Failed to get connection to {target_endpoint} | {e:#}");
      return Err((
        StatusCode::BAD_GATEWAY,
        format!("Failed to connect to worker: {e}"),
      ));
    }
  };

  // Open a bidi stream
  let (mut send, mut recv) = match conn.open_bi().await {
    Ok(streams) => streams,
    Err(e) => {
      warn!("Failed to open bidi stream | {e:#}");
      // Remove the dead connection from the pool
      state.pool.write().await.remove(&target_endpoint);
      return Err((
        StatusCode::BAD_GATEWAY,
        format!("Failed to open stream: {e}"),
      ));
    }
  };

  // Write the u16 port prefix (big-endian)
  let port_bytes = target_port.to_be_bytes();
  if let Err(e) = send.write_all(&port_bytes).await {
    warn!("Failed to write port prefix | {e:#}");
    return Err((
      StatusCode::BAD_GATEWAY,
      format!("Failed to write port prefix: {e}"),
    ));
  }

  // Write the raw HTTP request bytes
  if let Err(e) = send.write_all(&raw_request).await {
    warn!("Failed to write HTTP request | {e:#}");
    return Err((
      StatusCode::BAD_GATEWAY,
      format!("Failed to write request: {e}"),
    ));
  }

  // Half-close to signal request complete
  let _ = send.finish();

  // Read the full response from the stream
  let response_bytes = match recv.read_to_end(64 * 1024 * 1024).await
  {
    Ok(bytes) => bytes,
    Err(e) => {
      warn!("Failed to read response from stream | {e:#}");
      return Err((
        StatusCode::BAD_GATEWAY,
        format!("Failed to read response: {e}"),
      ));
    }
  };

  // Parse the raw HTTP response into an axum Response
  parse_http_response(&response_bytes)
}

/// Proxy a raw HTTP/1.1 request directly to `127.0.0.1:port`,
/// bypassing Iroh entirely (used when the target is the local
/// Periphery itself).
async fn proxy_to_local(
  port: u16,
  raw_request: &[u8],
) -> Result<Response<Body>, (StatusCode, String)> {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};
  use tokio::net::TcpStream;
  use tokio::time::timeout;

  let addr = format!("127.0.0.1:{port}");
  let mut stream =
    match timeout(Duration::from_secs(30), TcpStream::connect(&addr))
      .await
    {
      Ok(Ok(stream)) => stream,
      Ok(Err(e)) => {
        warn!("Local proxy: failed to connect to {addr} | {e:#}");
        return Err((
          StatusCode::BAD_GATEWAY,
          format!("Failed to connect to local port {port}: {e}"),
        ));
      }
      Err(_) => {
        return Err((
          StatusCode::GATEWAY_TIMEOUT,
          format!("Timed out connecting to local port {port}"),
        ));
      }
    };

  // Write the raw HTTP request
  if let Err(e) = stream.write_all(raw_request).await {
    return Err((
      StatusCode::BAD_GATEWAY,
      format!("Failed to write request to container: {e}"),
    ));
  }
  stream.shutdown().await.ok();

  // Read the full HTTP response
  let mut response_bytes = Vec::new();
  match timeout(
    Duration::from_secs(300),
    stream.read_to_end(&mut response_bytes),
  )
  .await
  {
    Ok(Ok(_)) => {}
    Ok(Err(e)) => {
      return Err((
        StatusCode::BAD_GATEWAY,
        format!("Failed to read response from container: {e}"),
      ));
    }
    Err(_) => {
      return Err((
        StatusCode::GATEWAY_TIMEOUT,
        "Timed out reading response from container".to_string(),
      ));
    }
  }

  parse_http_response(&response_bytes)
}

/// Get a pooled connection or create a new one.
async fn get_or_create_connection(
  state: &BridgeState,
  target_endpoint: &str,
) -> Result<Connection> {
  // Try the pool first
  {
    let pool = state.pool.read().await;
    if let Some(conn) = pool.get(target_endpoint) {
      if conn.close_reason().is_none() {
        return Ok(conn.clone());
      }
    }
  }

  // Connection missing or dead — create a new one
  info!("Creating new Iroh connection to worker {target_endpoint}");
  let endpoint_id: EndpointId =
    target_endpoint.parse().with_context(|| {
      format!("Failed to parse EndpointId from '{target_endpoint}'")
    })?;
  let addr: EndpointAddr = endpoint_id.into();
  let conn = state
    .endpoint
    .connect(addr, HTTP_PROXY_ALPN)
    .await
    .context("Failed to connect to worker Iroh endpoint")?;

  // Store in pool
  state
    .pool
    .write()
    .await
    .insert(target_endpoint.to_string(), conn.clone());

  Ok(conn)
}

/// Reconstruct raw HTTP/1.1 request bytes from an axum Request.
///
/// Produces: `{METHOD} {path?query} HTTP/1.1\r\n{headers}\r\n\r\n{body}`
async fn reconstruct_http_request(
  request: Request<Body>,
) -> Result<Vec<u8>> {
  let (parts, body) = request.into_parts();
  let method = parts.method;
  let uri = parts.uri;
  let headers = parts.headers;

  // Collect body bytes
  let body_bytes = axum::body::to_bytes(body, 256 * 1024 * 1024)
    .await
    .map_err(|e| {
      anyhow::anyhow!("Failed to collect request body: {e}")
    })?;

  // Build the request line
  let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");

  let mut buf = Vec::with_capacity(512 + body_bytes.len());
  buf.extend_from_slice(
    format!("{method} {path} HTTP/1.1\r\n").as_bytes(),
  );

  // Write headers, skipping hop-by-hop and routing-metadata headers
  // that must not be forwarded to the container:
  //   - connection: replaced with `Connection: close` below
  //   - transfer-encoding: body is de-chunked by to_bytes, so the
  //     header would lie; replaced with Content-Length
  //   - x-target-endpoint / x-target-port: internal routing metadata
  const SKIP_HEADERS: &[&str] = &[
    "connection",
    "transfer-encoding",
    "x-target-endpoint",
    "x-target-port",
  ];
  for (name, value) in headers.iter() {
    let lower = name.as_str().to_lowercase();
    if SKIP_HEADERS.contains(&lower.as_str()) {
      continue;
    }
    buf.extend_from_slice(name.as_str().as_bytes());
    buf.extend_from_slice(b": ");
    buf.extend_from_slice(value.as_bytes());
    buf.extend_from_slice(b"\r\n");
  }

  // Inject Connection: close so the container closes the TCP
  // connection after sending the response. Without this HTTP/1.1
  // defaults to keep-alive and read_response hangs waiting for EOF.
  buf.extend_from_slice(b"Connection: close\r\n");

  // If there is a body, set Content-Length from the actual byte
  // length (the body has already been de-chunked by to_bytes).
  if !body_bytes.is_empty() {
    buf.extend_from_slice(
      format!("Content-Length: {}\r\n", body_bytes.len()).as_bytes(),
    );
  }

  // Blank line to end headers
  buf.extend_from_slice(b"\r\n");

  // Write body
  buf.extend_from_slice(&body_bytes);

  Ok(buf)
}

/// Parse a raw HTTP/1.1 response into an axum Response.
///
/// Expected format: `HTTP/1.1 {code} {reason}\r\n{headers}\r\n\r\n{body}`
fn parse_http_response(
  raw: &[u8],
) -> Result<Response<Body>, (StatusCode, String)> {
  // Find end of headers (double CRLF)
  let header_end =
    find_subsequence(raw, b"\r\n\r\n").ok_or_else(|| {
      (
        StatusCode::BAD_GATEWAY,
        "Malformed HTTP response: no header terminator".to_string(),
      )
    })?;

  let head = &raw[..header_end];
  let body = &raw[header_end + 4..];

  let head_str = std::str::from_utf8(head).map_err(|e| {
    (
      StatusCode::BAD_GATEWAY,
      format!("Malformed HTTP response: {e}"),
    )
  })?;

  let mut lines = head_str.split("\r\n");

  // Parse status line: HTTP/1.1 200 OK
  let status_line = lines.next().ok_or_else(|| {
    (
      StatusCode::BAD_GATEWAY,
      "Malformed HTTP response: no status line".to_string(),
    )
  })?;
  let status_parts: Vec<&str> = status_line.splitn(3, ' ').collect();
  if status_parts.len() < 2 {
    return Err((
      StatusCode::BAD_GATEWAY,
      "Malformed HTTP response: bad status line".to_string(),
    ));
  }
  let status_code: u16 = status_parts[1].parse().map_err(|_| {
    (
      StatusCode::BAD_GATEWAY,
      "Malformed HTTP response: bad status code".to_string(),
    )
  })?;
  let status = StatusCode::from_u16(status_code).map_err(|_| {
    (
      StatusCode::BAD_GATEWAY,
      format!("Invalid status code: {status_code}"),
    )
  })?;

  // Parse headers
  let mut response = Response::builder().status(status);
  for line in lines {
    if line.is_empty() {
      continue;
    }
    if let Some((name, value)) = line.split_once(": ") {
      if let (Ok(name), Ok(value)) = (
        HeaderName::from_bytes(name.as_bytes()),
        HeaderValue::from_str(value),
      ) {
        response = response.header(name, value);
      }
    }
  }

  // Set body
  response.body(Body::from(body.to_vec())).map_err(|e| {
    (
      StatusCode::INTERNAL_SERVER_ERROR,
      format!("Failed to build response: {e}"),
    )
  })
}

/// Find the first occurrence of a subsequence in a byte slice.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
  haystack
    .windows(needle.len())
    .position(|window| window == needle)
}
