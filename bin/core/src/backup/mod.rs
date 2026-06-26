pub mod scheduler;

use std::str::FromStr;
use std::sync::OnceLock;

use anyhow::Context;
use database::mungos::{
  by_id::update_one_by_id,
  find::find_collect,
  mongodb::bson::{self, doc, oid::ObjectId, Bson, Document},
  update::Update,
};
use komodo_client::entities::deployment::{
  BackupConfig, BackupDestination, Deployment, VolumeBackupRecord,
};
use komodo_client::entities::server::Server;
use komodo_client::entities::stack::Stack;
use periphery_client::api::volume_backup::{
  BackupVolume, ListVolumeBackups,
};

use crate::{helpers::periphery_client, periphery::PeripheryClient, state::db_client};

static BACKUP_DESTINATION: OnceLock<Option<BackupDestination>> =
  OnceLock::new();

/// Global backup S3 destination, lazily initialized from
/// KOMODO_BACKUP_S3_* env vars. Returns None if backup is not
/// configured (scheduler and on-demand both no-op).
pub fn backup_destination() -> Option<&'static BackupDestination> {
  BACKUP_DESTINATION
    .get_or_init(|| {
      let endpoint = std::env::var("KOMODO_BACKUP_S3_ENDPOINT").ok()?;
      let region = std::env::var("KOMODO_BACKUP_S3_REGION").ok()?;
      let bucket = std::env::var("KOMODO_BACKUP_S3_BUCKET").ok()?;
      let access_key = std::env::var("KOMODO_BACKUP_S3_ACCESS_KEY_ID")
        .or_else(|_| std::env::var("KOMODO_BACKUP_S3_ACCESS_KEY"))
        .ok()?;
      let secret_key = std::env::var("KOMODO_BACKUP_S3_SECRET_ACCESS_KEY")
        .or_else(|_| std::env::var("KOMODO_BACKUP_S3_SECRET_KEY"))
        .ok()?;
      Some(BackupDestination {
        endpoint,
        region,
        bucket,
        access_key,
        secret_key,
      })
    })
    .as_ref()
}

/// Back up all named volumes for a deployment. Updates info.last_backup.
pub async fn backup_deployment_volumes(
  deployment_id: &str,
) -> anyhow::Result<()> {
  let dest = backup_destination()
    .context("Backup destination not configured (set KOMODO_BACKUP_S3_* env vars)")?
    .clone();

  let deployment: Deployment = find_collect(
    &db_client().deployments,
    doc! { "_id": ObjectId::from_str(deployment_id)
      .context("Invalid deployment id ObjectId")? },
    None,
  )
  .await
  .context("Failed to query deployment")?
  .into_iter()
  .next()
  .context("Deployment not found")?;

  let server: Server = find_collect(
    &db_client().servers,
    doc! { "_id": ObjectId::from_str(&deployment.info.assigned_server)
      .context("Invalid assigned_server ObjectId")? },
    None,
  )
  .await
  .context("Failed to query server for assigned_server")?
  .into_iter()
  .next()
  .context("Server not found for assigned_server")?;
  let periphery = periphery_client(&server).await?;

  let max_backups = deployment
    .config
    .backup
    .as_ref()
    .map(|b: &BackupConfig| b.max_backups)
    .unwrap_or(7);

  for vm in &deployment.config.volumes {
    let result = periphery
      .request(BackupVolume {
        deployment_id: deployment_id.to_string(),
        volume_name: vm.volume.clone(),
        destination: dest.clone(),
      })
      .await
      .context("BackupVolume RPC failed")?;

    let record = VolumeBackupRecord {
      s3_key: result.s3_key,
      timestamp: chrono::Utc::now().timestamp(),
      size_bytes: result.size_bytes,
      checksum: result.checksum,
    };

    let mut set_doc = Document::new();
    set_doc.insert(
      format!("info.last_backup.{}", vm.volume),
      to_bson(&record)?,
    );
    update_one_by_id(
      &db_client().deployments,
      deployment_id,
      Update::Set(set_doc),
      None,
    )
    .await
    .context("Failed to update info.last_backup")?;

    enforce_retention(
      &periphery,
      deployment_id,
      &vm.volume,
      &dest,
      max_backups,
    )
    .await?;
  }

  Ok(())
}

