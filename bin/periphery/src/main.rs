#![allow(unused_crate_dependencies)]

use command::{CommandOptions, run_standard_command};
use futures_util::{StreamExt, stream::FuturesUnordered};
use komodo_client::entities::config::periphery::CliArgs;
use tracing::Instrument;

use crate::{config::periphery_args, state::periphery_secret_key};

#[macro_use]
extern crate tracing;

mod api;
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
