use std::time::Duration;

use encoding::{
  CastBytes as _, Decode as _, Encode as _, WithChannel,
};
use iroh::endpoint::{RecvStream, SendStream};
use mogh_resolver::Resolve;
use periphery_client::transport::{
  EncodedRequestMessage, EncodedTransportMessage, RequestMessage,
  TransportMessage,
};
use transport::{
  channel::{BufferedReceiver, Sender},
  iroh::framing::{FramedReader, FramedWriter},
};

use crate::{
  api::{Args, PeripheryRequest},
  config::periphery_config,
};

pub mod client;

/// Handle an Iroh bidi stream (send, recv) for the lifetime of the connection
/// to Core.
async fn handle_socket(
  send: SendStream,
  recv: RecvStream,
  core: &str,
  sender: &Sender<EncodedTransportMessage>,
  receiver: &mut BufferedReceiver<EncodedTransportMessage>,
) {
  let config = periphery_config();
  info!(
    "Connected to Komodo Core {core}{}",
    if !config.connect_as.is_empty() {
      format!(" as Server {}", config.connect_as)
    } else {
      String::new()
    }
  );

  let mut writer = FramedWriter::new(send);
  let mut reader = FramedReader::new(recv);

  let forward_writes = async {
    loop {
      let message = match receiver.recv().await {
        Ok(message) => message,
        Err(_) => break,
      };
      if let Err(e) = writer.write_message(&message).await {
        warn!("Failed to send response | {e:?}");
        break;
      }
      receiver.clear_buffer();
    }
  };

  let handle_reads = async {
    loop {
      match reader.read_message().await {
        Ok(message) => match message.decode() {
          Ok(TransportMessage::Request(message)) => {
            handle_request(core.to_string(), sender.clone(), message);
          }
          Ok(TransportMessage::Terminal(message)) => {
            crate::terminal::handle_message(message).await;
          }
          Ok(other) => {
            warn!(
              "Received unexpected transport message | {other:?}"
            );
          }
          Err(e) => {
            warn!("Failed to decode transport message | {e:#}");
          }
        },
        Err(e) => {
          warn!("{e:#}");
          break;
        }
      }
    }
  };

  tokio::select! {
    _ = forward_writes => {},
    _ = handle_reads => {},
  }
}

fn handle_request(
  core: String,
  sender: Sender<EncodedTransportMessage>,
  message: EncodedRequestMessage,
) {
  tokio::spawn(async move {
    let WithChannel {
      channel,
      data: request,
    }: WithChannel<PeripheryRequest> =
      match message.decode().and_then(RequestMessage::map_decode) {
        Ok(res) => res,
        Err(e) => {
          // TODO: handle:
          warn!("Failed to parse Request bytes | {e:#}");
          return;
        }
      };

    let resolve_response = async {
      let response =
        match request.resolve(&Args { core, id: channel }).await {
          Ok(res) => res,
          Err(e) => (&e).encode(),
        };
      if let Err(e) = sender.send_response(channel, response).await {
        error!("Failed to send response over channel | {e:?}");
      }
    };

    let ping_in_progress = async {
      loop {
        tokio::time::sleep(Duration::from_secs(4)).await;
        if let Err(e) = sender.send_in_progress(channel).await {
          error!("Failed to ping in progress over channel | {e:?}");
        }
      }
    };

    tokio::select! {
      _ = resolve_response => {},
      _ = ping_in_progress => {},
    }
  });
}
