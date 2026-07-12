//! # Configuring the Komodo Periphery Agent
//!
//! The periphery configuration is passed in three ways:
//! 1. Command line args ([CliArgs])
//! 2. Environment Variables ([Env])
//! 3. Configuration File ([PeripheryConfig])
//!
//! The final configuration is built by combining parameters
//! passed through the different methods. The priority of the args is
//! strictly hierarchical, meaning params passed with [CliArgs] have top priority,
//! followed by those passed in the environment, followed by those passed in
//! the configuration file.
//!

use clap::Parser;
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

use crate::{
  deserializers::{ForgivingVec, string_list_deserializer},
  entities::{
    Timelength,
    logger::{LogConfig, LogLevel, StdioLogMode},
  },
};

use super::{
  DockerRegistry, GitProvider, ProviderAccount, empty_or_redacted,
};

/// # Periphery Command Line Arguments.
///
/// This structure represents the periphery command line arguments used to
/// configure the periphery agent. A help manual for the periphery binary
/// can be printed using `/path/to/periphery --help`.
///
/// Example command:
/// ```sh
/// periphery \
///   --config-path /path/to/periphery.config.base.toml \
///   --config-path /other_path/to/overide-periphery-config-directory \
///   --config-keyword periphery \
///   --config-keyword config \
///   --merge-nested-config true \
///   --extend-config-arrays false \
///   --log-level info
/// ```
#[derive(Parser)]
#[command(name = "periphery", author, about, version)]
pub struct CliArgs {
  /// Additional utilities.
  #[command(subcommand)]
  pub command: Option<Command>,

  /// Sets the path of a config file or directory to use.
  /// Can use multiple times
  #[arg(long, short = 'c')]
  pub config_path: Option<Vec<PathBuf>>,

  /// Sets the keywords to match directory periphery config file names on.
  /// Supports wildcard syntax.
  /// Can use multiple times to match multiple patterns independently.
  #[arg(long, short = 'm')]
  pub config_keyword: Option<Vec<String>>,

  /// Merges nested configs, eg. secrets, providers.
  /// Will override the equivalent env configuration.
  /// Default: true
  #[arg(long)]
  pub merge_nested_config: Option<bool>,

  /// Extends config arrays, eg. core_endpoint_addrs.
  /// Will override the equivalent env configuration.
  /// Default: true
  #[arg(long)]
  pub extend_config_arrays: Option<bool>,

  /// Configure the logging level: error, warn, info, debug, trace.
  /// Default: info
  /// If passed, will override any other log_level set.
  #[arg(long)]
  pub log_level: Option<tracing::Level>,
}

#[cfg(feature = "cli")]
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {}

