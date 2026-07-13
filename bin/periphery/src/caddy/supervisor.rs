//! Caddy process supervision.
//!
//! Spawns the Caddy binary as a child process running with the JSON
//! adapter, and provides a helper to hot-reload configuration via
//! the Caddy admin API (`POST /load` on 127.0.0.1:2019).

use std::{process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::process::Command;
use tracing::{error, info};

/// The Caddy admin API base URL.
const ADMIN_API: &str = "http://127.0.0.1:2019";

/// Spawn the Caddy process and supervise it.
///
/// Spawns a tokio task that owns the `Child` handle and loops:
/// spawn → wait for startup → check alive → `wait()` on exit →
/// backoff → respawn. Keeping the `Child` in the spawned task avoids
/// the orphaned-process bug where dropping the handle detaches the
/// first child while a second monitor tries (and fails) to bind port
/// 2019.
pub async fn start_caddy(binary_path: &str) -> Result<()> {
  info!("Starting Caddy process: {binary_path}");
  let binary_path = binary_path.to_string();

  tokio::spawn(async move {
    loop {
      let mut child = match Command::new(&binary_path)
        .arg("run")
        .arg("--adapter")
        .arg("json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
      {
        Ok(child) => child,
        Err(e) => {
          error!("Failed to spawn Caddy: {e}");
          tokio::time::sleep(Duration::from_secs(5)).await;
          continue;
        }
      };

      // Wait for startup.
      tokio::time::sleep(Duration::from_secs(2)).await;

      // Check if still alive.
      match child.try_wait() {
        Ok(Some(status)) => {
          error!("Caddy exited immediately: {status}");
          tokio::time::sleep(Duration::from_secs(5)).await;
          continue;
        }
        Ok(None) => {
          info!("Caddy process started successfully");
        }
        Err(e) => {
          error!("Failed to check Caddy status: {e}");
          tokio::time::sleep(Duration::from_secs(5)).await;
          continue;
        }
      }

      // Wait for process exit.
      match child.wait().await {
        Ok(status) => error!("Caddy process exited: {status}"),
        Err(e) => error!("Caddy wait error: {e}"),
      }

      // Backoff before restart.
      tokio::time::sleep(Duration::from_secs(5)).await;
    }
  });

  Ok(())
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
