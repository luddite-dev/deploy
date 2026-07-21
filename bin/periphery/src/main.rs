#![allow(unused_crate_dependencies)]

use command::{CommandOptions, run_standard_command};
use futures_util::{StreamExt, stream::FuturesUnordered};
use komodo_client::entities::config::periphery::CliArgs;
use tracing::Instrument;

use crate::{
  config::periphery_args,
  state::{host_public_ipv4, host_public_ipv6, periphery_secret_key},
};

#[macro_use]
extern crate tracing;

mod api;
mod caddy;
mod config;
mod connection;
mod docker;
mod helpers;
mod http_bridge;
mod stack;
mod state;
mod stats;
mod terminal;

async fn app() -> anyhow::Result<()> {
  dotenvy::dotenv().ok();
  let config = config::periphery_config();
  mogh_logger::init(&config.logging)?;

  let startup_span = info_span!("PeripheryStartup");

  check_podman_volume_export_import_support().await?;

  // Init Iroh secret key and endpoint
  let secret_key = periphery_secret_key().clone();
  info!("Iroh EndpointId: {}", secret_key.public());

  let endpoint =
    transport::iroh::endpoint::create_periphery_endpoint(secret_key)
      .await?;

  let mut handles = async {
    info!("Komodo Periphery version: v{}", env!("CARGO_PKG_VERSION"));

    if config.pretty_startup_config {
      info!("{:#?}", config.sanitized());
    } else {
      info!("{:?}", config.sanitized());
    }

    stats::spawn_polling_thread();
    docker::stats::spawn_polling_thread();

    let handles = FuturesUnordered::new();

    // Spawn outbound connections to Core
    if config.core_endpoint_addrs.is_empty() {
      info!("No core_endpoint_addrs configured. Waiting for environment variable setup.");
    } else if config.connect_as.is_empty() {
      warn!(
        "'core_endpoint_addrs' are defined for outbound connection, but missing 'connect_as' (PERIPHERY_CONNECT_AS)."
      );
    } else {
      for addr in &config.core_endpoint_addrs {
        match connection::client::handler(endpoint.clone(), addr).await {
          Ok(handle) => handles.push(handle),
          Err(e) => {
            error!("Failed to start outbound connection to {addr} | {e:#}");
          }
        }
      }
    }

    handles
  }
  .instrument(startup_span)
  .await;

  // Start HTTP forward handler (all nodes)
  {
    let endpoint = endpoint.clone();
    tokio::spawn(async move {
      if let Err(e) =
        http_bridge::forward::start_forward_handler(endpoint).await
      {
        error!("HTTP forward handler error: {e:#}");
      }
    });
  }

  // ===========
  // = Ingress startup hard-gate =
  // ===========
  // Ingress nodes need a public IP (auto-discovered or env-overridden)
  // to route DNS records to. If both are None at startup, exit non-
  // zero so systemd reports failure rather than silently running an
  // ingress node that can't serve traffic.
  if config.ingress_enabled {
    let (ipv4, ipv6) =
      tokio::join!(host_public_ipv4(), host_public_ipv6());
    if ipv4.is_none() && ipv6.is_none() {
      error!(
        "ingress-enabled Periphery has no public IPv4/IPv6 — \
         set PERIPHERY_PUBLIC_IPV4 / PERIPHERY_PUBLIC_IPV6, \
         or ensure HTTPS egress to api4.ipify.org and \
         api6.ipify.org works"
      );
      std::process::exit(1);
    }
    info!(
      "Ingress startup check OK: ipv4={:?} ipv6={:?}",
      ipv4, ipv6
    );
  }

  // Start HTTP ingress bridge (ingress nodes only)
  if config.ingress_enabled {
    let endpoint = endpoint.clone();
    let port = config.http_bridge_port;
    tokio::spawn(async move {
      if let Err(e) =
        http_bridge::ingress::start_ingress_bridge(endpoint, port)
          .await
      {
        error!("HTTP ingress bridge error: {e:#}");
      }
    });
  }

  // Start Caddy supervisor (ingress nodes only)
  if config.ingress_enabled {
    let binary_path = config.caddy_binary_path.clone();
    let manifest_url = config.vendored_manifest_url.clone();
    tokio::spawn(async move {
      if let Err(e) = caddy::binary::ensure_caddy_binary(
        &binary_path,
        &manifest_url,
      )
      .await
      {
        error!("Failed to ensure Caddy binary: {e:#}");
        return;
      }
      if let Err(e) =
        caddy::supervisor::start_caddy(&binary_path).await
      {
        error!("Failed to start Caddy: {e:#}");
        return;
      }
      info!("Caddy supervisor running");
    });
  }

  // Watch the threads
  while let Some(res) = handles.next().await {
    match res {
      Ok(Err(e)) => {
        error!("CONNECTION ERROR: {e:#}");
      }
      Err(e) => {
        error!("SPAWN ERROR: {e:#}");
      }
      Ok(Ok(())) => {}
    }
  }

  Ok(())
}

/// Verifies the host Podman supports `volume export` and `volume import`.
/// If either subcommand is missing, Periphery refuses to start.
async fn check_podman_volume_export_import_support()
-> anyhow::Result<()> {
  for sub in ["volume export --help", "volume import --help"] {
    let cmd = format!("podman {sub}");
    let output =
      run_standard_command(&cmd, CommandOptions::default()).await;
    if !output.success() {
      anyhow::bail!(
        "unsupported Podman version: `podman {sub}` did not run successfully. \
         Periphery requires Podman with volume export/import support."
      );
    }
  }
  Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let _args: &CliArgs = periphery_args();

  let mut term_signal = tokio::signal::unix::signal(
    tokio::signal::unix::SignalKind::terminate(),
  )?;
  tokio::select! {
    res = tokio::spawn(app()) => return res?,
    _ = term_signal.recv() => {
      info!("Exiting all active Terminals for shutdown");
      terminal::delete_all_terminals().await;
      Ok(())
    },
  }
}
