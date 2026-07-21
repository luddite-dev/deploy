use std::{
  collections::HashMap,
  sync::{Arc, OnceLock},
};

use anyhow::{Context, anyhow};
use arc_swap::ArcSwap;
use komodo_client::entities::{
  docker::container::ContainerStats, terminal::TerminalStdinMessage,
};
use mogh_cache::{CloneCache, CloneVecCache};
use periphery_client::transport::EncodedTransportMessage;
use tokio::sync::{Mutex, OnceCell, RwLock, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use transport::channel::BufferedChannel;
use uuid::Uuid;

use crate::{
  config::periphery_config,
  docker::DockerClient,
  helpers::{resolve_host_public_ipv4, resolve_host_public_ipv6},
  stats::StatsClient,
  terminal::PeripheryTerminal,
};

/// Load the Iroh secret key for Periphery.
/// If the key file does not exist, generates and persists a new one.
pub fn periphery_secret_key() -> &'static iroh::SecretKey {
  static PERIPHERY_SECRET_KEY: OnceLock<iroh::SecretKey> =
    OnceLock::new();
  PERIPHERY_SECRET_KEY.get_or_init(|| {
    let config = periphery_config();
    let spec = config.iroh_secret_key.as_deref();
    let path = if let Some(stripped) =
      spec.and_then(|s| s.strip_prefix("file:"))
    {
      stripped.to_string()
    } else {
      config
        .root_directory
        .join("keys/iroh.key")
        .to_str()
        .expect("Invalid root directory")
        .to_string()
    };
    transport::iroh::secret::load_secret_key(&path)
      .expect("Failed to load Periphery Iroh secret key")
  })
}

/// Core Address / Host -> Channel
pub type CoreConnection = BufferedChannel<EncodedTransportMessage>;
pub type CoreConnections = CloneCache<String, Arc<CoreConnection>>;

pub fn core_connections() -> &'static CoreConnections {
  static CORE_CONNECTIONS: OnceLock<CoreConnections> =
    OnceLock::new();
  CORE_CONNECTIONS.get_or_init(Default::default)
}

pub fn stats_client() -> &'static RwLock<StatsClient> {
  static STATS_CLIENT: OnceLock<RwLock<StatsClient>> =
    OnceLock::new();
  STATS_CLIENT.get_or_init(|| RwLock::new(StatsClient::default()))
}

pub fn terminals() -> &'static CloneVecCache<Arc<PeripheryTerminal>> {
  static TERMINALS: OnceLock<CloneVecCache<Arc<PeripheryTerminal>>> =
    OnceLock::new();
  TERMINALS.get_or_init(Default::default)
}

#[derive(Default)]
pub struct TerminalChannels(CloneCache<Uuid, Arc<TerminalChannel>>);

impl TerminalChannels {
  pub async fn get(
    &self,
    channel: &Uuid,
  ) -> Option<Arc<TerminalChannel>> {
    self.0.get(channel).await
  }

  pub async fn insert(
    &self,
    channel: Uuid,
    terminal: Arc<TerminalChannel>,
  ) -> Option<Arc<TerminalChannel>> {
    self.0.insert(channel, terminal).await
  }

  pub async fn remove(&self, channel: &Uuid) {
    let Some(channel) = self.0.remove(channel).await else {
      return;
    };
    channel.cancel.cancel();
  }
}

pub fn terminal_channels() -> &'static TerminalChannels {
  static TERMINAL_CHANNELS: OnceLock<TerminalChannels> =
    OnceLock::new();
  TERMINAL_CHANNELS.get_or_init(Default::default)
}

#[derive(Debug)]
pub struct TerminalChannel {
  pub sender: mpsc::Sender<TerminalStdinMessage>,
  pub cancel: CancellationToken,
}

pub fn terminal_triggers() -> &'static TerminalTriggers {
  static TERMINAL_TRIGGERS: OnceLock<TerminalTriggers> =
    OnceLock::new();
  TERMINAL_TRIGGERS.get_or_init(Default::default)
}

/// Periphery must wait for Core to finish setting
/// up channel forwarding before sending message,
/// or the first sent messages may be missed.
#[derive(Default)]
pub struct TerminalTriggers(CloneCache<Uuid, Arc<TerminalTrigger>>);

impl TerminalTriggers {
  #[instrument("InsertTerminalTrigger", skip(self))]
  pub async fn insert(&self, channel: Uuid) {
    let (sender, receiver) = oneshot::channel();
    let trigger = Arc::new(TerminalTrigger {
      sender: Some(sender).into(),
      receiver: Some(receiver).into(),
    });
    self.0.insert(channel, trigger).await;
  }

