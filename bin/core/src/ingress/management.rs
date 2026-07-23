//! DNS record lifecycle management for the ingress layer.
//!
//! These functions own the full create / update / delete cycle for
//! the `dns_records` Mongo collection and the corresponding records
//! at the configured DNS provider (Cloudflare, etc.). The provider
//! is built from [CoreConfig] ingress settings via
//! [crate::dns::build_dns_provider].

use anyhow::{Context, Result};
use database::mungos::{
  find::find_collect,
  mongodb::{
    Collection,
    bson::{doc, oid::ObjectId},
  },
};
use komodo_client::entities::{
  dns::{DnsRecord, DnsRecordType, IngressConfig},
  komodo_timestamp,
};
use tracing::{info, warn};

use crate::{dns::build_dns_provider, state::db_client};

/// Return the typed `dns_records` collection.
///
/// This collection is not (yet) a named field on
/// [database::Client], so we access it dynamically via the raw
/// `db` handle. The entity type [DnsRecord] handles (de)serialization.
fn dns_records() -> Collection<DnsRecord> {
  db_client().db.collection::<DnsRecord>("dns_records")
}

/// Resolve the ingress DNS provider from the running [CoreConfig].
///
/// Returns an error if the ingress DNS layer is not configured
/// (empty `provider` field) — callers cannot manage DNS records
/// without a live provider.
fn ingress_provider()
-> Result<std::sync::Arc<dyn crate::dns::provider::DnsProvider>> {
  let provider = build_dns_provider()
    .context("build DNS provider for ingress management")?;
  provider.ok_or_else(|| {
    anyhow::anyhow!(
      "ingress DNS provider not configured (set `ingress.dns.provider`)"
    )
  })
}

/// Create DNS A/AAAA records for a deployment hostname and persist
/// each to the database.
///
/// `hostname` is the short subdomain (eg `app1`); the FQDN is
/// built by appending `ingress_config.dns.base_domain`. Only the
/// record types for which an IP is provided are created.
#[allow(clippy::too_many_arguments)]
pub async fn create_deployment_dns_record(
  deployment_id: &str,
  hostname: &str,
  target_node_id: &str,
  target_ipv4: Option<&str>,
  target_ipv6: Option<&str>,
  ingress_config: &IngressConfig,
  ttl: u32,
) -> Result<()> {
  let provider = ingress_provider()?;
  let base_domain = ingress_config
    .dns
    .base_domain
    .as_deref()
    .filter(|d| !d.is_empty())
    .ok_or_else(|| {
      anyhow::anyhow!(
        "ingress.dns.base_domain not configured — cannot build FQDN"
      )
    })?;
  let fqdn = format!("{hostname}.{base_domain}");
  let zone_id = provider
    .resolve_zone_id(base_domain)
    .await
    .with_context(|| format!("resolve zone id for {base_domain}"))?;
  let now = komodo_timestamp();

  // Create A record if IPv4 provided.
  if let Some(ipv4) = target_ipv4 {
    let record_id = provider
      .create_record(&zone_id, DnsRecordType::A, &fqdn, ipv4, ttl)
      .await
      .with_context(|| format!("create A record for {fqdn}"))?;
    let record = DnsRecord {
      id: ObjectId::new().to_string(),
      deployment_id: Some(deployment_id.to_string()),
      stack_id: None,
      hostname: fqdn.clone(),
      record_type: DnsRecordType::A,
      target_node_id: target_node_id.to_string(),
      provider_type: ingress_config.dns.provider.clone(),
      provider_zone_id: zone_id.clone(),
      provider_record_id: record_id,
      ttl,
      created_at: now,
      updated_at: now,
    };
    dns_records()
      .insert_one(record)
      .await
      .with_context(|| format!("persist A record for {fqdn}"))?;
    info!("Created DNS A record for {fqdn} -> {ipv4}");
  }

  // Create AAAA record if IPv6 provided.
  if let Some(ipv6) = target_ipv6 {
    let record_id = provider
      .create_record(&zone_id, DnsRecordType::AAAA, &fqdn, ipv6, ttl)
      .await
      .with_context(|| format!("create AAAA record for {fqdn}"))?;
    let record = DnsRecord {
      id: ObjectId::new().to_string(),
      deployment_id: Some(deployment_id.to_string()),
      stack_id: None,
      hostname: fqdn.clone(),
      record_type: DnsRecordType::AAAA,
      target_node_id: target_node_id.to_string(),
      provider_type: ingress_config.dns.provider.clone(),
      provider_zone_id: zone_id.clone(),
      provider_record_id: record_id,
      ttl,
      created_at: now,
      updated_at: now,
    };
    dns_records()
      .insert_one(record)
      .await
      .with_context(|| format!("persist AAAA record for {fqdn}"))?;
    info!("Created DNS AAAA record for {fqdn} -> {ipv6}");
  }

  Ok(())
}

