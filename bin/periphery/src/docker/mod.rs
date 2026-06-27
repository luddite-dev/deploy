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
    let docker = Docker::connect_with_defaults()
      .context("Failed to connect to docker api. Docker monitoring won't work and will return empty results.")?;
    Ok(DockerClient { docker })
  }
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

  let log = run_shell_command(&format!(
    "echo {registry_token} | docker login {domain} --username '{account}' --password-stdin",
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
  let command = format!("docker pull {image}");
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
  format!("docker stop{signal}{time} {container_name}")
}

fn convert_object_version(
  version: bollard::models::ObjectVersion,
) -> ObjectVersion {
  ObjectVersion {
    index: version.index,
  }
}

fn convert_driver(driver: bollard::models::Driver) -> Driver {
  Driver {
    name: driver.name,
    options: driver.options,
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

fn convert_resource_object(
  object: bollard::models::ResourceObject,
) -> ResourceObject {
  ResourceObject {
    nano_cpus: object.nano_cpus,
    memory_bytes: object.memory_bytes,
    generic_resources: object
      .generic_resources
      .map(convert_generic_resources),
  }
}

fn convert_generic_resources(
  resources: Vec<bollard::models::GenericResourcesInner>,
) -> Vec<GenericResourcesInner> {
  resources
    .into_iter()
    .map(|resource| GenericResourcesInner {
      named_resource_spec: resource.named_resource_spec.map(|spec| {
        GenericResourcesInnerNamedResourceSpec {
          kind: spec.kind,
          value: spec.value,
        }
      }),
      discrete_resource_spec: resource.discrete_resource_spec.map(
        |spec| GenericResourcesInnerDiscreteResourceSpec {
          kind: spec.kind,
          value: spec.value,
        },
      ),
    })
    .collect()
}

fn convert_platform(platform: bollard::models::Platform) -> Platform {
  Platform {
    architecture: platform.architecture,
    os: platform.os,
  }
}

fn convert_tls_info(tls_info: bollard::models::TlsInfo) -> TlsInfo {
  TlsInfo {
    trust_root: tls_info.trust_root,
    cert_issuer_subject: tls_info.cert_issuer_subject,
    cert_issuer_public_key: tls_info.cert_issuer_public_key,
  }
}
