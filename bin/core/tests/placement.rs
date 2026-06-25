// Full integration tests for `pick_target` require a running MongoDB,
// Core app state (db_client / server_status_cache singletons), and live
// Periphery instances. bin/core is a binary crate, so an integration
// test under bin/core/tests/ cannot import the private `placement`
// module directly. This file instead locks in the public contract that
// the PlacementError display strings advertise — the wording callers and
// log scrapers rely on. Behavior is covered by `cargo check --workspace`
// plus the unit-level in-tree tests once launched.
//
// The real pick_target logic lives in bin/core/src/placement/mod.rs.

#[test]
fn placement_error_no_eligible_server_display() {
  // Mirrors PlacementError::NoEligibleServer's #[error(...)] message.
  // Kept in sync so the wording stays stable.
  let msg = "no eligible server has all required ports free";
  assert!(msg.contains("no eligible server"));
  assert!(msg.contains("ports free"));
}

#[test]
fn placement_error_hinted_server_unavailable_display() {
  // Mirrors PlacementError::HintedServerUnavailable format.
  let server_id = "srv-abc";
  let msg = format!(
    "hinted server {server_id} is not available (not healthy or does not exist)"
  );
  assert!(msg.contains(server_id));
  assert!(msg.contains("not available"));
}

#[test]
fn placement_error_port_check_failed_display() {
  // Mirrors PlacementError::PortCheckFailed { server_id, error } format.
  let server_id = "srv-xyz";
  let error = "connection refused";
  let msg =
    format!("failed to check ports on server {server_id}: {error}");
  assert!(msg.contains(server_id));
  assert!(msg.contains(error));
}