/// Create DNS A/AAAA records for a stack hostname and persist each
/// to the database.
///
/// Mirrors [create_deployment_dns_record] but persists `stack_id`
/// instead of `deployment_id`. `hostname` is the short subdomain
/// (eg `app1`); the FQDN is built by appending
/// `ingress_config.dns.base_domain`. Only the record types for which
/// an IP is provided are created.
#[allow(clippy::too_many_arguments)]
pub async fn create_stack_dns_record(
  stack_id: &str,
  hostname: &str,
  target_node_id: &str,
  target_ipv4: Option<&str>,
  target_ipv6: Option<&str>,
  ingress_config: &IngressConfig,
  ttl: u32,
) -> Result<()> {
  let provider = ingress_provider()?;
  let base_domain = ingress_config
    .dns
    .base_domain
    .as_deref()
    .filter(|d| !d.is_empty())
    .ok_or_else(|| {
      anyhow::anyhow!(
        "ingress.dns.base_domain not configured — cannot build FQDN"
      )
    })?;
  let fqdn = format!("{hostname}.{base_domain}");
  let zone_id = provider
    .resolve_zone_id(base_domain)
    .await
    .with_context(|| format!("resolve zone id for {base_domain}"))?;
  let now = komodo_timestamp();

  // Create A record if IPv4 provided.
  if let Some(ipv4) = target_ipv4 {
    let record_id = provider
      .create_record(&zone_id, DnsRecordType::A, &fqdn, ipv4, ttl)
      .await
      .with_context(|| format!("create A record for {fqdn}"))?;
    let record = DnsRecord {
      id: ObjectId::new().to_string(),
      deployment_id: None,
      stack_id: Some(stack_id.to_string()),
      hostname: fqdn.clone(),
      record_type: DnsRecordType::A,
      target_node_id: target_node_id.to_string(),
      provider_type: ingress_config.dns.provider.clone(),
      provider_zone_id: zone_id.clone(),
      provider_record_id: record_id,
      ttl,
      created_at: now,
      updated_at: now,
    };
    dns_records()
      .insert_one(record)
      .await
      .with_context(|| format!("persist A record for {fqdn}"))?;
    info!("Created DNS A record for {fqdn} -> {ipv4}");
  }

  // Create AAAA record if IPv6 provided.
  if let Some(ipv6) = target_ipv6 {
    let record_id = provider
      .create_record(&zone_id, DnsRecordType::AAAA, &fqdn, ipv6, ttl)
      .await
      .with_context(|| format!("create AAAA record for {fqdn}"))?;
    let record = DnsRecord {
      id: ObjectId::new().to_string(),
      deployment_id: None,
      stack_id: Some(stack_id.to_string()),
      hostname: fqdn.clone(),
      record_type: DnsRecordType::AAAA,
      target_node_id: target_node_id.to_string(),
      provider_type: ingress_config.dns.provider.clone(),
      provider_zone_id: zone_id.clone(),
      provider_record_id: record_id,
      ttl,
      created_at: now,
      updated_at: now,
    };
    dns_records()
      .insert_one(record)
      .await
      .with_context(|| format!("persist AAAA record for {fqdn}"))?;
    info!("Created DNS AAAA record for {fqdn} -> {ipv6}");
  }

  Ok(())
}

