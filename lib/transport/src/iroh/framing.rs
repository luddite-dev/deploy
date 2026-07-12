use anyhow::anyhow;
use encoding::CastBytes;
use periphery_client::transport::EncodedTransportMessage;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024; // 16 MiB

/// Writes length-prefixed `EncodedTransportMessage` values to an async sink.
/// Each message is framed as: 4-byte big-endian length + payload bytes.
pub struct FramedWriter<W> {
  inner: W,
}

impl<W: AsyncWrite + Unpin> FramedWriter<W> {
  pub fn new(inner: W) -> Self {
    Self { inner }
  }

  pub async fn write_message(
    &mut self,
    message: &EncodedTransportMessage,
  ) -> anyhow::Result<()> {
    let bytes = message.clone().into_vec();
    let len = bytes.len() as u32;
    self.inner.write_all(&len.to_be_bytes()).await?;
    self.inner.write_all(&bytes).await?;
    self.inner.flush().await?;
    Ok(())
  }

  pub fn into_inner(self) -> W {
    self.inner
  }
}

/// Reads length-prefixed `EncodedTransportMessage` values from an async source.
/// Each message is framed as: 4-byte big-endian length + payload bytes.
pub struct FramedReader<R> {
  inner: R,
}

impl<R: AsyncRead + Unpin> FramedReader<R> {
  pub fn new(inner: R) -> Self {
    Self { inner }
  }

  pub async fn read_message(
    &mut self,
  ) -> anyhow::Result<EncodedTransportMessage> {
    let mut len_buf = [0u8; 4];
    self.inner.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE {
      return Err(anyhow!("Message too large: {len} bytes"));
    }
    let mut buf = vec![0u8; len];
    self.inner.read_exact(&mut buf).await?;
    Ok(EncodedTransportMessage::from_vec(buf))
  }

  pub fn into_inner(self) -> R {
    self.inner
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn test_framing_round_trip() {
    let (client, server) = tokio::io::duplex(1024);
    let mut writer = FramedWriter::new(client);
    let mut reader = FramedReader::new(server);

    let original = EncodedTransportMessage::from_vec(vec![42u8; 100]);
    writer.write_message(&original).await.unwrap();

    let decoded = reader.read_message().await.unwrap();
    assert_eq!(original.into_vec(), decoded.into_vec());
  }

  #[tokio::test]
  async fn test_framing_empty_message() {
    let (client, server) = tokio::io::duplex(1024);
    let mut writer = FramedWriter::new(client);
    let mut reader = FramedReader::new(server);

    let original = EncodedTransportMessage::from_vec(vec![]);
    writer.write_message(&original).await.unwrap();

    let decoded = reader.read_message().await.unwrap();
    assert_eq!(original.into_vec(), decoded.into_vec());
  }

  #[tokio::test]
  async fn test_framing_multiple_messages() {
    let (client, server) = tokio::io::duplex(1024);
    let mut writer = FramedWriter::new(client);
    let mut reader = FramedReader::new(server);

    let msgs = vec![
      EncodedTransportMessage::from_vec(vec![1, 2, 3]),
      EncodedTransportMessage::from_vec(vec![4, 5]),
      EncodedTransportMessage::from_vec(vec![]),
      EncodedTransportMessage::from_vec(vec![6, 7, 8, 9, 10]),
    ];

    for msg in &msgs {
      writer.write_message(msg).await.unwrap();
    }

    for expected in &msgs {
      let decoded = reader.read_message().await.unwrap();
      assert_eq!(expected.clone().into_vec(), decoded.into_vec());
    }
  }

  #[tokio::test]
  async fn test_read_eof_errors() {
    let data: Vec<u8> = vec![];
    let mut reader = FramedReader::new(&data[..]);
    let result = reader.read_message().await;
    assert!(result.is_err());
  }
}
