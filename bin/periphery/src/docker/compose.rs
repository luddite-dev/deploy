use std::time::Duration;

use anyhow::{Context, anyhow};
use command::{CommandOptions, run_komodo_standard_command};
use komodo_client::entities::stack::ComposeProject;
use serde::{Deserialize, Serialize};

use crate::config::periphery_config;
use crate::docker::container_cli;

pub fn docker_compose() -> &'static str {
  static COMPOSE: std::sync::OnceLock<&str> =
    std::sync::OnceLock::new();
  COMPOSE.get_or_init(|| {
    let cli = container_cli();
    if periphery_config().legacy_compose_cli {
      Box::leak(format!("{cli}-compose").into_boxed_str())
    } else {
      Box::leak(format!("{cli} compose").into_boxed_str())
    }
  })
}

pub async fn list_compose_projects()
-> anyhow::Result<Vec<ComposeProject>> {
  let docker_compose = docker_compose();
  let res = run_komodo_standard_command(
    "List Projects",
    format!("{docker_compose} ls --all --format json"),
    CommandOptions::default().timeout(Duration::from_secs(1)),
  )
  .await;

  if !res.success {
    return Err(anyhow!("{}", res.combined()).context(format!(
      "Failed to list compose projects using {docker_compose} ls"
    )));
  }

  let mut res =
    serde_json::from_str::<Vec<DockerComposeLsItem>>(&res.stdout)
      .with_context(|| res.stdout.clone())
      .with_context(|| {
        format!(
          "Failed to parse '{docker_compose} ls' response from json"
        )
      })?
      .into_iter()
      .filter(|item| !item.name.is_empty())
      .map(|item| ComposeProject {
        name: item.name,
        status: item.status,
        compose_files: item
          .config_files
          .split(',')
          .map(str::to_string)
          .collect(),
      })
      .collect::<Vec<_>>();

  res.sort_by(|a, b| {
    a.status.cmp(&b.status).then_with(|| a.name.cmp(&b.name))
  });

  Ok(res)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerComposeLsItem {
  #[serde(default, alias = "Name")]
  pub name: String,
  #[serde(alias = "Status")]
  pub status: Option<String>,
  /// Comma seperated list of paths
  #[serde(default, alias = "ConfigFiles")]
  pub config_files: String,
}