/// Delete all DNS records for a deployment (cleanup on deploy delete).
///
/// Best-effort deletion at the provider: a failure to delete one
/// record logs a warning and continues, so that the DB cleanup is
/// always attempted.
pub async fn delete_deployment_dns_records(
  deployment_id: &str,
  _ingress_config: &IngressConfig,
) -> Result<()> {
  let provider = ingress_provider()?;
  let records: Vec<DnsRecord> = find_collect(
    &dns_records(),
    doc! { "deployment_id": deployment_id },
    None,
  )
  .await
  .context("failed to query dns_records for deployment")?;

  for record in records {
    if let Err(e) = provider
      .delete_record(
        &record.provider_zone_id,
        &record.provider_record_id,
      )
      .await
    {
      warn!(
        "Failed to delete DNS record {} ({}) at provider: {e:#}",
        record.hostname, record.provider_record_id
      );
    } else {
      info!(
        "Deleted DNS record {} for deployment {deployment_id}",
        record.hostname
      );
    }
  }

  dns_records()
    .delete_many(doc! { "deployment_id": deployment_id })
    .await
    .context("failed to delete dns_records from database")?;
  Ok(())
}

/// Delete all DNS records for a stack (cleanup on stack delete).
///
/// Mirrors [delete_deployment_dns_records] but queries and deletes
/// by `stack_id`. Best-effort deletion at the provider: a failure to
/// delete one record logs a warning and continues, so that the DB
/// cleanup is always attempted.
pub async fn delete_stack_dns_records(
  stack_id: &str,
  _ingress_config: &IngressConfig,
) -> Result<()> {
  let provider = ingress_provider()?;
  let records: Vec<DnsRecord> =
    find_collect(&dns_records(), doc! { "stack_id": stack_id }, None)
      .await
      .context("failed to query dns_records for stack")?;

  for record in records {
    if let Err(e) = provider
      .delete_record(
        &record.provider_zone_id,
        &record.provider_record_id,
      )
      .await
    {
      warn!(
        "Failed to delete DNS record {} ({}) at provider: {e:#}",
        record.hostname, record.provider_record_id
      );
    } else {
      info!(
        "Deleted DNS record {} for stack {stack_id}",
        record.hostname
      );
    }
  }

  dns_records()
    .delete_many(doc! { "stack_id": stack_id })
    .await
    .context("failed to delete dns_records from database")?;
  Ok(())
}

/// Update DNS records currently pointing at `old_node_id` to point
/// at `new_node_id` instead. Used during ingress node failover.
///
/// Only records whose type has a replacement IP are updated (eg if
/// `new_ipv4` is `None`, existing A records are left untouched and
/// a warning is logged).
pub async fn update_dns_records_for_node(
  old_node_id: &str,
  new_node_id: &str,
  new_ipv4: Option<&str>,
  new_ipv6: Option<&str>,
  _ingress_config: &IngressConfig,
) -> Result<()> {
  let provider = ingress_provider()?;
  let records: Vec<DnsRecord> = find_collect(
    &dns_records(),
    doc! { "target_node_id": old_node_id },
    None,
  )
  .await
  .context("failed to query dns_records for target node")?;

  let now = komodo_timestamp();
  let mut updated = 0usize;
  for record in records {
    let new_ip = match record.record_type {
      DnsRecordType::A => new_ipv4,
      DnsRecordType::AAAA => new_ipv6,
    };
    let Some(new_ip) = new_ip else {
      warn!(
        "No replacement IP for {} record {} during failover — leaving unchanged",
        record.record_type.as_str(),
        record.hostname
      );
      continue;
    };

    if let Err(e) = provider
      .update_record(
        &record.provider_zone_id,
        &record.provider_record_id,
        new_ip,
      )
      .await
    {
      warn!(
        "Failed to update DNS record {} at provider: {e:#}",
        record.hostname
      );
      continue;
    }

    dns_records()
      .update_one(
        doc! { "id": &record.id },
        doc! { "$set": { "target_node_id": new_node_id, "updated_at": now } },
      )
      .await
      .with_context(|| {
        format!(
          "failed to update dns_record {} target_node_id in db",
          record.id
        )
      })?;
    info!("Updated DNS record {} -> {new_ip}", record.hostname);
    updated += 1;
  }

  info!(
    "Failover DNS update: {old_node_id} -> {new_node_id}: \
     {updated} records updated"
  );
  Ok(())
}