  pub async fn send(&self, channel: &Uuid) -> anyhow::Result<()> {
    let trigger = self.0.get(channel).await.with_context(|| {
      format!("No trigger found for channel {channel}")
    })?;
    trigger.send().await
  }

  pub async fn recv(&self, channel: &Uuid) -> anyhow::Result<()> {
    let trigger = self.0.get(channel).await.with_context(|| {
      format!("No trigger found for channel {channel}")
    })?;
    trigger.wait().await?;
    self.0.remove(channel).await;
    Ok(())
  }
}

#[derive(Debug)]
pub struct TerminalTrigger {
  sender: Mutex<Option<oneshot::Sender<()>>>,
  receiver: Mutex<Option<oneshot::Receiver<()>>>,
}

impl TerminalTrigger {
  /// This consumes the Trigger Sender.
  pub async fn send(&self) -> anyhow::Result<()> {
    let mut sender = self.sender.lock().await;
    let sender = sender
      .take()
      .context("Called TerminalTrigger 'send' more than once.")?;
    sender
      .send(())
      .map_err(|_| anyhow!("TerminalTrigger sender already used"))
  }

  /// This consumes the Trigger Receiver.
  pub async fn wait(&self) -> anyhow::Result<()> {
    let mut receiver = self.receiver.lock().await;
    let receiver = receiver
      .take()
      .context("Called TerminalTrigger 'wait' more than once.")?;
    receiver.await.context("Failed to receive TerminalTrigger")
  }
}

pub fn docker_client() -> &'static SwappableDockerClient {
  static DOCKER_CLIENT: OnceLock<SwappableDockerClient> =
    OnceLock::new();
  DOCKER_CLIENT.get_or_init(SwappableDockerClient::init)
}

#[derive(Default)]
pub struct SwappableDockerClient(ArcSwap<Option<DockerClient>>);

impl SwappableDockerClient {
  pub fn init() -> Self {
    let docker = DockerClient::connect()
      // Only logs on first init, although keeps trying to connect
      .inspect_err(|e| warn!("{e:#}"))
      .ok();
    Self(ArcSwap::new(Arc::new(docker)))
  }

  pub fn load(&self) -> arc_swap::Guard<Arc<Option<DockerClient>>> {
    let res = self.0.load();
    if res.is_some() {
      return res;
    }
    self.reload();
    self.0.load()
  }

  pub fn reload(&self) {
    self.0.store(Arc::new(DockerClient::connect().ok()));
  }
}

pub type ContainerStatsMap = HashMap<String, ContainerStats>;

pub fn container_stats() -> &'static ArcSwap<ContainerStatsMap> {
  static CONTAINER_STATS: OnceLock<ArcSwap<ContainerStatsMap>> =
    OnceLock::new();
  CONTAINER_STATS.get_or_init(Default::default)
}

pub async fn host_public_ipv4() -> Option<&'static String> {
  static PUBLIC_IPV4: OnceCell<Option<String>> =
    OnceCell::const_new();
  PUBLIC_IPV4
    .get_or_init(|| async {
      // Override from config takes precedence — skip discovery if set.
      let cfg = periphery_config();
      if let Some(v4) = cfg.public_ipv4.clone() {
        return Some(v4);
      }
      resolve_host_public_ipv4().await.or_else(|| {
        warn!("Failed to resolve host public IPv4 via ipify");
        None
      })
    })
    .await
    .as_ref()
}

pub async fn host_public_ipv6() -> Option<&'static String> {
  static PUBLIC_IPV6: OnceCell<Option<String>> =
    OnceCell::const_new();
  PUBLIC_IPV6
    .get_or_init(|| async {
      // Override from config takes precedence — skip discovery if set.
      let cfg = periphery_config();
      if let Some(v6) = cfg.public_ipv6.clone() {
        return Some(v6);
      }
      resolve_host_public_ipv6().await.or_else(|| {
        warn!("Failed to resolve host public IPv6 via ipify");
        None
      })
    })
    .await
    .as_ref()
}

type CancelCache = CloneCache<String, CancellationToken>;

/// Maps build id => CancellationToken
pub fn build_cancel_cache() -> &'static CancelCache {
  static BUILD_CANCEL_CACHE: OnceLock<CancelCache> = OnceLock::new();
  BUILD_CANCEL_CACHE.get_or_init(Default::default)
}