/// # Periphery Environment Variables
///
/// The variables should be passed in the traditional `UPPER_SNAKE_CASE` format,
/// although the lower case format can still be parsed. If equivalent paramater is passed
/// in [CliArgs], the value passed to the environment will be ignored in favor of the cli arg.
#[derive(Deserialize)]
pub struct Env {
  /// Specify the config paths (files or folders) used to build up the
  /// final [PeripheryConfig].
  /// If not provided, will use Default config.
  ///
  /// Note. This is overridden if the equivalent arg is passed in [CliArgs].
  #[serde(default, alias = "periphery_config_path")]
  pub periphery_config_paths: Vec<PathBuf>,
  /// If specifying folders, use this to narrow down which
  /// files will be matched to parse into the final [PeripheryConfig].
  /// Only files inside the folders which have names containing a keywords
  /// provided to `config_keywords` will be included.
  /// Keywords support wildcard matching syntax.
  ///
  /// Note. This is overridden if the equivalent arg is passed in [CliArgs].
  #[serde(
    default = "super::default_config_keywords",
    alias = "periphery_config_keyword"
  )]
  pub periphery_config_keywords: Vec<String>,

  /// Will merge nested config object (eg. secrets, providers) across multiple
  /// config files. Default: `true`
  ///
  /// Note. This is overridden if the equivalent arg is passed in [CliArgs].
  #[serde(default = "super::default_merge_nested_config")]
  pub periphery_merge_nested_config: bool,

  /// Will extend config arrays (eg. `core_endpoint_addrs`) across multiple config files.
  /// Default: `true`
  ///
  /// Note. This is overridden if the equivalent arg is passed in [CliArgs].
  #[serde(default = "super::default_extend_config_arrays")]
  pub periphery_extend_config_arrays: bool,

  /// Override `iroh_secret_key`
  pub periphery_iroh_secret_key: Option<String>,
  /// Override `iroh_secret_key` from file
  pub periphery_iroh_secret_key_file: Option<PathBuf>,
  /// Override `onboarding_key`
  pub periphery_onboarding_key: Option<String>,
  /// Override `onboarding_key` from file
  pub periphery_onboarding_key_file: Option<PathBuf>,
  /// Override `core_endpoint_addrs`
  #[serde(alias = "periphery_core_endpoint_addr")]
  pub periphery_core_endpoint_addrs: Option<Vec<String>>,
  /// Override `connect_as`
  pub periphery_connect_as: Option<String>,
  /// Override `root_directory`
  pub periphery_root_directory: Option<PathBuf>,
  /// Override `repo_dir`
  pub periphery_repo_dir: Option<PathBuf>,
  /// Override `stack_dir`
  pub periphery_stack_dir: Option<PathBuf>,
  /// Override `build_dir`
  pub periphery_build_dir: Option<PathBuf>,
  /// Override `default_terminal_command`
  pub periphery_default_terminal_command: Option<String>,
  /// Override `disable_terminals`
  pub periphery_disable_terminals: Option<bool>,
  /// Override `disable_container_terminals`
  #[serde(alias = "periphery_disable_container_exec")]
  pub periphery_disable_container_terminals: Option<bool>,
  /// Override `stats_polling_rate`
  pub periphery_stats_polling_rate: Option<Timelength>,
  /// Override `container_stats_polling_rate`
  pub periphery_container_stats_polling_rate: Option<Timelength>,
  /// Override `legacy_compose_cli`
  pub periphery_legacy_compose_cli: Option<bool>,

  // LOGGING
  /// Override `logging.level`
  pub periphery_logging_level: Option<LogLevel>,
  /// Override `logging.stdio`
  pub periphery_logging_stdio: Option<StdioLogMode>,
  /// Override `logging.pretty`
  pub periphery_logging_pretty: Option<bool>,
  /// Override `logging.location`
  pub periphery_logging_location: Option<bool>,
  /// Override `logging.ansi`
  pub periphery_logging_ansi: Option<bool>,
  /// Override `logging.timestamps`
  pub periphery_logging_timestamps: Option<bool>,
  /// Override `logging.otlp_endpoint`
  pub periphery_logging_otlp_endpoint: Option<String>,
  /// Override `logging.opentelemetry_service_name`
  pub periphery_logging_opentelemetry_service_name: Option<String>,
  /// Override `logging.opentelemetry_scope_name`
  pub periphery_logging_opentelemetry_scope_name: Option<String>,
  /// Override `pretty_startup_config`
  pub periphery_pretty_startup_config: Option<bool>,

  /// Override `include_disk_mounts`
  pub periphery_include_disk_mounts: Option<ForgivingVec<PathBuf>>,
  /// Override `exclude_disk_mounts`
  pub periphery_exclude_disk_mounts: Option<ForgivingVec<PathBuf>>,
}

/// # Periphery Configuration File
///
/// Refer to the [example file](https://github.com/moghtech/komodo/blob/main/config/periphery.config.toml) for a full example.
#[derive(Debug, Clone, Deserialize)]
pub struct PeripheryConfig {
  /// The Iroh secret key file path (32 raw bytes).
  ///
  /// If the file does not exist, will generate a new key and persist it
  /// so that the endpoint's identity is stable across restarts.
  ///
  /// Default: ${root_directory}/keys/iroh.key
  #[serde(skip_serializing_if = "Option::is_none")]
  pub iroh_secret_key: Option<String>,

  /// Provide an onboarding key to use with the new Server
  /// creation flow.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub onboarding_key: Option<String>,

