use komodo_client::entities::NoData;
use mogh_resolver::Resolve;
use periphery_client::api::keys::{
  RotateCorePublicKey, RotatePrivateKey, RotatePrivateKeyResponse,
};

use crate::state::periphery_secret_key;

//

impl Resolve<crate::api::Args> for RotatePrivateKey {
  #[instrument(
    "RotatePrivateKey",
    skip_all,
    fields(
      id = args.id.to_string(),
      core = args.core,
    )
  )]
  async fn resolve(
    self,
    args: &crate::api::Args,
  ) -> anyhow::Result<RotatePrivateKeyResponse> {
    // In Iroh transport mode, the secret key is at a fixed file path
    // and does not support rotation via the old PKI mechanism.
    let endpoint_id = periphery_secret_key().public().to_string();
    info!("Iroh EndpointId: {endpoint_id}");
    Ok(RotatePrivateKeyResponse {
      public_key: endpoint_id,
    })
  }
}

//

impl Resolve<crate::api::Args> for RotateCorePublicKey {
  #[instrument(
    "RotateCorePublicKey",
    skip_all,
    fields(
      id = args.id.to_string(),
      core = args.core,
      public_key = self.public_key,
    )
  )]
  async fn resolve(
    self,
    args: &crate::api::Args,
  ) -> anyhow::Result<NoData> {
    // In Iroh transport mode, there is no core public key to rotate.
    // Core authentication is handled by the Iroh endpoint identity.
    Ok(NoData {})
  }
}
