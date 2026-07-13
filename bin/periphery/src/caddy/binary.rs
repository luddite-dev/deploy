//! Caddy binary management.
//!
//! Fetches a manifest from the vendored repo, compares the local
//! version, downloads a new binary if needed, verifies the SHA256
//! checksum, and performs an atomic swap into place.

use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// Top-level manifest structure served at `manifest_url`.
#[derive(Debug, Deserialize)]
struct Manifest {
  /// Keyed by artifact name, e.g. `"caddy"`.
  #[serde(default)]
  artifacts: HashMap<String, Artifact>,
}

/// A single downloadable artifact entry.
#[derive(Debug, Deserialize)]
struct Artifact {
  /// Semantic version string, e.g. `"v2.8.4"`.
  version: String,
  /// Download URL with optional `{{arch}}` placeholder.
  download_url: String,
  /// Checksums keyed by `sha256`.
  #[serde(default)]
  checksums: HashMap<String, String>,
}

/// Ensure the Caddy binary at `binary_path` matches the version
/// advertised in the manifest at `manifest_url`.
///
/// 1. Fetch the manifest.
/// 2. Look up the `"caddy"` artifact.
/// 3. If a local binary exists and its version matches, return early.
/// 4. Otherwise download, verify SHA256, and atomically swap into place.
pub async fn ensure_caddy_binary(
  binary_path: &str,
  manifest_url: &str,
) -> Result<()> {
  let manifest = fetch_manifest(manifest_url).await?;

  let artifact =
    manifest.artifacts.get("caddy").ok_or_else(|| {
      anyhow::anyhow!("manifest does not contain a 'caddy' artifact")
    })?;

  // If the local binary already matches the manifest version, skip.
  if let Some(local_version) =
    get_local_caddy_version(binary_path).await
  {
    if local_version == artifact.version {
      info!(
        "Caddy binary {} already at {} — skipping download",
        binary_path, artifact.version
      );
      return Ok(());
    }
    info!(
      "Caddy binary: local {} → remote {}, updating",
      local_version, artifact.version
    );
  }

  let arch = current_arch();
  let url = artifact.download_url.replace("{{arch}}", arch);

  let expected_checksum = artifact
    .checksums
    .get("sha256")
    .ok_or_else(|| {
      anyhow::anyhow!("manifest missing sha256 checksum for caddy")
    })?
    .trim_start_matches("sha256:");

  let data = download_binary(&url).await.with_context(|| {
    format!("Failed to download Caddy binary from {url}")
  })?;

  // Verify checksum
  let actual = sha256(&data);
  if actual != expected_checksum {
    bail!(
      "Caddy binary checksum mismatch: expected {expected_checksum}, got {actual}"
    );
  }
  info!("Caddy binary checksum verified ({actual})");

  // Atomic swap: write to .tmp then rename
  let tmp_path = format!("{binary_path}.tmp");
  let bin_dir = Path::new(binary_path)
    .parent()
    .context("binary_path has no parent directory")?;
  tokio::fs::create_dir_all(bin_dir).await.with_context(|| {
    format!("Failed to create bin dir {:?}", bin_dir)
  })?;

  tokio::fs::write(&tmp_path, &data).await.with_context(|| {
    format!("Failed to write tmp binary {tmp_path}")
  })?;

  // Set executable permissions on Unix
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(&tmp_path, perms).with_context(
      || format!("Failed to set permissions on {tmp_path}"),
    )?;
  }

  tokio::fs::rename(&tmp_path, binary_path)
    .await
    .with_context(|| {
      format!("Failed to rename {tmp_path} → {binary_path}")
    })?;

  info!(
    "Caddy binary {} installed ({}, {})",
    binary_path, artifact.version, arch
  );

  Ok(())
}

/// Fetch and deserialize the manifest JSON.
async fn fetch_manifest(url: &str) -> Result<Manifest> {
  let resp = reqwest::get(url)
    .await
    .with_context(|| format!("Failed to GET manifest from {url}"))?;
  if !resp.status().is_success() {
    bail!("manifest fetch returned HTTP {} for {url}", resp.status());
  }
  let manifest: Manifest = resp
    .json()
    .await
    .context("Failed to deserialize manifest JSON")?;
  Ok(manifest)
}

/// Run `caddy version` and parse the version string from stdout.
///
/// Caddy's `version` subcommand prints something like:
///   `v2.8.4 h1:...`
/// We extract just the leading version token (`v2.8.4`).
async fn get_local_caddy_version(
  binary_path: &str,
) -> Option<String> {
  let output = tokio::process::Command::new(binary_path)
    .arg("version")
    .output()
    .await
    .ok()?;

  if !output.status.success() {
    warn!(
      "caddy version exited with {} — treating as missing",
      output.status
    );
    return None;
  }

  let stdout = String::from_utf8_lossy(&output.stdout);
  // First whitespace-delimited token is the version.
  let version = stdout.split_whitespace().next()?;
  Some(version.to_string())
}

/// Compute the SHA256 hex digest of `data`.
fn sha256(data: &[u8]) -> String {
  let mut hasher = Sha256::new();
  hasher.update(data);
  hex::encode(hasher.finalize())
}

/// Map the compile-time target architecture to the manifest arch slug.
fn current_arch() -> &'static str {
  if cfg!(target_arch = "x86_64") {
    "linux-amd64"
  } else if cfg!(target_arch = "aarch64") {
    "linux-arm64"
  } else {
    "linux-amd64"
  }
}

/// Download the binary bytes from `url`.
async fn download_binary(url: &str) -> Result<Vec<u8>> {
  let resp = reqwest::get(url)
    .await
    .with_context(|| format!("Failed to GET binary from {url}"))?;
  if !resp.status().is_success() {
    bail!(
      "binary download returned HTTP {} for {url}",
      resp.status()
    );
  }
  let bytes = resp
    .bytes()
    .await
    .context("Failed to read binary response body")?;
  Ok(bytes.to_vec())
}
