use iroh::{Endpoint, SecretKey, endpoint::presets};

/// ALPN for all luddite/control connections.
pub const ALPN: &[u8] = b"luddite/control/1";

/// ALPN for HTTP proxy streams (data plane).
/// Separate from the control plane ALPN.
pub const HTTP_PROXY_ALPN: &[u8] = b"luddite/http-proxy/1";

/// Create an Iroh `Endpoint` configured for Core (listener).
///
/// Binds with the given secret key and accepts incoming connections
/// on the `luddite/control/1` ALPN.
pub async fn create_core_endpoint(
  secret_key: SecretKey,
) -> anyhow::Result<Endpoint> {
  let endpoint = Endpoint::builder(presets::N0)
    .secret_key(secret_key)
    .alpns(vec![ALPN.to_vec()])
    .bind()
    .await?;
  Ok(endpoint)
}

/// Create an Iroh `Endpoint` configured for Periphery (dialer).
///
/// Binds with the given secret key and accepts incoming connections
/// on both the control and HTTP-proxy ALPNs. Periphery dials out to
/// Core on the control ALPN, and accepts data-plane streams from
/// ingress peripheries on the HTTP-proxy ALPN.
pub async fn create_periphery_endpoint(
  secret_key: SecretKey,
) -> anyhow::Result<Endpoint> {
  let endpoint = Endpoint::builder(presets::N0)
    .secret_key(secret_key)
    .alpns(vec![ALPN.to_vec(), HTTP_PROXY_ALPN.to_vec()])
    .bind()
    .await?;
  Ok(endpoint)
}
