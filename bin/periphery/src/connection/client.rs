use std::{
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use anyhow::Context;
use encoding::{Decode as _, Encode as _};
use iroh::{
  Endpoint, EndpointAddr, EndpointId, endpoint::Connection,
};
use periphery_client::transport::{LoginMessage, TransportMessage};
use transport::iroh::{
  endpoint::ALPN,
  framing::{FramedReader, FramedWriter},
};

use crate::{config::periphery_config, state::core_connections};

/// Initiate an outbound Iroh connection to Komodo Core.
///
/// `core_addr` is an Iroh [`NodeAddr`] string (e.g. "node_id@relay_url").
#[instrument("StartCoreConnection", skip(endpoint))]
pub async fn handler(
  endpoint: Endpoint,
  core_addr: &str,
) -> anyhow::Result<tokio::task::JoinHandle<anyhow::Result<()>>> {
  let config = periphery_config();

  let node_addr: EndpointAddr = core_addr
    .parse::<EndpointId>()
    .with_context(|| {
      format!("Failed to parse Iroh EndpointId from '{core_addr}'")
    })?
    .into();

  let core = if config.connect_as.is_empty() {
    core_addr.to_string()
  } else {
    config.connect_as.clone()
  };

  info!("Initiating outbound Iroh connection to Core: {core_addr}");

  let channel = core_connections().get_or_insert_default(&core).await;

  let our_endpoint_id = endpoint.id().to_string();
  let onboarding_key = config.onboarding_key.clone();
  let node_addr = node_addr;
  let onboarded = Arc::new(AtomicBool::new(false));

  let handle = tokio::spawn(async move {
    let mut receiver = channel.receiver()?;
    loop {
      // After a successful onboarding, switch to EndpointId login
      // for all future reconnections.
      let use_onboarding = onboarding_key.is_some()
        && !onboarded.load(Ordering::Relaxed);

      let conn = match connect_to_core(&endpoint, &node_addr).await {
        Ok(conn) => conn,
        Err(e) => {
          warn!("Failed to connect to Core | {e:#}");
          tokio::time::sleep(Duration::from_secs(
            periphery_client::CONNECTION_RETRY_SECONDS,
          ))
          .await;
          continue;
        }
      };

      // Open bidi stream for login + data
      let (send, recv) = match conn.open_bi().await {
        Ok(streams) => streams,
        Err(e) => {
          warn!("Failed to open bidi stream to Core | {e:#}");
          tokio::time::sleep(Duration::from_secs(
            periphery_client::CONNECTION_RETRY_SECONDS,
          ))
          .await;
          continue;
        }
      };

      let mut writer = FramedWriter::new(send);
      let mut reader = FramedReader::new(recv);

      // Send login message(s).
      // If we have an onboarding key and haven't onboarded yet, send
      // OnboardingToken followed by our EndpointId so Core can register us.
      // After onboarding, send EndpointId for normal reconnection.
      if use_onboarding {
        let token = onboarding_key.as_ref().unwrap();
        if let Err(e) = writer
          .write_message(
            &LoginMessage::OnboardingToken(token.clone()).encode(),
          )
          .await
        {
          warn!("Failed to send onboarding token | {e:#}");
          tokio::time::sleep(Duration::from_secs(
            periphery_client::CONNECTION_RETRY_SECONDS,
          ))
          .await;
          continue;
        }
        // Send ConnectAs with the desired server name.
        // Core will use the Iroh connection's remote_id for the endpoint_id.
        if let Err(e) = writer
          .write_message(
            &LoginMessage::ConnectAs(config.connect_as.clone())
              .encode(),
          )
          .await
        {
          warn!("Failed to send ConnectAs | {e:#}");
          tokio::time::sleep(Duration::from_secs(
            periphery_client::CONNECTION_RETRY_SECONDS,
          ))
          .await;
          continue;
        }
      } else {
        if let Err(e) = writer
          .write_message(
            &LoginMessage::EndpointId(our_endpoint_id.clone())
              .encode(),
          )
          .await
        {
          warn!("Failed to send login message | {e:#}");
          tokio::time::sleep(Duration::from_secs(
            periphery_client::CONNECTION_RETRY_SECONDS,
          ))
          .await;
          continue;
        }
      }

      // Wait for login success
      let success = match reader.read_message().await {
        Ok(msg) => match msg.decode() {
          Ok(TransportMessage::Login(encoded)) => {
            match encoded.decode() {
              Ok(LoginMessage::Success) => true,
              Ok(other) => {
                warn!(
                  "Received unexpected login response: {other:?}"
                );
                false
              }
              Err(e) => {
                warn!("Failed to decode login response | {e:#}");
                false
              }
            }
          }
          Ok(other) => {
            warn!("Expected login response, got {other:?}");
            false
          }
          Err(e) => {
            warn!("Failed to decode transport message | {e:#}");
            false
          }
        },
        Err(e) => {
          warn!("Failed to read login response | {e:#}");
          // If connection was dropped and we were using onboarding,
          // the onboarding might have failed. Retry.
          false
        }
      };

      if !success {
        tokio::time::sleep(Duration::from_secs(
          periphery_client::CONNECTION_RETRY_SECONDS,
        ))
        .await;
        continue;
      }

      // Onboarding succeeded — mark it so we use EndpointId next time
      if use_onboarding {
        onboarded.store(true, Ordering::Relaxed);
      }

      info!("Login to Core successful");

      // Handle the socket
      let send = writer.into_inner();
      let recv = reader.into_inner();
      super::handle_socket(
        send,
        recv,
        &core,
        &channel.sender,
        &mut receiver,
      )
      .await;

      // When handle_socket returns, the connection was dropped.
      // Retry the connection after a delay.
      tokio::time::sleep(Duration::from_secs(
        periphery_client::CONNECTION_RETRY_SECONDS,
      ))
      .await;
    }
  });

  Ok(handle)
}

async fn connect_to_core(
  endpoint: &Endpoint,
  node_addr: &EndpointAddr,
) -> anyhow::Result<Connection> {
  endpoint
    .connect(node_addr.clone(), ALPN)
    .await
    .context("Failed to connect to Core Iroh endpoint")
}
