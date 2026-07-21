//! Ingress node failover.
//!
//! When an ingress node goes [ServerState::NotOk], Core must
//! migrate its DNS records (and Caddy config) to a different
//! healthy ingress node. This module is the entry point for that
//! flow:
//!
//! 1. Select a new ingress node (healthy + ingress_enabled).
//! 2. Repoint DNS records via [super::management::update_dns_records_for_node].
//! 3. (Task 10) Rebuild and push the Caddy config to the new ingress Periphery.

use anyhow::{Context, Result};
use database::mungos::{find::find_collect, mongodb::bson::doc};
use komodo_client::entities::{
  dns::IngressConfig, server::Server, server::ServerState,
};
use tracing::info;

use crate::state::{db_client, server_status_cache};

/// Handle failover when an ingress node goes down.
///
/// 1. Select a new ingress node.
/// 2. Update DNS records to point to the new node.
/// 3. Rebuild + push Caddy config to the new ingress Periphery (Task 10).
pub async fn handle_ingress_failover(
  failed_node_id: &str,
  ingress_config: &IngressConfig,
) -> Result<()> {
  let new_node = select_new_ingress_node(failed_node_id)
    .await
    .context("select new ingress node for failover")?;
  info!(
    "Failover: selected new ingress node {} for failed node {}",
    new_node.id, failed_node_id
  );

  // Read new node's public IPs from cache (same pattern as
  // try_setup_ingress in resource/deployment.rs).
  let cache_entry = server_status_cache().get(&new_node.id).await;
  let (new_ipv4, new_ipv6) = cache_entry
    .as_ref()
    .and_then(|s| s.periphery_info.as_ref())
    .map(|info| (info.public_ipv4.clone(), info.public_ipv6.clone()))
    .unwrap_or((None, None));

  if new_ipv4.is_none() && new_ipv6.is_none() {
    anyhow::bail!(
      "failover target node {} has no cached public_ipv4/v6 — \
       wait for the next poll cycle or set \
       PERIPHERY_PUBLIC_IPV4 / _IPV6 on the Periphery host",
      new_node.id
    );
  }

  // Repoint DNS records to the new ingress node.
  super::management::update_dns_records_for_node(
    failed_node_id,
    &new_node.id,
    new_ipv4.as_deref(),
    new_ipv6.as_deref(),
    ingress_config,
  )
  .await
  .context("update DNS records during failover")?;

  // TODO(Task 10): Rebuild + push Caddy config to the new ingress
  // Periphery. This will be wired in once the deployment lifecycle
  // integration lands in Task 10.

  info!(
    "Failover complete: migrated DNS from {} to {}",
    failed_node_id, new_node.id
  );
  Ok(())
}

/// Select a new ingress node: `ingress_enabled=true`, state `Ok`,
/// excluding `failed_node_id`.
///
/// Queries MongoDB for `config.ingress_enabled = true` (a relatively
/// static config field), then checks the in-memory
/// `server_status_cache()` for `ServerState::Ok`. The cache is the
/// authoritative runtime state — the monitor loop
/// (`refresh_server_cache`) updates the cache every 15s but does NOT
/// write `info.state` back to MongoDB, so querying the DB for state
/// would return stale data.
pub async fn select_new_ingress_node(
  failed_node_id: &str,
) -> Result<Server> {
  let candidates: Vec<Server> = find_collect(
    &db_client().servers,
    doc! {
      "config.ingress_enabled": true,
    },
    None,
  )
  .await
  .context("failed to query ingress-enabled servers")?;

  let cache = server_status_cache();

  for server in candidates {
    if server.id == failed_node_id {
      continue;
    }
    // Check the in-memory cache for live server state.
    // If the server is not in the cache yet (e.g. Core just
    // started), fall back to accepting it — the monitor loop
    // will correct the state on the next cycle.
    let healthy = match cache.get(&server.id).await {
      Some(status) => matches!(status.state, ServerState::Ok),
      None => true,
    };
    if healthy {
      return Ok(server);
    }
  }

  anyhow::bail!(
    "no healthy ingress-enabled server available for failover \
     (excluding failed node {failed_node_id})"
  )
}
