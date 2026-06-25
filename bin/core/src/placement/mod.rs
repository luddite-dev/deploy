use std::collections::HashMap;

use database::mungos::{
  find::find_collect,
  mongodb::bson::doc,
};
use komodo_client::entities::server::{Server, ServerState};
use periphery_client::api::placement::CheckHostPorts;

use crate::{helpers::periphery_client, state::db_client, state::server_status_cache};

#[derive(Debug, thiserror::Error)]
pub enum PlacementError {
  #[error("hinted server {0} is not available (not healthy or does not exist)")]
  HintedServerUnavailable(String),
  #[error("hinted server {server_id} is healthy but one or more required host ports are busy")]
  HintedServerPortConflict { server_id: String },
  #[error("no eligible server has all required ports free")]
  NoEligibleServer,
  #[error("failed to check ports on server {server_id}: {error}")]
  PortCheckFailed { server_id: String, error: String },
}

/// Pick a target server for a deployment based on port availability.
///
/// - `fixed_ports`: host ports (`PortMapping.host = Some(p)`) that must be
///   free on the chosen node.
/// - `hint_server_id`: optional server_id the user pinned. Empty = the
///   scheduler decides.
///
/// Returns the chosen server_id. Per the placement spec, the caller writes
/// this id into `info.assigned_server` (NOT into `config.server_id`); the
/// user's empty-or-hint expression is preserved so future reevaluations
/// start from the user's intent.
pub async fn pick_target(
  fixed_ports: &[u16],
  hint_server_id: &str,
) -> Result<String, PlacementError> {
  // Load every server; candidate eligibility is decided from the live
  // status cache (state == Ok), not from persisted info.
  let servers: Vec<Server> = find_collect(&db_client().servers, doc! {}, None)
    .await
    .map_err(|e| PlacementError::PortCheckFailed {
      server_id: "DB".into(),
      error: e.to_string(),
    })?;

  let mut candidates: Vec<Server> = Vec::new();
  for server in &servers {
    if let Some(cached) = server_status_cache().get(&server.id).await {
      if matches!(cached.state, ServerState::Ok) {
        candidates.push(server.clone());
      }
    }
  }

  // Spread heuristic: fewest currently-assigned deployments wins ties.
  let counts = count_deployments_per_server().await;
  candidates.sort_by_key(|s| counts.get(&s.id).copied().unwrap_or(0));

  // No fixed ports and no hint: any healthy candidate will do.
  if fixed_ports.is_empty() && hint_server_id.is_empty() {
    return candidates
      .first()
      .map(|s| s.id.clone())
      .ok_or(PlacementError::NoEligibleServer);
  }

  // Hint path: the user pinned a server. Probe only that one; do not
  // silently fall back — preserves user intent.
  if !hint_server_id.is_empty() {
    let Some(server) = candidates.iter().find(|s| s.id == hint_server_id)
    else {
      return Err(PlacementError::HintedServerUnavailable(
        hint_server_id.to_string(),
      ));
    };
    let free = check_ports_on_server(server, fixed_ports).await?;
    if fixed_ports.iter().all(|p| free.contains(p)) {
      return Ok(server.id.clone());
    }
    return Err(PlacementError::HintedServerPortConflict {
      server_id: server.id.clone(),
    });
  }

  // No hint: probe each candidate (cheapest-loaded first) for a fit.
  for server in &candidates {
    let free = check_ports_on_server(server, fixed_ports).await?;
    if fixed_ports.iter().all(|p| free.contains(p)) {
      return Ok(server.id.clone());
    }
  }

  Err(PlacementError::NoEligibleServer)
}

async fn check_ports_on_server(
  server: &Server,
  ports: &[u16],
) -> Result<Vec<u16>, PlacementError> {
  if ports.is_empty() {
    return Ok(Vec::new());
  }
  let periphery = periphery_client(server)
    .await
    .map_err(|e| PlacementError::PortCheckFailed {
      server_id: server.id.clone(),
      error: e.to_string(),
    })?;
  let response = periphery
    .request(CheckHostPorts { ports: ports.to_vec() })
    .await
    .map_err(|e| PlacementError::PortCheckFailed {
      server_id: server.id.clone(),
      error: e.to_string(),
    })?;
  Ok(response.free)
}

/// Cheap spread heuristic: count workloads (deployments + stacks) already
/// assigned to each server via the `info.assigned_server` index. Failures
/// are treated as an empty map (every candidate ties at zero) — placement
/// still works, just without the spread signal.
async fn count_deployments_per_server() -> HashMap<String, u32> {
  let mut counts = HashMap::new();
  let deployments: Vec<komodo_client::entities::deployment::Deployment> =
    find_collect(&db_client().deployments, doc! {}, None)
      .await
      .unwrap_or_default();
  for d in deployments {
    if !d.info.assigned_server.is_empty() {
      *counts.entry(d.info.assigned_server).or_insert(0) += 1;
    }
  }
  let stacks: Vec<komodo_client::entities::stack::Stack> =
    find_collect(&db_client().stacks, doc! {}, None)
      .await
      .unwrap_or_default();
  for s in stacks {
    if !s.info.assigned_server.is_empty() {
      *counts.entry(s.info.assigned_server).or_insert(0) += 1;
    }
  }
  counts
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn no_eligible_server_display() {
    let e = PlacementError::NoEligibleServer;
    assert!(e.to_string().contains("no eligible server"));
  }

  #[test]
  fn hinted_unavailable_display() {
    let e = PlacementError::HintedServerUnavailable("srv-x".into());
    let s = e.to_string();
    assert!(s.contains("srv-x"));
    assert!(s.contains("not available"));
  }

  #[test]
  fn hinted_port_conflict_display() {
    let e = PlacementError::HintedServerPortConflict {
      server_id: "srv-y".into(),
    };
    let s = e.to_string();
    assert!(s.contains("srv-y"));
    assert!(s.contains("ports are busy"));
  }

  #[test]
  fn port_check_failed_display() {
    let e = PlacementError::PortCheckFailed {
      server_id: "srv-z".into(),
      error: "timeout".into(),
    };
    let s = e.to_string();
    assert!(s.contains("srv-z"));
    assert!(s.contains("timeout"));
  }
}
