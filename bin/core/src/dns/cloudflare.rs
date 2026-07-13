use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use komodo_client::entities::dns::DnsRecordType;

use super::provider::DnsProvider;

const CLOUDFLARE_API_BASE: &str =
  "https://api.cloudflare.com/client/v4";

/// Cloudflare-backed [DnsProvider]. Uses a Bearer token for auth,
/// and caches zone id resolution per-domain.
pub struct CloudflareDnsProvider {
  api_token: String,
  client: reqwest::Client,
  // domain -> zone_id cache, populated lazily on resolve_zone_id.
  zone_cache: RwLock<HashMap<String, String>>,
}

impl CloudflareDnsProvider {
  pub fn new(api_token: String) -> Result<Self> {
    Ok(Self {
      api_token,
      client: reqwest::Client::new(),
      zone_cache: RwLock::new(HashMap::new()),
    })
  }

  fn auth_bearer(&self) -> String {
    format!("Bearer {}", self.api_token)
  }

  /// Extract the inner result from a Cloudflare envelope, or
  /// produce an anyhow error from the reported errors array.
  fn unpack<T>(
    resp: CloudflareResponse<T>,
    context: &str,
  ) -> Result<T> {
    if resp.success {
      resp.result.ok_or_else(|| {
        anyhow::anyhow!(
          "Cloudflare response success but missing result: {context}"
        )
      })
    } else {
      let detail = resp
        .errors
        .iter()
        .map(|e| format!("{}: {}", e.code, e.message))
        .collect::<Vec<_>>()
        .join("; ");
      anyhow::bail!("Cloudflare {context} failed: {detail}")
    }
  }

  /// Send a request, then verify the HTTP status before
  /// attempting to deserialize the JSON body. Non-2xx responses
  /// (e.g. 502 HTML) would otherwise produce confusing
  /// deserialization errors from `.json()`.
  async fn send_and_check(
    &self,
    request: reqwest::RequestBuilder,
    context: &str,
  ) -> Result<reqwest::Response> {
    let resp = request
      .send()
      .await
      .with_context(|| format!("Failed to send {context}"))?;
    if !resp.status().is_success() {
      let status = resp.status();
      let body = resp.text().await.unwrap_or_default();
      anyhow::bail!(
        "Cloudflare API error ({context}): {status} - {body}"
      );
    }
    Ok(resp)
  }
}

#[async_trait]
impl DnsProvider for CloudflareDnsProvider {
  async fn resolve_zone_id(&self, domain: &str) -> Result<String> {
    // Check cache first (read lock).
    if let Some(zone_id) = self.zone_cache.read().await.get(domain) {
      return Ok(zone_id.clone());
    }

    let url = format!("{CLOUDFLARE_API_BASE}/zones");
    let resp = self
      .send_and_check(
        self
          .client
          .get(&url)
          .header("Authorization", self.auth_bearer())
          .query(&[("name", domain)]),
        &format!("zone resolve request for {domain}"),
      )
      .await?;
    let resp = resp
      .json::<CloudflareResponse<Vec<ZoneResult>>>()
      .await
      .with_context(|| {
        format!("Failed to parse zone resolve response for {domain}")
      })?;

    let zones =
      Self::unpack(resp, &format!("resolve zone for {domain}"))?;
    let zone = zones.into_iter().next().ok_or_else(|| {
      anyhow::anyhow!("No Cloudflare zone matched domain {domain}")
    })?;

    // Populate cache (write lock).
    self
      .zone_cache
      .write()
      .await
      .insert(domain.to_string(), zone.id.clone());
    Ok(zone.id)
  }

  async fn create_record(
    &self,
    zone_id: &str,
    record_type: DnsRecordType,
    name: &str,
    content: &str,
    ttl: u32,
  ) -> Result<String> {
    let url =
      format!("{CLOUDFLARE_API_BASE}/zones/{zone_id}/dns_records");
    let body = CreateRecordBody {
      record_type: record_type.as_str(),
      name: name.to_string(),
      content: content.to_string(),
      ttl,
      proxied: false,
    };
    let resp = self
      .send_and_check(
        self
          .client
          .post(&url)
          .header("Authorization", self.auth_bearer())
          .json(&body),
        &format!("create record request for {name}"),
      )
      .await?;
    let resp = resp
      .json::<CloudflareResponse<RecordResult>>()
      .await
      .with_context(|| {
        format!("Failed to parse create record response for {name}")
      })?;

    let record = Self::unpack(resp, "create record")?;
    Ok(record.id)
  }

  async fn update_record(
    &self,
    zone_id: &str,
    record_id: &str,
    content: &str,
  ) -> Result<()> {
    let url = format!(
      "{CLOUDFLARE_API_BASE}/zones/{zone_id}/dns_records/{record_id}"
    );
    let body = UpdateRecordBody {
      content: content.to_string(),
    };
    let resp = self
      .send_and_check(
        self
          .client
          .patch(&url)
          .header("Authorization", self.auth_bearer())
          .json(&body),
        &format!("update record request for {record_id}"),
      )
      .await?;
    let resp = resp
      .json::<CloudflareResponse<RecordResult>>()
      .await
      .with_context(|| {
        format!(
          "Failed to parse update record response for {record_id}"
        )
      })?;

    Self::unpack(resp, "update record")?;
    Ok(())
  }

  async fn delete_record(
    &self,
    zone_id: &str,
    record_id: &str,
  ) -> Result<()> {
    let url = format!(
      "{CLOUDFLARE_API_BASE}/zones/{zone_id}/dns_records/{record_id}"
    );
    let resp = self
      .send_and_check(
        self
          .client
          .delete(&url)
          .header("Authorization", self.auth_bearer()),
        &format!("delete record request for {record_id}"),
      )
      .await?;
    let resp = resp
      .json::<CloudflareResponse<RecordResult>>()
      .await
      .with_context(|| {
        format!(
          "Failed to parse delete record response for {record_id}"
        )
      })?;

    Self::unpack(resp, "delete record")?;
    Ok(())
  }
}

// ===== Cloudflare API Envelope / Bodies =====

#[derive(Deserialize)]
struct CloudflareError {
  #[allow(dead_code)]
  code: i32,
  message: String,
}

#[derive(Deserialize)]
struct CloudflareResponse<T> {
  success: bool,
  #[serde(default)]
  errors: Vec<CloudflareError>,
  result: Option<T>,
}

#[derive(Deserialize)]
struct ZoneResult {
  id: String,
}

#[derive(Deserialize)]
struct RecordResult {
  id: String,
}

#[derive(Serialize)]
struct CreateRecordBody {
  #[serde(rename = "type")]
  record_type: &'static str,
  name: String,
  content: String,
  ttl: u32,
  proxied: bool,
}

#[derive(Serialize)]
struct UpdateRecordBody {
  content: String,
}
