use command::{CommandOptions, run_standard_command};
use komodo_client::entities::deployment::{
  BackupDestination, BackupResult, RestoreResult, VolumeBackupInfo,
};
use md5::{Digest, Md5};
use mogh_resolver::Resolve;
use s3::{Bucket, Region, creds::Credentials};

use periphery_client::api::volume_backup::{
  BackupVolume, ListVolumeBackups, RestoreVolume,
};

use crate::api::Args;

fn build_bucket(
  dest: &BackupDestination,
) -> anyhow::Result<Box<Bucket>> {
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
  let bucket =
    Bucket::new(&dest.bucket, region, creds)?.with_path_style();
  Ok(bucket)
}

fn s3_key_prefix(deployment_id: &str, volume_name: &str) -> String {
  format!("backups/deployments/{deployment_id}/volumes/{volume_name}")
}

impl Resolve<Args> for BackupVolume {
  #[instrument("backup_volume")]
  async fn resolve(
    self,
    _args: &Args,
  ) -> anyhow::Result<BackupResult> {
    let timestamp = chrono::Utc::now().timestamp();
    let local_file =
      format!("/tmp/{}-{timestamp}.tar", self.volume_name);
    let s3_key = format!(
      "{}/{timestamp}.tar",
      s3_key_prefix(&self.deployment_id, &self.volume_name)
    );

    let output = run_standard_command(
      &format!(
        "podman volume export {} --output {}",
        self.volume_name, local_file
      ),
      CommandOptions::default(),
    )
    .await;
    if !output.success() {
      anyhow::bail!("podman volume export failed: {}", output.stderr);
    }

    let bucket = build_bucket(&self.destination)?;
    let file_data = std::fs::read(&local_file)?;
    let size_bytes = file_data.len() as u64;
    let checksum = format!("{:x}", Md5::digest(&file_data));
    bucket.put_object(&s3_key, &file_data).await?;

    std::fs::remove_file(&local_file)?;

    Ok(BackupResult {
      s3_key,
      size_bytes,
      checksum,
    })
  }
}

impl Resolve<Args> for RestoreVolume {
  #[instrument("restore_volume")]
  async fn resolve(
    self,
    _args: &Args,
  ) -> anyhow::Result<RestoreResult> {
    let local_file = format!("/tmp/{}-restore.tar", self.volume_name);

    let bucket = build_bucket(&self.destination)?;
    let response = bucket.get_object(&self.source_key).await?;
    let data = response.to_vec();
    let bytes_restored = data.len() as u64;
    std::fs::write(&local_file, &data)?;

    let _ = run_standard_command(
      &format!("podman volume create {}", self.volume_name),
      CommandOptions::default(),
    )
    .await;

    let output = run_standard_command(
      &format!(
        "podman volume import {} {}",
        self.volume_name, local_file
      ),
      CommandOptions::default(),
    )
    .await;
    if !output.success() {
      anyhow::bail!("podman volume import failed: {}", output.stderr);
    }

    std::fs::remove_file(&local_file)?;

    Ok(RestoreResult { bytes_restored })
  }
}

impl Resolve<Args> for ListVolumeBackups {
  #[instrument("list_volume_backups")]
  async fn resolve(
    self,
    _args: &Args,
  ) -> anyhow::Result<Vec<VolumeBackupInfo>> {
    let bucket = build_bucket(&self.destination)?;
    let prefix = format!(
      "{}/",
      s3_key_prefix(&self.deployment_id, &self.volume_name)
    );
    let results = bucket.list(prefix, None).await?;

    let mut backups: Vec<VolumeBackupInfo> = results
      .into_iter()
      .flat_map(|page| page.contents)
      .filter_map(|obj| {
        let key = obj.key;
        let timestamp = key
          .rsplit('/')
          .next()?
          .strip_suffix(".tar")?
          .parse::<i64>()
          .ok()?;
        Some(VolumeBackupInfo {
          s3_key: key,
          timestamp,
          size_bytes: obj.size,
        })
      })
      .collect();

    backups.sort_by_key(|b| b.timestamp);
    Ok(backups)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_s3_key_prefix_format() {
    let p = s3_key_prefix("dep-1", "data-vol");
    assert_eq!(p, "backups/deployments/dep-1/volumes/data-vol");
  }

  #[test]
  fn test_backup_result_default() {
    let r = BackupResult::default();
    assert_eq!(r.s3_key, "");
    assert_eq!(r.size_bytes, 0);
  }
}
