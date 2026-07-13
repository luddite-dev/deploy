use anyhow::Result;
use async_trait::async_trait;

use komodo_client::entities::dns::DnsRecordType;

/// Trait-abstracted DNS provider. Cloudflare is the first
/// implementation; future providers (Technitium, RFC 2136)
/// implement this same trait.
#[async_trait]
pub trait DnsProvider: Send + Sync {
  /// Resolve the provider's zone id for the given domain.
  /// Implementations should cache the result.
  async fn resolve_zone_id(&self, domain: &str) -> Result<String>;

  /// Create a DNS record. Returns the provider-side record id.
  async fn create_record(
    &self,
    zone_id: &str,
    record_type: DnsRecordType,
    name: &str,
    content: &str,
    ttl: u32,
  ) -> Result<String>;

  /// Update the content (target) of an existing DNS record.
  async fn update_record(
    &self,
    zone_id: &str,
    record_id: &str,
    content: &str,
  ) -> Result<()>;

  /// Delete an existing DNS record.
  async fn delete_record(
    &self,
    zone_id: &str,
    record_id: &str,
  ) -> Result<()>;
}
