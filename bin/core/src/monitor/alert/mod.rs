use std::{collections::HashMap, sync::Mutex};

use anyhow::Context;
use komodo_client::entities::{
  alert::AlertDataVariant, permission::PermissionLevel,
  resource::ResourceQuery, server::Server, user::system_user,
};

use crate::resource;

mod deployment;
mod server;
mod stack;

// called after cache update
pub async fn check_alerts(ts: i64) {
  let server = get_all_servers_map().await;

  let (servers, server_names) =
    server.inspect_err(|e| error!("{e:#}")).unwrap_or_default();

  tokio::join!(
    server::alert_servers(ts, servers),
    deployment::alert_deployments(ts, &server_names),
    stack::alert_stacks(ts, &server_names)
  );
}

async fn get_all_servers_map()
-> anyhow::Result<(HashMap<String, Server>, HashMap<String, String>)>
{
  let servers = resource::list_full_for_user::<Server>(
    ResourceQuery::default(),
    system_user(),
    PermissionLevel::Read.into(),
    &[],
  )
  .await
  .context("failed to get servers from db (in alert_servers)")?;

  let servers = servers
    .into_iter()
    .map(|server| (server.id.clone(), server))
    .collect::<HashMap<_, _>>();

  let server_names = servers
    .iter()
    .map(|(id, server)| (id.clone(), server.name.clone()))
    .collect::<HashMap<_, _>>();

  Ok((servers, server_names))
}

/// Alert buffer to prevent immediate alerts on transient issues
struct AlertBuffer {
  buffer: Mutex<HashMap<(String, AlertDataVariant), bool>>,
}

impl AlertBuffer {
  fn new() -> Self {
    Self {
      buffer: Mutex::new(HashMap::new()),
    }
  }

  /// Check if alert should be opened. Requires two consecutive calls to return true.
  fn ready_to_open(
    &self,
    server_id: String,
    variant: AlertDataVariant,
  ) -> bool {
    let mut lock = self.buffer.lock().unwrap();
    let ready = lock.entry((server_id, variant)).or_default();
    if *ready {
      *ready = false;
      true
    } else {
      *ready = true;
      false
    }
  }

  /// Reset buffer state for a specific server/alert combination
  fn reset(&self, server_id: String, variant: AlertDataVariant) {
    let mut lock = self.buffer.lock().unwrap();
    lock.remove(&(server_id, variant));
  }
}