/// Back up all named volumes for a stack (parsed from compose YAML).
pub async fn backup_stack_volumes(stack_id: &str) -> anyhow::Result<()> {
  let dest = backup_destination()
    .context("Backup destination not configured")?
    .clone();

  let stack: Stack = find_collect(
    &db_client().stacks,
    doc! { "_id": ObjectId::from_str(stack_id)
      .context("Invalid stack id ObjectId")? },
    None,
  )
  .await
  .context("Failed to query stack")?
  .into_iter()
  .next()
  .context("Stack not found")?;

  let server: Server = find_collect(
    &db_client().servers,
    doc! { "_id": ObjectId::from_str(&stack.info.assigned_server)
      .context("Invalid assigned_server ObjectId")? },
    None,
  )
  .await
  .context("Failed to query server for assigned_server")?
  .into_iter()
  .next()
  .context("Server not found for assigned_server")?;
  let periphery = periphery_client(&server).await?;

  let volumes = parse_stack_volumes(&stack.config.file_contents)?;
  let max_backups = stack
    .config
    .backup
    .as_ref()
    .map(|b: &BackupConfig| b.max_backups)
    .unwrap_or(7);

  for vol_name in volumes {
    let result = periphery
      .request(BackupVolume {
        deployment_id: stack_id.to_string(),
        volume_name: vol_name.clone(),
        destination: dest.clone(),
      })
      .await
      .context("BackupVolume RPC failed")?;

    let record = VolumeBackupRecord {
      s3_key: result.s3_key,
      timestamp: chrono::Utc::now().timestamp(),
      size_bytes: result.size_bytes,
      checksum: result.checksum,
    };

    let mut set_doc = Document::new();
    set_doc.insert(
      format!("info.last_backup.{vol_name}"),
      to_bson(&record)?,
    );
    update_one_by_id(
      &db_client().stacks,
      stack_id,
      Update::Set(set_doc),
      None,
    )
    .await
    .context("Failed to update info.last_backup")?;

    enforce_retention(&periphery, stack_id, &vol_name, &dest, max_backups)
      .await?;
  }

  Ok(())
}

/// Delete oldest backups beyond max_backups, via S3 delete_object.
async fn enforce_retention(
  periphery: &PeripheryClient,
  deployment_id: &str,
  volume_name: &str,
  dest: &BackupDestination,
  max_backups: u32,
) -> anyhow::Result<()> {
  let backups: Vec<_> = periphery
    .request(ListVolumeBackups {
      deployment_id: deployment_id.to_string(),
      volume_name: volume_name.to_string(),
      destination: dest.clone(),
    })
    .await
    .context("ListVolumeBackups RPC failed")?;

  if backups.len() as u32 <= max_backups {
    return Ok(());
  }

  let to_delete = &backups[..backups.len().saturating_sub(max_backups as usize)];
  for backup in to_delete {
    delete_s3_object(dest, &backup.s3_key).await?;
  }
  Ok(())
}

async fn delete_s3_object(
  dest: &BackupDestination,
  key: &str,
) -> anyhow::Result<()> {
  use s3::{Bucket, Region, creds::Credentials};
  let creds = Credentials::new(
    Some(&dest.access_key),
    Some(&dest.secret_key),
    None,
    None,
    None,
  )?;
  let region = Region::Custom {
    region: dest.region.clone(),
    endpoint: dest.endpoint.clone(),
  };
  let bucket = Bucket::new(&dest.bucket, region, creds)?.with_path_style();
  let _ = bucket.delete_object(key).await?;
  Ok(())
}

/// Parse the top-level `volumes:` keys from a docker compose YAML.
/// Returns an empty vec if there is no top-level volumes mapping.
fn parse_stack_volumes(yaml: &str) -> anyhow::Result<Vec<String>> {
  let parsed: serde_yaml_ng::Value =
    serde_yaml_ng::from_str(yaml).context("Failed to parse compose YAML")?;
  let volumes = parsed
    .get("volumes")
    .and_then(|v| v.as_mapping())
    .map(|m| {
      m
        .keys()
        .filter_map(|k| k.as_str().map(String::from))
        .collect()
    })
    .unwrap_or_default();
  Ok(volumes)
}

fn to_bson<T: serde::Serialize>(v: &T) -> anyhow::Result<Bson> {
  Ok(bson::to_bson(v)?)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_stack_volumes_named() {
    let yaml = "services:\n  web:\n    image: nginx\n    volumes:\n      - data:/var/lib/data\nvolumes:\n  data:\n";
    let vols = parse_stack_volumes(yaml).unwrap();
    assert_eq!(vols, vec!["data".to_string()]);
  }

  #[test]
  fn test_parse_stack_volumes_no_volumes_key() {
    let yaml = "services:\n  web:\n    image: nginx\n";
    let vols = parse_stack_volumes(yaml).unwrap();
    assert!(vols.is_empty());
  }

  #[test]
  fn test_parse_stack_volumes_multiple() {
    let yaml = "services:\n  web:\n    image: nginx\nvolumes:\n  data:\n  certs:\n  cache:\n";
    let vols = parse_stack_volumes(yaml).unwrap();
    assert_eq!(vols, vec!["data", "certs", "cache"]);
  }

  #[test]
  fn test_parse_stack_volumes_empty_mapping() {
    let yaml = "services:\n  web:\n    image: nginx\nvolumes: {}\n";
    let vols = parse_stack_volumes(yaml).unwrap();
    assert!(vols.is_empty());
  }

  #[test]
  fn test_backup_destination_unconfigured_when_env_absent() {
    // Clear any cache from prior tests in the same process: OnceLock can
    // only be set once, so if a prior test set it, we cannot re-test. We
    // only assert the contract holds when env is unset AND the cell is
    // still empty; otherwise we just check it returns Some consistently.
    let present = backup_destination();
    // Contract: environment-not-set => None. If env is set in the test
    // runner, verify it parses all five fields.
    let endpoint_set = std::env::var("KOMODO_BACKUP_S3_ENDPOINT").is_ok();
    if !endpoint_set {
      assert!(present.is_none(), "expected None when KOMODO_BACKUP_S3_* unset");
    } else {
      assert!(present.is_some(), "expected Some when env set");
    }
  }
}
