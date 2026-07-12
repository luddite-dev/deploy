use std::{
  sync::{
    Arc,
    atomic::{self, AtomicBool},
  },
  time::Duration,
};

use anyhow::anyhow;
use database::mungos::{by_id::update_one_by_id, mongodb::bson::doc};
use encoding::{
  CastBytes as _, Decode as _, EncodedJsonMessage, EncodedResponse,
  WithChannel,
};
use iroh::endpoint::{RecvStream, SendStream};
use komodo_client::entities::{optional_str, server::Server};
use mogh_cache::CloneCache;
use periphery_client::transport::{
  EncodedTransportMessage, ResponseMessage, TransportMessage,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use transport::{
  channel::{BufferedReceiver, Sender, buffered_channel},
  iroh::framing::{FramedReader, FramedWriter},
};
use uuid::Uuid;

use crate::state::db_client;

pub mod server;

#[derive(Default)]
pub struct PeripheryConnections(
  CloneCache<String, Arc<PeripheryConnection>>,
);

impl PeripheryConnections {
  /// Insert a recreated connection.
  pub async fn insert(
    &self,
    server_id: String,
    args: PeripheryConnectionArgs<'_>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    let (connection, receiver) = if let Some(existing_connection) =
      self.0.remove(&server_id).await
    {
      existing_connection.with_new_args(args)
    } else {
      PeripheryConnection::new(args)
    };

    self.0.insert(server_id, connection.clone()).await;

    (connection, receiver)
  }

  pub async fn get(
    &self,
    server_id: &String,
  ) -> Option<Arc<PeripheryConnection>> {
    self.0.get(server_id).await
  }

  /// Remove and cancel connection
  pub async fn remove(
    &self,
    server_id: &String,
  ) -> Option<Arc<PeripheryConnection>> {
    self
      .0
      .remove(server_id)
      .await
      .inspect(|connection| connection.cancel())
  }
}

/// The configurable args of a connection
#[derive(Debug, Clone, PartialEq)]
pub struct PeripheryConnectionArgs<'a> {
  /// Usually the server id
  pub id: &'a str,
  pub endpoint_id: Option<&'a str>,
}

impl<'a> PeripheryConnectionArgs<'a> {
  pub fn from_server(server: &'a Server) -> Self {
    Self {
      id: &server.id,
      endpoint_id: optional_str(&server.info.endpoint_id),
    }
  }

  pub fn to_owned(self) -> OwnedPeripheryConnectionArgs {
    OwnedPeripheryConnectionArgs {
      id: self.id.to_string(),
      endpoint_id: self.endpoint_id.map(str::to_string),
    }
  }

  pub fn matches<'b>(
    self,
    args: impl Into<PeripheryConnectionArgs<'b>>,
  ) -> bool {
    self == args.into()
  }
}

#[derive(Debug, Clone)]
pub struct OwnedPeripheryConnectionArgs {
  /// Usually the Server id.
  pub id: String,
  /// The Iroh EndpointId expected from Periphery.
  pub endpoint_id: Option<String>,
}

impl OwnedPeripheryConnectionArgs {
  pub fn borrow(&self) -> PeripheryConnectionArgs<'_> {
    PeripheryConnectionArgs {
      id: &self.id,
      endpoint_id: self.endpoint_id.as_deref(),
    }
  }
}

impl From<PeripheryConnectionArgs<'_>>
  for OwnedPeripheryConnectionArgs
{
  fn from(value: PeripheryConnectionArgs<'_>) -> Self {
    value.to_owned()
  }
}

impl<'a> From<&'a OwnedPeripheryConnectionArgs>
  for PeripheryConnectionArgs<'a>
{
  fn from(value: &'a OwnedPeripheryConnectionArgs) -> Self {
    value.borrow()
  }
}

/// Sends None as InProgress ping.
pub type ResponseChannels =
  CloneCache<Uuid, Sender<EncodedResponse<EncodedJsonMessage>>>;

pub type TerminalChannels =
  CloneCache<Uuid, Sender<anyhow::Result<Vec<u8>>>>;

#[derive(Debug)]
pub struct PeripheryConnection {
  /// The connection args
  pub args: OwnedPeripheryConnectionArgs,
  /// Send and receive bytes over the connection socket.
  pub sender: Sender<EncodedTransportMessage>,
  /// Cancel the connection
  pub cancel: CancellationToken,
  /// Whether Periphery is currently connected.
  pub connected: AtomicBool,
  // These fields must be maintained if new connection replaces old
  // at the same server id.
  /// Stores latest connection error
  pub error: Arc<RwLock<Option<mogh_error::Serror>>>,
  /// Forward bytes from Periphery to response channel handlers.
  pub responses: Arc<ResponseChannels>,
  /// Forward bytes from Periphery to terminal channel handlers.
  pub terminals: Arc<TerminalChannels>,
}

impl PeripheryConnection {
  pub fn new(
    args: impl Into<OwnedPeripheryConnectionArgs>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    let (sender, receiever) = buffered_channel();
    (
      PeripheryConnection {
        sender,
        args: args.into(),
        cancel: CancellationToken::new(),
        connected: AtomicBool::new(false),
        error: Default::default(),
        responses: Default::default(),
        terminals: Default::default(),
      }
      .into(),
      receiever,
    )
  }

