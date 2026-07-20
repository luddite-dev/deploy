use std::{path::Path, sync::OnceLock};

use anyhow::{Context, anyhow};
use bollard::Docker;
use command::{
  CommandOptions, run_komodo_standard_command, run_shell_command,
};
use komodo_client::entities::{
  TerminationSignal, docker::*, update::Log,
};

pub mod compose;
pub mod image;
pub mod stats;

mod container;
mod network;
mod volume;

pub struct DockerClient {
  docker: Docker,
}

impl DockerClient {
  pub fn connect() -> anyhow::Result<DockerClient> {
    // Autodetect Podman socket if DOCKER_HOST is not explicitly set.
    // This sets the env var so both bollard's API connection and the
    // `docker compose` CLI commands shell out to the same socket.
    if std::env::var("DOCKER_HOST").is_err() {
      if let Some(socket) = autodetect_container_socket() {
        info!("Auto-detected container runtime socket: {socket}");
        // SAFETY: This runs during single-threaded startup before any
        // threads that might read DOCKER_HOST are spawned.
        unsafe { std::env::set_var("DOCKER_HOST", &socket) };
      } else {
        warn!(
          "No container runtime socket found. \
           Checked: $XDG_RUNTIME_DIR/podman/podman.sock, \
           /run/podman/podman.sock, /var/run/docker.sock. \
           Set DOCKER_HOST manually if your socket is elsewhere."
        );
      }
    }

    let docker = Docker::connect_with_defaults()
      .context("Failed to connect to container runtime API. Container monitoring won't work.")?;
    Ok(DockerClient { docker })
  }
}

/// Probe for a container runtime socket (Podman first, then Docker).
/// Returns a `unix://` URL suitable for `DOCKER_HOST`.
fn autodetect_container_socket() -> Option<String> {
  // 1. Rootless Podman: $XDG_RUNTIME_DIR/podman/podman.sock
  if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
    let path = format!("{xdg}/podman/podman.sock");
    if Path::new(&path).exists() {
      return Some(format!("unix://{path}"));
    }
  }

  // 2. System Podman: /run/podman/podman.sock
  let system_podman = "/run/podman/podman.sock";
  if Path::new(system_podman).exists() {
    return Some(format!("unix://{system_podman}"));
  }

  // 3. Docker fallback: /var/run/docker.sock
  let docker_sock = "/var/run/docker.sock";
  if Path::new(docker_sock).exists() {
    return Some(format!("unix://{docker_sock}"));
  }

  None
}

/// Returns the container CLI binary name to use for shell-out commands.
///
/// If `DOCKER_HOST` explicitly points to `/var/run/docker.sock`, the user
/// has chosen Docker — return `"docker"`. Otherwise, prefer `podman` if the
/// binary exists in PATH, falling back to `"docker"`.
pub fn container_cli() -> &'static str {
  static CLI: OnceLock<&str> = OnceLock::new();
  CLI.get_or_init(|| {
    // Explicit Docker usage
    if let Ok(host) = std::env::var("DOCKER_HOST") {
      if host.contains("/var/run/docker.sock") {
        return "docker";
      }
    }
    // Prefer podman if available
    if std::process::Command::new("podman")
      .arg("--version")
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .status()
      .is_ok()
    {
      return "podman";
    }
    // Fallback
    "docker"
  })
}

/// Returns whether login was actually performed.
#[instrument("DockerLogin", skip(registry_token))]
pub async fn docker_login(
  domain: &str,
  account: &str,
  // For local token override from core.
  registry_token: Option<&str>,
) -> anyhow::Result<bool> {
  if domain.is_empty() || account.is_empty() {
    return Ok(false);
  }

  let registry_token = match registry_token {
    Some(token) => token,
    None => crate::helpers::registry_token(domain, account)?,
  };

  let cli = container_cli();
  let log = run_shell_command(&format!(
    "echo {registry_token} | {cli} login {domain} --username '{account}' --password-stdin",
  ), CommandOptions::default())
  .await;

  if log.success() {
    return Ok(true);
  }

  let mut e = anyhow!("End of trace");
  for line in
    log.stderr.split('\n').filter(|line| !line.is_empty()).rev()
  {
    e = e.context(line.to_string());
  }
  for line in
    log.stdout.split('\n').filter(|line| !line.is_empty()).rev()
  {
    e = e.context(line.to_string());
  }
  Err(e.context(format!("Registry {domain} login error")))
}

