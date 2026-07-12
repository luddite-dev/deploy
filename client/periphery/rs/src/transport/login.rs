use anyhow::{Context, anyhow};
use encoding::{
  CastBytes, Decode, Encode, EncodedResponse, impl_cast_bytes_vec,
  impl_from_for_wrapper,
};
use strum::EnumDiscriminants;

use crate::transport::{EncodedTransportMessage, TransportMessage};

#[derive(Debug)]
pub struct EncodedLoginMessage(
  EncodedResponse<InnerEncodedLoginMessage>,
);

impl_from_for_wrapper!(
  EncodedLoginMessage,
  EncodedResponse<InnerEncodedLoginMessage>
);
impl_cast_bytes_vec!(EncodedLoginMessage, EncodedResponse);

/// ```markdown
/// | -- u8[] -- | --------- u8 ------------ |
/// | <CONTENTS> | LoginMessageVariant |
/// ```
#[derive(Clone, Debug)]
pub struct InnerEncodedLoginMessage(Vec<u8>);

impl_cast_bytes_vec!(InnerEncodedLoginMessage, Vec);

/// Login messages for the Iroh transport.
///
/// The first message on any new bidi stream must be a `LoginMessage`:
/// - `OnboardingToken` for new nodes (validated against `onboarding_keys` DB).
/// - `EndpointId` for known nodes reconnecting (validated against Server allowlist).
///
/// Core responds with `Success` to accept the connection.
#[derive(Clone, EnumDiscriminants)]
#[strum_discriminants(name(LoginMessageVariant))]
pub enum LoginMessage {
  /// Bearer token for onboarding a new Periphery node.
  /// Sent as the first message on a new connection.
  OnboardingToken(String),
  /// The sender's Iroh EndpointId.
  /// Sent as the first message when reconnecting a known node.
  EndpointId(String),
  /// Login completed successfully.
  Success,
}

impl Encode<EncodedTransportMessage> for LoginMessage {
  fn encode(self) -> EncodedTransportMessage {
    let variant: LoginMessageVariant = (&self).into();
    let variant_byte = variant.as_byte();
    let mut bytes = match self {
      LoginMessage::Success => Vec::new(),
      LoginMessage::OnboardingToken(token) => token.into_bytes(),
      LoginMessage::EndpointId(id) => id.into_bytes(),
    };
    bytes.push(variant_byte);
    let inner = InnerEncodedLoginMessage(bytes);
    let res = Ok(inner).encode();
    TransportMessage::Login(EncodedLoginMessage(res)).encode()
  }
}

impl Decode<LoginMessage> for EncodedLoginMessage {
  fn decode(self) -> anyhow::Result<LoginMessage> {
    let mut bytes = self
      .0
      .decode()?
      .context("Should not receive Pending (2) Response message")?
      .into_vec();

    let variant_byte = bytes
      .pop()
      .context("Failed to parse login message | Bytes are empty")?;

    let variant = LoginMessageVariant::from_byte(variant_byte)?;

    use LoginMessageVariant::*;
    let message = match variant {
      Success => LoginMessage::Success,
      OnboardingToken => {
        let token = String::from_utf8(bytes)
          .context("Onboarding token is not valid utf-8")?;
        LoginMessage::OnboardingToken(token)
      }
      EndpointId => {
        let id = String::from_utf8(bytes)
          .context("EndpointId is not valid utf-8")?;
        LoginMessage::EndpointId(id)
      }
    };

    Ok(message)
  }
}

impl LoginMessageVariant {
  pub fn from_byte(byte: u8) -> anyhow::Result<Self> {
    use LoginMessageVariant::*;
    let variant = match byte {
      0 => OnboardingToken,
      1 => EndpointId,
      2 => Success,
      other => {
        return Err(anyhow!(
          "Got unrecognized LoginMessageVariant byte: {other}"
        ));
      }
    };
    Ok(variant)
  }

  pub fn as_byte(self) -> u8 {
    use LoginMessageVariant::*;
    match self {
      OnboardingToken => 0,
      EndpointId => 1,
      Success => 2,
    }
  }
}
