//! Caddy process supervision.
//!
//! Spawns the Caddy binary as a child process running with the JSON
//! adapter, and provides a helper to hot-reload configuration via
//! the Caddy admin API (`POST /load` on 127.0.0.1:2019).

use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::process::Command;
use tracing::{error, info, warn};

/// The Caddy admin API base URL.
const ADMIN_API: &str = "http://127.0.0.1:2019";

/// Spawn the Caddy process and supervise it.
///
/// - Starts `caddy run --adapter json`
/// - Waits 2 seconds for startup
/// - Checks the process is still alive
/// - Spawns a monitor task that restarts Caddy on unexpected exit
pub async fn start_caddy(binary_path: &str) -> Result<()> {
  info!("Starting Caddy: {binary_path} run --adapter json");

  let mut child = Command::new(binary_path)
    .arg("run")
    .arg("--adapter")
    .arg("json")
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .with_context(|| {
      format!("Failed to spawn Caddy binary at {binary_path}")
    })?;

  // Wait briefly for the process to start up.
  tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

  // Check the process is still alive after the grace period.
  match child.try_wait() {
    Ok(Some(status)) => {
      bail!(
        "Caddy exited immediately during startup with status {status}"
      );
    }
    Ok(None) => {
      info!(
        "Caddy process is running (pid: {})",
        child.id().unwrap_or(0)
      );
    }
    Err(e) => {
      warn!("Failed to poll Caddy process status: {e}");
    }
  }

  // Spawn a monitor task that restarts Caddy on crash.
  let binary_owned = binary_path.to_string();
  tokio::spawn(async move {
    monitor_caddy(&binary_owned).await;
  });

  Ok(())
}

/// Monitor the (already-spawned) Caddy process and restart it on exit.
///
/// This loop re-spawns Caddy with a small backoff if it exits.
async fn monitor_caddy(binary_path: &str) {
  loop {
    info!("(re)spawning Caddy: {binary_path} run --adapter json");

    let mut child = match Command::new(binary_path)
      .arg("run")
      .arg("--adapter")
      .arg("json")
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
    {
      Ok(c) => c,
      Err(e) => {
        error!("Failed to spawn Caddy: {e:#}");
        // Back off before retrying.
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        continue;
      }
    };

    let pid = child.id().unwrap_or(0);
    info!("Caddy process started (pid: {pid})");

    match child.wait().await {
      Ok(status) => {
        warn!("Caddy process (pid: {pid}) exited with {status}");
      }
      Err(e) => {
        error!("Failed to wait on Caddy process (pid: {pid}): {e}");
      }
    }

    // Back off before restarting to avoid tight crash loops.
    warn!("Restarting Caddy in 5 seconds…");
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
  }
}

/// Hot-reload the Caddy configuration via the admin API.
///
/// Sends `POST /load` to `127.0.0.1:2019` with the given JSON config
/// as the request body (`Content-Type: application/json`).
pub async fn reload_config(config: &Value) -> Result<()> {
  let client = reqwest::Client::new();

  let resp = client
    .post(format!("{ADMIN_API}/load"))
    .header("Content-Type", "application/json")
    .json(config)
    .send()
    .await
    .context("Failed to POST Caddy config to admin API")?;

  if !resp.status().is_success() {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    bail!("Caddy admin API /load returned HTTP {status}: {body}");
  }

  info!("Caddy config reloaded via admin API");
  Ok(())
}