  // =======================
  // = OUTBOUND CONNECTION =
  // =======================
  /// Iroh endpoint addresses of Komodo Core to connect outbound to.
  #[serde(
    default,
    alias = "core_endpoint_addr",
    deserialize_with = "string_list_deserializer"
  )]
  pub core_endpoint_addrs: Vec<String>,

  /// Server name / id to connect as
  #[serde(default)]
  pub connect_as: String,

  // ==================
  // = OTHER SETTINGS =
  // ==================
  /// The directory Komodo will use as the default root for the specific (repo, stack, build) directories.
  ///
  /// repo: ${root_directory}/repos
  /// stack: ${root_directory}/stacks
  /// build: ${root_directory}/builds
  ///
  /// Note. These can each be overridden with a specific directory
  /// by specifying `repo_dir`, `stack_dir`, or `build_dir` explicitly
  ///
  /// Default: `/etc/komodo`
  #[serde(default = "default_root_directory")]
  pub root_directory: PathBuf,

  /// The system directory where Komodo managed repos will be cloned.
  /// If not provided, will default to `${root_directory}/repos`.
  /// Default: empty
  #[serde(skip_serializing_if = "Option::is_none")]
  pub repo_dir: Option<PathBuf>,

  /// The system directory where stacks will managed.
  /// If not provided, will default to `${root_directory}/stacks`.
  /// Default: empty
  #[serde(skip_serializing_if = "Option::is_none")]
  pub stack_dir: Option<PathBuf>,

  /// The system directory where builds will managed.
  /// If not provided, will default to `${root_directory}/builds`.
  /// Default: empty
  #[serde(skip_serializing_if = "Option::is_none")]
  pub build_dir: Option<PathBuf>,

  /// Configure the default terminal command
  /// when one isn't provided.
  /// Default: `bash`
  #[serde(default = "default_default_terminal_command")]
  pub default_terminal_command: String,

  /// Whether to disable the create terminal
  /// and disallow direct remote shell access.
  /// Default: false
  #[serde(default)]
  pub disable_terminals: bool,

  /// Whether to disable the container exec / attach api
  /// and disallow remote container shell access.
  /// Default: false
  #[serde(default, alias = "disable_container_exec")]
  pub disable_container_terminals: bool,

  /// The rate at which the system stats will be polled to update the cache.
  /// Options: https://docs.rs/komodo_client/latest/komodo_client/entities/enum.Timelength.html
  /// Default: `5-sec`
  #[serde(default = "default_stats_polling_rate")]
  pub stats_polling_rate: Timelength,

  /// The rate at which the container stats will be polled to update the cache.
  /// Options: https://docs.rs/komodo_client/latest/komodo_client/entities/enum.Timelength.html
  /// Default: `30-sec`
  #[serde(default = "default_container_stats_polling_rate")]
  pub container_stats_polling_rate: Timelength,

  /// Whether stack actions should use `docker-compose ...`
  /// instead of `docker compose ...`.
  /// Default: false
  #[serde(default)]
  pub legacy_compose_cli: bool,

  /// Logging configuration
  #[serde(default)]
  pub logging: LogConfig,

  /// Pretty-log (multi-line) the startup config
  /// for easier human readability.
  #[serde(default)]
  pub pretty_startup_config: bool,

  /// If non-empty, only includes specific mount paths in the disk report.
  #[serde(default)]
  pub include_disk_mounts: ForgivingVec<PathBuf>,

  /// Exclude specific mount paths in the disk report.
  #[serde(default)]
  pub exclude_disk_mounts: ForgivingVec<PathBuf>,

  /// Mapping on local periphery secrets. These can be interpolated into eg. Deployment environment variables.
  /// Default: none
  #[serde(default)]
  pub secrets: HashMap<String, String>,

  /// Configure git credentials used to clone private repos.
  /// Supports any git provider.
  #[serde(default, alias = "git_provider")]
  pub git_providers: ForgivingVec<GitProvider>,

  /// Configure docker credentials used to push / pull images.
  /// Supports any docker image repository.
  #[serde(default, alias = "docker_registry")]
  pub docker_registries: ForgivingVec<DockerRegistry>,
}