  pub fn with_new_args(
    &self,
    args: impl Into<OwnedPeripheryConnectionArgs>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    // Ensure this connection is cancelled.
    self.cancel();
    let (sender, receiever) = buffered_channel();
    (
      PeripheryConnection {
        sender,
        args: args.into(),
        cancel: CancellationToken::new(),
        connected: AtomicBool::new(false),
        error: self.error.clone(),
        responses: self.responses.clone(),
        terminals: self.terminals.clone(),
      }
      .into(),
      receiever,
    )
  }

  /// Handle an Iroh bidi stream (send, recv) for the lifetime of the connection.
  pub async fn handle_socket(
    &self,
    send: SendStream,
    recv: RecvStream,
    receiver: &mut BufferedReceiver<EncodedTransportMessage>,
  ) {
    let cancel = self.cancel.child_token();

    self.set_connected(true);
    self.clear_error().await;

    let mut writer = FramedWriter::new(send);
    let mut reader = FramedReader::new(recv);

    let _cancel_guard = cancel.clone();

    let forward_writes = async {
      loop {
        tokio::select! {
          message = receiver.recv() => {
            match message {
              Ok(message) => {
                if let Err(e) = writer.write_message(&message).await {
                  self.set_error(e).await;
                  break;
                }
                receiver.clear_buffer();
              }
              Err(_) => break,
            }
          }
          _ = cancel.cancelled() => break,
        }
      }
    };

    let handle_reads = async {
      loop {
        match reader.read_message().await {
          Ok(message) => {
            let encoded = message.into_vec();
            let decoded = EncodedTransportMessage::from_vec(encoded);
            match decoded.decode() {
              Ok(transport_msg) => {
                self.handle_incoming_message(transport_msg).await;
              }
              Err(e) => {
                warn!("Failed to decode transport message | {e:#}");
              }
            }
          }
          Err(e) => {
            self.set_error(e).await;
            break;
          }
        }
      }
    };

    tokio::select! {
      _ = forward_writes => {},
      _ = handle_reads => {},
    }

    self.set_connected(false);
  }

  pub async fn handle_incoming_message(
    &self,
    message: TransportMessage,
  ) {
    match message {
      TransportMessage::Response(data) => {
        match data.decode().map(ResponseMessage::into_inner) {
          Ok(WithChannel { channel, data }) => {
            let Some(response_channel) =
              self.responses.get(&channel).await
            else {
              warn!(
                "Failed to forward Response message | No response channel found at {channel}"
              );
              return;
            };
            if let Err(e) = response_channel.send(data).await {
              warn!(
                "Failed to forward Response | Response channel failure at {channel} | {e:#}"
              );
            }
          }
          Err(e) => {
            warn!("Failed to read Response message | {e:#}");
          }
        }
      }
      TransportMessage::Terminal(data) => match data.decode() {
        Ok(WithChannel {
          channel: channel_id,
          data,
        }) => {
          let Some(channel) = self.terminals.get(&channel_id).await
          else {
            warn!(
              "Failed to forward Terminal message | No terminal channel found at {channel_id}"
            );
            return;
          };
          if let Err(e) = channel.send(data).await {
            warn!(
              "Failed to forward Terminal message | Channel failure at {channel_id} | {e:#}"
            );
          }
        }
        Err(e) => {
          warn!("Failed to read Terminal message | {e:#}");
        }
      },
      //
      other => {
        warn!("Received unexpected transport message | {other:?}");
      }
    }
  }

  pub fn set_connected(&self, connected: bool) {
    self.connected.store(connected, atomic::Ordering::Relaxed);
  }

  pub fn connected(&self) -> bool {
    self.connected.load(atomic::Ordering::Relaxed)
  }

  /// Polls connected 3 times (500ms in between) before bailing.
  pub async fn bail_if_not_connected(&self) -> anyhow::Result<()> {
    const POLL_TIMES: usize = 3;
    for i in 0..POLL_TIMES {
      if self.connected() {
        return Ok(());
      }
      if i < POLL_TIMES - 1 {
        tokio::time::sleep(Duration::from_millis(500)).await;
      }
    }
    if let Some(e) = self.error().await {
      Err(mogh_error::serror_into_anyhow_error(e))
    } else {
      Err(anyhow!("Server is not currently connected"))
    }
  }

  pub async fn error(&self) -> Option<mogh_error::Serror> {
    self.error.read().await.clone()
  }

  pub async fn set_error(&self, e: anyhow::Error) {
    let mut error = self.error.write().await;
    *error = Some(e.into());
  }

  pub async fn clear_error(&self) {
    let mut error = self.error.write().await;
    *error = None;
  }

  pub fn cancel(&self) {
    self.cancel.cancel();
  }
}

/// Spawn task to set the 'attempted_endpoint_id'
/// for easy manual connection acceptance later on.
fn spawn_update_attempted_endpoint_id(
  id: String,
  endpoint_id: impl Into<Option<String>>,
) {
  let endpoint_id = endpoint_id.into();
  tokio::spawn(async move {
    if let Err(e) = update_one_by_id(
      &db_client().servers,
      &id,
      doc! {
        "$set": {
          "info.attempted_endpoint_id": &endpoint_id.as_deref().unwrap_or_default(),
        }
      },
      None,
    )
    .await
    {
      warn!(
        "Failed to update attempted endpoint_id for Server {id} | {e:?}"
      );
    };
  });
}