#[instrument("PullImage")]
pub async fn pull_image(image: &str) -> Log {
  let cli = container_cli();
  let command = format!("{cli} pull {image}");
  run_komodo_standard_command(
    "Docker Pull",
    command,
    CommandOptions::default(),
  )
  .await
}

pub fn stop_container_command(
  container_name: &str,
  signal: Option<TerminationSignal>,
  time: Option<i32>,
) -> String {
  let signal = signal
    .map(|signal| format!(" --signal {signal}"))
    .unwrap_or_default();
  let time = time
    .map(|time| format!(" --time {time}"))
    .unwrap_or_default();
  let cli = container_cli();
  format!("{cli} stop{signal}{time} {container_name}")
}

fn convert_object_version(
  version: bollard::models::ObjectVersion,
) -> ObjectVersion {
  ObjectVersion {
    index: version.index,
  }
}

fn convert_mount(mount: bollard::models::Mount) -> Mount {
  Mount {
    target: mount.target,
    source: mount.source,
    typ: mount.typ.map(convert_mount_type).unwrap_or_default(),
    read_only: mount.read_only,
    consistency: mount.consistency,
    bind_options: mount.bind_options.map(|options| {
      MountBindOptions {
        propagation: options
          .propagation
          .map(convert_mount_propogation)
          .unwrap_or_default(),
        non_recursive: options.non_recursive,
        create_mountpoint: options.create_mountpoint,
        read_only_non_recursive: options.read_only_non_recursive,
        read_only_force_recursive: options.read_only_force_recursive,
      }
    }),
    volume_options: mount.volume_options.map(|options| {
      MountVolumeOptions {
        no_copy: options.no_copy,
        labels: options.labels.unwrap_or_default(),
        driver_config: options.driver_config.map(|config| {
          MountVolumeOptionsDriverConfig {
            name: config.name,
            options: config.options.unwrap_or_default(),
          }
        }),
        subpath: options.subpath,
      }
    }),
    tmpfs_options: mount.tmpfs_options.map(|options| {
      MountTmpfsOptions {
        size_bytes: options.size_bytes,
        mode: options.mode,
      }
    }),
  }
}

fn convert_mount_type(typ: bollard::config::MountType) -> MountType {
  match typ {
    bollard::config::MountType::BIND => MountType::Bind,
    bollard::config::MountType::VOLUME => MountType::Volume,
    bollard::config::MountType::IMAGE => MountType::Image,
    bollard::config::MountType::TMPFS => MountType::Tmpfs,
    bollard::config::MountType::NPIPE => MountType::Npipe,
    bollard::config::MountType::CLUSTER => MountType::Cluster,
  }
}

fn convert_mount_propogation(
  propogation: bollard::config::MountBindOptionsPropagationEnum,
) -> MountBindOptionsPropagationEnum {
  match propogation {
    bollard::config::MountBindOptionsPropagationEnum::EMPTY => {
      MountBindOptionsPropagationEnum::Empty
    }
    bollard::config::MountBindOptionsPropagationEnum::PRIVATE => {
      MountBindOptionsPropagationEnum::Private
    }
    bollard::config::MountBindOptionsPropagationEnum::RPRIVATE => {
      MountBindOptionsPropagationEnum::Rprivate
    }
    bollard::config::MountBindOptionsPropagationEnum::SHARED => {
      MountBindOptionsPropagationEnum::Shared
    }
    bollard::config::MountBindOptionsPropagationEnum::RSHARED => {
      MountBindOptionsPropagationEnum::Rshared
    }
    bollard::config::MountBindOptionsPropagationEnum::SLAVE => {
      MountBindOptionsPropagationEnum::Slave
    }
    bollard::config::MountBindOptionsPropagationEnum::RSLAVE => {
      MountBindOptionsPropagationEnum::Rslave
    }
  }
}

fn convert_health_config(
  config: bollard::models::HealthConfig,
) -> HealthConfig {
  HealthConfig {
    test: config.test.unwrap_or_default(),
    interval: config.interval,
    timeout: config.timeout,
    retries: config.retries,
    start_period: config.start_period,
    start_interval: config.start_interval,
  }
}

fn convert_resources_ulimits(
  ulimit: bollard::models::ResourcesUlimits,
) -> ResourcesUlimits {
  ResourcesUlimits {
    name: ulimit.name,
    soft: ulimit.soft,
    hard: ulimit.hard,
  }
}