fn default_root_directory() -> PathBuf {
  "/etc/komodo".parse().unwrap()
}

fn default_default_terminal_command() -> String {
  String::from("bash")
}

fn default_stats_polling_rate() -> Timelength {
  Timelength::FiveSeconds
}

fn default_container_stats_polling_rate() -> Timelength {
  Timelength::ThirtySeconds
}

impl Default for PeripheryConfig {
  fn default() -> Self {
    Self {
      iroh_secret_key: None,
      onboarding_key: None,
      core_endpoint_addrs: Default::default(),
      connect_as: Default::default(),
      root_directory: default_root_directory(),
      repo_dir: None,
      stack_dir: None,
      build_dir: None,
      default_terminal_command: default_default_terminal_command(),
      disable_terminals: Default::default(),
      disable_container_terminals: Default::default(),
      stats_polling_rate: default_stats_polling_rate(),
      container_stats_polling_rate:
        default_container_stats_polling_rate(),
      legacy_compose_cli: Default::default(),
      logging: Default::default(),
      pretty_startup_config: Default::default(),
      include_disk_mounts: Default::default(),
      exclude_disk_mounts: Default::default(),
      secrets: Default::default(),
      git_providers: Default::default(),
      docker_registries: Default::default(),
    }
  }
}

impl PeripheryConfig {
  pub fn sanitized(&self) -> PeripheryConfig {
    PeripheryConfig {
      iroh_secret_key: self.iroh_secret_key.as_ref().map(|key| {
        if key.starts_with("file:") {
          key.clone()
        } else {
          empty_or_redacted(key)
        }
      }),
      onboarding_key: self
        .onboarding_key
        .as_ref()
        .map(|key| empty_or_redacted(key)),
      core_endpoint_addrs: self.core_endpoint_addrs.clone(),
      connect_as: self.connect_as.clone(),
      root_directory: self.root_directory.clone(),
      repo_dir: self.repo_dir.clone(),
      stack_dir: self.stack_dir.clone(),
      build_dir: self.build_dir.clone(),
      default_terminal_command: self.default_terminal_command.clone(),
      disable_terminals: self.disable_terminals,
      disable_container_terminals: self.disable_container_terminals,
      stats_polling_rate: self.stats_polling_rate,
      container_stats_polling_rate: self.container_stats_polling_rate,
      legacy_compose_cli: self.legacy_compose_cli,
      logging: self.logging.clone(),
      pretty_startup_config: self.pretty_startup_config,
      include_disk_mounts: self.include_disk_mounts.clone(),
      exclude_disk_mounts: self.exclude_disk_mounts.clone(),
      secrets: self
        .secrets
        .iter()
        .map(|(var, secret)| {
          (var.to_string(), empty_or_redacted(secret))
        })
        .collect(),
      git_providers: self
        .git_providers
        .iter()
        .map(|provider| GitProvider {
          domain: provider.domain.clone(),
          https: provider.https,
          accounts: provider
            .accounts
            .iter()
            .map(|account| ProviderAccount {
              username: account.username.clone(),
              token: empty_or_redacted(&account.token),
            })
            .collect(),
        })
        .collect(),
      docker_registries: self
        .docker_registries
        .iter()
        .map(|provider| DockerRegistry {
          domain: provider.domain.clone(),
          organizations: provider.organizations.clone(),
          accounts: provider
            .accounts
            .iter()
            .map(|account| ProviderAccount {
              username: account.username.clone(),
              token: empty_or_redacted(&account.token),
            })
            .collect(),
        })
        .collect(),
    }
  }

  pub fn repo_dir(&self) -> PathBuf {
    if let Some(dir) = &self.repo_dir {
      dir.to_owned()
    } else {
      self.root_directory.join("repos")
    }
  }

  pub fn stack_dir(&self) -> PathBuf {
    if let Some(dir) = &self.stack_dir {
      dir.to_owned()
    } else {
      self.root_directory.join("stacks")
    }
  }

  pub fn build_dir(&self) -> PathBuf {
    if let Some(dir) = &self.build_dir {
      dir.to_owned()
    } else {
      self.root_directory.join("builds")
    }
  }
}
