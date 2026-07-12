use std::time::Duration;

use anyhow::{Context, anyhow};
use database::mungos::mongodb::bson::doc;
use encoding::{Decode as _, Encode as _};
use iroh::{Endpoint, endpoint::Connection};
use komodo_client::{
  api::write::{
    CreateBuilder, CreateServer, UpdateResourceMeta, UpdateServer,
  },
  entities::{
    builder::{PartialBuilderConfig, PartialServerBuilderConfig},
    komodo_timestamp,
    onboarding_key::OnboardingKey,
    server::PartialServerConfig,
    user::system_user,
  },
};
use mogh_resolver::Resolve;
use periphery_client::transport::{LoginMessage, TransportMessage};
use transport::iroh::framing::{FramedReader, FramedWriter};

use crate::{
  api::write::WriteArgs,
  config::iroh_periphery_endpoint_ids,
  helpers::query::id_or_name_filter,
  monitor::refresh_server_cache,
  state::{db_client, periphery_connections},
};

use super::{
  PeripheryConnectionArgs, spawn_update_attempted_endpoint_id,
};

/// Run the Iroh accept loop for incoming Periphery connections.
pub async fn run_accept_loop(endpoint: Endpoint) {
  info!("Iroh accept loop started. EndpointId: {}", endpoint.id());
  loop {
    let connecting = match endpoint.accept().await {
      Some(connecting) => connecting,
      None => {
        info!("Iroh endpoint closed, stopping accept loop");
        break;
      }
    };
    let conn = match connecting.await {
      Ok(conn) => conn,
      Err(e) => {
        warn!("Failed to accept Iroh connection | {e:#}");
        continue;
      }
    };
    tokio::spawn(async move {
      if let Err(e) = handle_connection(conn).await {
        warn!("Iroh connection handler error | {e:#}");
      }
    });
  }
}

async fn handle_connection(conn: Connection) -> anyhow::Result<()> {
  let remote_endpoint_id = conn.remote_id().to_string();
  debug!(
    "Incoming Iroh connection from EndpointId: {remote_endpoint_id}"
  );

  // Accept a bidi stream for login + data
  let (send, recv) = conn
    .accept_bi()
    .await
    .context("Failed to accept bidi stream for login")?;

  let writer = FramedWriter::new(send);
  let mut reader = FramedReader::new(recv);

  // Read the first login message
  let login_msg = reader
    .read_message()
    .await
    .context("Failed to read login message")?;

  let login: LoginMessage = match login_msg.decode() {
    Ok(TransportMessage::Login(encoded)) => encoded.decode()?,
    other => {
      return Err(anyhow!("Expected login message, got {other:?}"));
    }
  };

  match login {
    LoginMessage::EndpointId(id) => {
      handle_existing_connection(
        writer,
        reader,
        id,
        remote_endpoint_id,
      )
      .await
    }
    LoginMessage::OnboardingToken(token) => {
      handle_onboarding_connection(writer, reader, token).await
    }
    LoginMessage::Success => {
      Err(anyhow!("Received unexpected LoginMessage::Success"))
    }
  }
}

