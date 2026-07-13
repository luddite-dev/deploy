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
use komodo_client::entities::{dns::IngressConfig, server::Server};
use tracing::info;

use crate::state::db_client;

/// The DB string form of [ServerState::Ok].
///
/// [ServerState] derives `strum::Display` with
/// `serialize_all = "kebab-case"`, but the value persisted to Mongo
/// is driven by serde (the rust variant name, eg `"Ok"`), as the
/// codebase writes literals like `"info.state": "Draining"` directly
/// (see `server/drain.rs`). We use the literal here to match.
const SERVER_STATE_OK: &str = "Ok";

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

  // Repoint DNS records to the new ingress node.
  super::management::update_dns_records_for_node(
    failed_node_id,
    &new_node.id,
    new_node.config.public_ipv4.as_deref(),
    new_node.config.public_ipv6.as_deref(),
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
/// Query servers by `config.ingress_enabled = true` and
/// `info.state = "Ok"`, then pick the first one whose `id` is not
/// the failed node. Exclusion is done in Rust rather than via Mongo
/// `$ne` on `_id` so that non-ObjectId id strings do not cause a
/// parse failure.
async fn select_new_ingress_node(
  failed_node_id: &str,
) -> Result<Server> {
  let candidates: Vec<Server> = find_collect(
    &db_client().servers,
    doc! {
      "config.ingress_enabled": true,
      "info.state": SERVER_STATE_OK,
    },
    None,
  )
  .await
  .context("failed to query ingress-enabled servers")?;

  candidates
    .into_iter()
    .find(|s| s.id != failed_node_id)
    .ok_or_else(|| {
      anyhow::anyhow!(
        "no healthy ingress-enabled server available for failover \
         (excluding failed node {failed_node_id})"
      )
    })
}
