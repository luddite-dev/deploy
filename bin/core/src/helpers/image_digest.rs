use std::{sync::Arc, time::Duration};

use komodo_client::entities::{ImageDigest, komodo_timestamp, server::Server};
use mogh_cache::CloneCache;
use periphery_client::api::docker::GetLatestImageDigest;

use crate::helpers::periphery_client;

/// Maps images -> (digest, valid until milliseconds)
pub struct ImageDigestCache(CloneCache<String, (ImageDigest, i64)>);

impl ImageDigestCache {
  /// Also spawns a task to periodically clean up expired image digests.
  pub fn new() -> Arc<ImageDigestCache> {
    let cache = Arc::new(ImageDigestCache(Default::default()));
    let clone = cache.clone();
    tokio::spawn(async move {
      let mut interval =
        tokio::time::interval(Duration::from_secs(60 * 60));
      interval.tick().await;
      loop {
        interval.tick().await;
        let ts = komodo_timestamp();
        clone
          .0
          .retain(|_, (_, valid_until)| *valid_until > ts)
          .await;
      }
    });
    cache
  }

  pub async fn get(
    &self,
    server: &Server,
    image: &String,
    account: Option<String>,
    token: Option<String>,
  ) -> anyhow::Result<ImageDigest> {
    if let Some((digest, valid_until)) = self.0.get(image).await
      // Ensure the query time was within last 10 mins to use cache.
      && valid_until > komodo_timestamp()
    {
      return Ok(digest);
    }

    let digest = periphery_client(server)
      .await?
      .request(GetLatestImageDigest {
        name: image.clone(),
        account,
        token,
      })
      .await?
      .digest;

    let digest = ImageDigest::new(image, &digest);

    self
      .0
      .insert(
        image,
        (digest.clone(), komodo_timestamp() + 10 * 60 * 1_000),
      )
      .await;

    Ok(digest)
  }
}