async fn handle_existing_connection(
  mut writer: FramedWriter<iroh::endpoint::SendStream>,
  reader: FramedReader<iroh::endpoint::RecvStream>,
  endpoint_id: String,
  remote_endpoint_id: String,
) -> anyhow::Result<()> {
  // Validate the endpoint_id matches the remote endpoint
  if endpoint_id != remote_endpoint_id {
    spawn_update_attempted_endpoint_id(
      String::new(),
      Some(endpoint_id.clone()),
    );
    writer
      .write_message(&LoginMessage::Success.encode())
      .await?;
    return Err(anyhow!(
      "EndpointId mismatch: claimed {endpoint_id} but remote is {remote_endpoint_id}"
    ));
  }

  // Look up server by endpoint_id
  let server = db_client()
    .servers
    .find_one(doc! { "info.endpoint_id": &endpoint_id })
    .await
    .context("Failed to query database for Server by endpoint_id")?;

  let Some(server) = server else {
    // Check against the global iroh_periphery_endpoint_ids allowlist
    if let Some(allowed) = iroh_periphery_endpoint_ids() {
      if allowed.iter().any(|id| id == &endpoint_id) {
        // Accept — but no server found, just send success and handle stream
        writer
          .write_message(&LoginMessage::Success.encode())
          .await?;
        return Ok(());
      }
    }
    // Don't send Success — the periphery should detect the dropped
    // connection and retry (possibly with onboarding).
    return Err(anyhow!(
      "No server found with endpoint_id {endpoint_id}"
    ));
  };

  // Check server is enabled
  if !server.config.enabled {
    return Err(anyhow!("Server '{}' is disabled", server.name));
  }

  // Update endpoint_id on server if changed
  if server.info.endpoint_id != endpoint_id {
    let args = WriteArgs {
      user: system_user().to_owned(),
    };
    let _ = UpdateServer {
      id: server.id.clone(),
      config: PartialServerConfig {
        enabled: Some(true),
        ..Default::default()
      },
    }
    .resolve(&args)
    .await;
  }

  // Insert connection
  let (connection, mut receiver) = periphery_connections()
    .insert(
      server.id.clone(),
      PeripheryConnectionArgs::from_server(&server),
    )
    .await;

  // Send Success
  writer
    .write_message(&LoginMessage::Success.encode())
    .await
    .context("Failed to send Login Success")?;

  // Spawn cache refresh
  let server_clone = server.clone();
  tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(100)).await;
    refresh_server_cache(&server_clone, true).await;
  });

  // Handle the socket
  let send = writer.into_inner();
  let recv = reader.into_inner();
  connection.handle_socket(send, recv, &mut receiver).await;

  Ok(())
}

async fn handle_onboarding_connection(
  mut writer: FramedWriter<iroh::endpoint::SendStream>,
  reader: FramedReader<iroh::endpoint::RecvStream>,
  token: String,
) -> anyhow::Result<()> {
  // Validate the onboarding token against DB
  let onboarding_key = db_client()
    .onboarding_keys
    .find_one(doc! { "public_key": &token })
    .await
    .context("Failed to query database for onboarding keys")?
    .context("Matching onboarding key not found")?;

  if !onboarding_key.enabled
    || (onboarding_key.expires != 0
      && onboarding_key.expires <= komodo_timestamp())
  {
    return Err(anyhow!("Onboarding key is invalid"));
  }

  // The remote endpoint_id will be used as the server's endpoint_id
  // We need to get it from the connection, but we already consumed the
  // bidi stream. The endpoint_id will be received in a subsequent message
  // or we can use the onboarding key's associated data.

  // Read the endpoint_id from the next message
  let mut reader = reader;
  let endpoint_msg = reader.read_message().await?;
  let endpoint_id: String = match endpoint_msg.decode() {
    Ok(TransportMessage::Login(encoded)) => {
      match encoded.decode()? {
        LoginMessage::EndpointId(id) => id,
        _ => {
          writer
            .write_message(&LoginMessage::Success.encode())
            .await?;
          return Err(anyhow!(
            "Expected EndpointId after onboarding"
          ));
        }
      }
    }
    _ => {
      return Err(anyhow!("Expected EndpointId message"));
    }
  };

  // Create or update the server
  let server_id = match create_or_update_server(
    &onboarding_key,
    endpoint_id.clone(),
  )
  .await
  {
    Ok(server_id) => server_id,
    Err(e) => {
      return Err(e);
    }
  };

  // Mark onboarding key as used
  let _ = db_client()
    .onboarding_keys
    .update_one(
      doc! { "public_key": &onboarding_key.public_key },
      doc! { "$push": { "onboarded": &server_id } },
    )
    .await;

  // Fetch the created/updated server for the connection
  let server = db_client()
    .servers
    .find_one(id_or_name_filter(&server_id))
    .await
    .context("Failed to query database for Server by id")?
    .context("Server not found after onboarding")?;

  // Insert connection
  let (connection, mut receiver) = periphery_connections()
    .insert(
      server.id.clone(),
      PeripheryConnectionArgs::from_server(&server),
    )
    .await;

  // Send Success
  writer
    .write_message(&LoginMessage::Success.encode())
    .await
    .context("Failed to send Login Success")?;

  info!(
    "Server onboarded successfully | server_id: {server_id} | endpoint_id: {endpoint_id}"
  );

  // Spawn cache refresh
  let server_clone = server.clone();
  tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(100)).await;
    refresh_server_cache(&server_clone, true).await;
  });

  // Handle the socket — enter the data exchange loop
  let send = writer.into_inner();
  let recv = reader.into_inner();
  connection.handle_socket(send, recv, &mut receiver).await;

  Ok(())
}

