pub mod cloudflare;
pub mod provider;

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::config::core_config;

use provider::DnsProvider;

/// Build the DNS provider from [CoreConfig] ingress settings.
///
/// Returns `Ok(None)` when the ingress DNS layer is not configured
/// (empty `provider` field), allowing callers to skip DNS wiring
/// entirely.
pub fn build_dns_provider() -> Result<Option<Arc<dyn DnsProvider>>> {
  let config = core_config();
  let provider_type = &config.ingress.dns.provider;
  if provider_type.is_empty() {
    return Ok(None);
  }
  match provider_type.as_str() {
    "cloudflare" => {
      let token_spec = config
        .ingress
        .dns
        .cloudflare_api_token
        .as_deref()
        .ok_or_else(|| {
          anyhow::anyhow!(
            "cloudflare_api_token not configured for ingress DNS"
          )
        })?;
      let token =
        if let Some(stripped) = token_spec.strip_prefix("file:") {
          std::fs::read_to_string(stripped)
            .with_context(|| {
              format!(
                "Failed to read cloudflare api token from {stripped}"
              )
            })?
            .trim()
            .to_string()
        } else {
          token_spec.to_string()
        };
      let provider = cloudflare::CloudflareDnsProvider::new(token)?;
      Ok(Some(Arc::new(provider)))
    }
    other => {
      Err(anyhow::anyhow!("Unknown DNS provider type: {other}"))
    }
  }
}
