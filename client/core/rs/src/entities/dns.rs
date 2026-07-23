use serde::{Deserialize, Serialize};
use typeshare::typeshare;

use crate::entities::I64;

/// The DNS record types supported by the ingress layer.
/// Currently only A / AAAA are needed for node endpoint routing.
#[typeshare]
#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "UPPERCASE")]
pub enum DnsRecordType {
  A,
  AAAA,
}

impl DnsRecordType {
  pub fn as_str(&self) -> &'static str {
    match self {
      DnsRecordType::A => "A",
      DnsRecordType::AAAA => "AAAA",
    }
  }
}

/// A DNS record managed by the Komodo ingress layer.
///
/// Each record maps a hostname to a node via the configured
/// DNS provider (Cloudflare, Technitium, etc.).
#[typeshare]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DnsRecord {
  /// Internal Komodo id for this DNS record.
  pub id: String,
  /// A or AAAA
  pub record_type: DnsRecordType,
  /// Fully-qualified hostname, eg `node1.komo.do`.
  pub hostname: String,
  /// The Komodo node this record points at.
  pub target_node_id: String,
  /// The provider type, eg `cloudflare`.
  pub provider_type: String,
  /// The zone id at the provider.
  pub provider_zone_id: String,
  /// The record id at the provider.
  pub provider_record_id: String,
  /// The deployment this record is attached to, if any.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub deployment_id: Option<String>,
  /// The stack this record is attached to, if any.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub stack_id: Option<String>,
  /// TTL in seconds.
  pub ttl: u32,
  /// Unix timestamp (ms) the record was created.
  pub created_at: I64,
  /// Unix timestamp (ms) the record was last updated.
  pub updated_at: I64,
}

/// Provider-specific DNS configuration. Currently only Cloudflare
/// is supported, but the `provider` field is kept open to allow
/// future providers (Technitium, RFC 2136, etc.).
#[typeshare]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DnsProviderConfig {
  /// The provider type. Set to `cloudflare` to use Cloudflare.
  /// Empty (default) disables the ingress DNS layer.
  #[serde(default)]
  pub provider: String,
  /// Cloudflare API token. May be a literal token, or a
  /// `file:/path/to/token` spec to load from file.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cloudflare_api_token: Option<String>,
  /// The base domain managed by this provider, eg `komo.do`.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub base_domain: Option<String>,
}

/// Top-level ingress configuration block on [CoreConfig].
#[typeshare]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct IngressConfig {
  /// DNS management sub-config.
  #[serde(default)]
  pub dns: DnsProviderConfig,
}
