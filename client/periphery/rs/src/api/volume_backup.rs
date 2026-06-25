use komodo_client::entities::deployment::{
  BackupDestination, BackupResult, RestoreResult, VolumeBackupInfo,
};
use mogh_resolver::Resolve;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(BackupResult)]
#[error(anyhow::Error)]
pub struct BackupVolume {
  pub deployment_id: String,
  pub volume_name: String,
  pub destination: BackupDestination,
}

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(RestoreResult)]
#[error(anyhow::Error)]
pub struct RestoreVolume {
  pub deployment_id: String,
  pub volume_name: String,
  pub source_key: String,
  pub destination: BackupDestination,
}

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(Vec<VolumeBackupInfo>)]
#[error(anyhow::Error)]
pub struct ListVolumeBackups {
  pub deployment_id: String,
  pub volume_name: String,
  pub destination: BackupDestination,
}