async fn create_or_update_server(
  onboarding_key: &OnboardingKey,
  endpoint_id: String,
) -> anyhow::Result<String> {
  // Check if a server with this name already exists (from a prior onboarding)
  let existing = db_client()
    .servers
    .find_one(id_or_name_filter(&onboarding_key.public_key))
    .await
    .ok()
    .flatten();

  if let Some(server) = existing {
    // Server already exists — just update its endpoint_id if needed
    if server.info.endpoint_id != endpoint_id {
      let args = WriteArgs {
        user: system_user().to_owned(),
      };
      let _ = UpdateServer {
        id: server.id.clone(),
        config: PartialServerConfig {
          enabled: Some(true),
          ..Default::default()
        },
      }
      .resolve(&args)
      .await;
    }
    return Ok(server.id);
  }

  // Server doesn't exist — create it
  create_server_maybe_builder(onboarding_key, endpoint_id).await
}

async fn create_server_maybe_builder(
  onboarding_key: &OnboardingKey,
  endpoint_id: String,
) -> anyhow::Result<String> {
  let config = if onboarding_key.copy_server.is_empty() {
    PartialServerConfig {
      enabled: Some(true),
      ..Default::default()
    }
  } else {
    let config = match db_client()
      .servers
      .find_one(id_or_name_filter(&onboarding_key.copy_server))
      .await
    {
      Ok(Some(server)) => server.config,
      Ok(None) => {
        warn!(
          "Server onboarding: Failed to find Server {}",
          onboarding_key.copy_server
        );
        Default::default()
      }
      Err(e) => {
        warn!(
          "Failed to query database for onboarding key 'copy_server' | {e:?}"
        );
        Default::default()
      }
    };
    PartialServerConfig {
      enabled: Some(true),
      ..config.into()
    }
  };

  let args = WriteArgs {
    user: system_user().to_owned(),
  };

  let server = CreateServer {
    name: onboarding_key.public_key.clone(),
    config,
    public_key: Some(endpoint_id),
  }
  .resolve(&args)
  .await
  .map_err(|e| e.error)
  .context("Server onboarding flow failed at Server creation")?;

  // Don't need to fail, only warn on this
  if let Err(e) = (UpdateResourceMeta {
    target: (&server).into(),
    tags: Some(onboarding_key.tags.clone()),
    description: None,
    template: None,
  })
  .resolve(&args)
  .await
  .map_err(|e| e.error)
  .context("Server onboarding flow failed at Server creation")
  {
    warn!("{e:#}");
  };

  if onboarding_key.create_builder {
    // Don't need to fail, only warn on this
    if let Err(e) = (CreateBuilder {
      name: onboarding_key.public_key.clone(),
      config: PartialBuilderConfig::Server(
        PartialServerBuilderConfig {
          server_ids: Some(vec![server.id.clone()]),
        },
      ),
    })
    .resolve(&args)
    .await
    .map_err(|e| e.error)
    .context("Server onboarding flow failed at Builder creation")
    {
      warn!("{e:#}");
    };
  }

  Ok(server.id)
}
