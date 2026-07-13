use anyhow::Context;
use database::mungos::mongodb::Collection;
use formatting::format_serror;
use indexmap::IndexSet;
use komodo_client::entities::{
  Operation, ResourceTarget, ResourceTargetVariant,
  build::Build,
  deployment::{
    Deployment, DeploymentConfig, DeploymentConfigDiff,
    DeploymentImage, DeploymentInfo, DeploymentListItem,
    DeploymentListItemInfo, DeploymentQuerySpecifics,
    DeploymentState, HttpProxyConfig, PartialDeploymentConfig,
  },
  environment_vars_from_str,
  permission::{
    PermissionLevel, PermissionLevelAndSpecifics, SpecificPermission,
  },
  resource::Resource,
  server::Server,
  to_container_compatible_name,
  update::Update,
  user::User,
};
use periphery_client::api::{
  container::RemoveContainer, placement::ReadContainerPorts,
};

use crate::{
  config::core_config,
  helpers::{
    empty_or_only_spaces, periphery_client,
    query::{get_deployment_state, get_server_for_command},
  },
  ingress::config::{CaddyRoute, build_caddy_config},
  monitor::refresh_server_cache,
  state::{
    action_states, all_resources_cache, db_client,
    deployment_status_cache,
  },
};

use super::get_check_permissions;

impl super::KomodoResource for Deployment {
  type Config = DeploymentConfig;
  type PartialConfig = PartialDeploymentConfig;
  type ConfigDiff = DeploymentConfigDiff;
  type Info = DeploymentInfo;
  type ListItem = DeploymentListItem;
  type QuerySpecifics = DeploymentQuerySpecifics;

  fn resource_type() -> ResourceTargetVariant {
    ResourceTargetVariant::Deployment
  }

  fn resource_target(id: impl Into<String>) -> ResourceTarget {
    ResourceTarget::Deployment(id.into())
  }

  fn validated_name(name: &str) -> String {
    to_container_compatible_name(name)
  }

  fn creator_specific_permissions() -> IndexSet<SpecificPermission> {
    [
      SpecificPermission::Inspect,
      SpecificPermission::Logs,
      SpecificPermission::Terminal,
    ]
    .into_iter()
    .collect()
  }

  fn inherit_specific_permissions_from(
    _self: &Resource<Self::Config, Self::Info>,
  ) -> Option<ResourceTarget> {
    if !_self.config.server_id.is_empty() {
      Some(ResourceTarget::Server(_self.config.server_id.clone()))
    } else {
      None
    }
  }

  fn coll() -> &'static Collection<Resource<Self::Config, Self::Info>>
  {
    &db_client().deployments
  }

  async fn to_list_item(
    deployment: Resource<Self::Config, Self::Info>,
  ) -> Self::ListItem {
    let status = deployment_status_cache().get(&deployment.id).await;
    let state = if action_states()
      .deployment
      .get(&deployment.id)
      .await
      .map(|s| s.get().map(|s| s.deploying))
      .transpose()
      .ok()
      .flatten()
      .unwrap_or_default()
    {
      DeploymentState::Deploying
    } else {
      status.as_ref().map(|s| s.curr.state).unwrap_or_default()
    };
    let all = all_resources_cache().load();
    let server_name = all
      .servers
      .get(&deployment.config.server_id)
      .map(|server| server.name.clone())
      .unwrap_or_default();
    let (build_image, build_id) = match deployment.config.image {
      DeploymentImage::Build { build_id, version } => {
        let (build_name, build_id, build_version) = all
          .builds
          .get(&build_id)
          .map(|b| (b.name.clone(), b.id.clone(), b.config.version))
          .unwrap_or((
            String::from("unknown"),
            String::new(),
            Default::default(),
          ));
        let version = if version.is_none() {
          build_version.to_string()
        } else {
          version.to_string()
        };
        (format!("{build_name}:{version}"), Some(build_id))
      }
      DeploymentImage::Image { image } => (image, None),
    };
    let (image, current_digests) = status
      .as_ref()
      .map(|s| {
        (
          s.curr.container.as_ref().map(|c| {
            c.image.clone().unwrap_or_else(|| String::from("Unknown"))
          }),
          s.curr.image_digests.as_ref(),
        )
      })
      .unwrap_or_default();
    let image = image.unwrap_or(build_image);
    let update_available = current_digests
      .map(|current_digests| {
        deployment
          .info
          .latest_image_digest
          .update_available(current_digests)
      })
      .unwrap_or_default();
    DeploymentListItem {
      name: deployment.name,
      id: deployment.id,
      template: deployment.template,
      tags: deployment.tags,
      resource_type: ResourceTargetVariant::Deployment,
      info: DeploymentListItemInfo {
        state,
        status: status.as_ref().and_then(|s| {
          s.curr.container.as_ref().and_then(|c| c.status.to_owned())
        }),
        image,
        update_available,
        server_id: deployment.config.server_id,
        server_name,
        build_id,
      },
    }
  }

  async fn busy(id: &String) -> anyhow::Result<bool> {
    action_states()
      .deployment
      .get(id)
      .await
      .unwrap_or_default()
      .busy()
  }

  // CREATE

  fn create_operation() -> Operation {
    Operation::CreateDeployment
  }

  fn user_can_create(user: &User) -> bool {
    user.admin || !core_config().disable_non_admin_create
  }

  async fn validate_create_config(
    config: &mut Self::PartialConfig,
    user: &User,
  ) -> anyhow::Result<()> {
    validate_config(config, user).await
  }

  async fn post_create(
    created: &Resource<Self::Config, Self::Info>,
    _update: &mut Update,
  ) -> anyhow::Result<()> {
    // Write the placement decision into info.assigned_server.
    let assigned_server = created.config.server_id.clone();
    if !assigned_server.is_empty() {
      database::mungos::by_id::update_one_by_id(
        &db_client().deployments,
        &created.id,
        database::mungos::update::Update::Set(
          database::mungos::mongodb::bson::doc! {
            "info.assigned_server": &assigned_server
          },
        ),
        None,
      )
      .await
      .context("Failed to set info.assigned_server")?;
    }
    if created.config.server_id.is_empty() {
      return Ok(());
    }
    let Ok(server) =
      get_server_for_command(&created.config.server_id)
        .await
        .inspect_err(|e| {
          warn!(
            "Failed to get Server for Deployment {} | {e:#}",
            created.name
          )
        })
    else {
      return Ok(());
    };
    refresh_server_cache(&server, true).await;
    // Read back container host ports so the Caddy ingress controller can
    // discover them. Best-effort: a failure here is logged, not fatal — the
    // deployment has been created successfully regardless.
    read_back_host_ports(&server, &created.name, &created.id).await;

    // If http_proxy is configured, create DNS record + push Caddy
    // config. Best-effort: failures are logged, not fatal.
    if let Some(http_proxy) = &created.config.http_proxy {
      if let Err(e) = try_setup_ingress(created, http_proxy).await {
        warn!(
          "Failed to set up ingress for deployment {}: {e:#}",
          created.name
        );
      }
    }
    Ok(())
  }

  // UPDATE

  fn update_operation() -> Operation {
    Operation::UpdateDeployment
  }

  async fn validate_update_config(
    _id: &str,
    config: &mut Self::PartialConfig,
    user: &User,
  ) -> anyhow::Result<()> {
    validate_config(config, user).await
  }

  async fn post_update(
    updated: &Self,
    update: &mut Update,
  ) -> anyhow::Result<()> {
    Self::post_create(updated, update).await
  }

  // RENAME

  fn rename_operation() -> Operation {
    Operation::RenameDeployment
  }

  // DELETE

  fn delete_operation() -> Operation {
    Operation::DeleteDeployment
  }

  async fn pre_delete(
    deployment: &Resource<Self::Config, Self::Info>,
    update: &mut Update,
  ) -> anyhow::Result<()> {
    if deployment.config.server_id.is_empty() {
      return Ok(());
    }
    let state = get_deployment_state(&deployment.id)
      .await
      .context("Failed to get deployment state")?;
    if matches!(
      state,
      DeploymentState::NotDeployed | DeploymentState::Unknown
    ) {
      return Ok(());
    }
    // container needs to be destroyed
    let server = match get_server_for_command(
      &deployment.config.server_id,
    )
    .await
    {
      Ok(res) => res,
      Err(e) => {
        update.push_error_log(
          "Remove Container / Service",
          format_serror(
            &e.context("Failed to retrieve Server from database")
              .into(),
          ),
        );
        return Ok(());
      }
    };

    if !server.config.enabled {
      // Don't need to
      update.push_simple_log(
        "Remove Container",
        "Skipping container removal, server is disabled.",
      );
      return Ok(());
    }
    let periphery = match periphery_client(&server).await {
      Ok(periphery) => periphery,
      Err(e) => {
        // This case won't ever happen, as periphery_client only fallible if the server is disabled.
        // Leaving it for completeness sake
        update.push_error_log(
          "Remove Container",
          format_serror(
            &e.context("Failed to get periphery client").into(),
          ),
        );
        return Ok(());
      }
    };
    match periphery
      .request(RemoveContainer {
        name: deployment.name.clone(),
        signal: deployment.config.termination_signal.into(),
        time: deployment.config.termination_timeout.into(),
      })
      .await
    {
      Ok(log) => update.logs.push(log),
      Err(e) => update.push_error_log(
        "Remove Container",
        format_serror(
          &e.context("Failed to remove container").into(),
        ),
      ),
    };

    Ok(())
  }

  async fn post_delete(
    resource: &Resource<Self::Config, Self::Info>,
    _update: &mut Update,
  ) -> anyhow::Result<()> {
    deployment_status_cache().remove(&resource.id).await;

    // Best-effort: delete DNS records + push updated Caddy config.
    if resource.config.http_proxy.is_some() {
      if let Err(e) = try_teardown_ingress(&resource.id).await {
        warn!(
          "Failed to tear down ingress for deployment {}: {e:#}",
          resource.id
        );
      }
    }
    Ok(())
  }
}

#[instrument("ValidateDeploymentConfig", skip_all)]
async fn validate_config(
  config: &mut PartialDeploymentConfig,
  user: &User,
) -> anyhow::Result<()> {
  if let Some(server_id) = &config.server_id
    && !server_id.is_empty()
  {
    let server = get_check_permissions::<Server>(
      server_id,
      user,
      PermissionLevel::Read.attach(),
    )
    .await
    .context("Cannot attach Deployment to this Server")?;
    config.server_id = Some(server.id);
  }
  // Only run the placement scheduler when server_id was explicitly part
  // of this partial config (Some). If it's None the caller didn't touch
  // server_id, so re-running the scheduler would overwrite the existing
  // assignment.
  if config.server_id.is_none() {
    return Ok(());
  }
  // Placement scheduling. The user's `config.server_id` (possibly empty)
  // is treated as a hint; the scheduler probes candidate periphery nodes
  // for free host ports and picks one.
  let hint = config.server_id.clone().unwrap_or_default();
  let fixed_ports: Vec<u16> = config
    .ports
    .as_ref()
    .map(|ps| ps.iter().filter_map(|pm| pm.host).collect())
    .unwrap_or_default();
  let chosen = crate::placement::pick_target(&fixed_ports, &hint)
    .await
    .map_err(|e| anyhow::anyhow!("Placement failed: {e}"))?;
  config.server_id = Some(chosen);
  if let Some(DeploymentImage::Build { build_id, version }) =
    &config.image
    && !build_id.is_empty()
  {
    let build = get_check_permissions::<Build>(
      build_id,
      user,
      PermissionLevel::Read.attach(),
    )
    .await
    .context("Cannot update deployment with this build attached.")?;
    config.image = Some(DeploymentImage::Build {
      build_id: build.id,
      version: *version,
    });
  }
  if let Some(environment) = &config.environment {
    environment_vars_from_str(environment)
      .context("Invalid environment")?;
  }
  if let Some(extra_args) = &mut config.extra_args {
    extra_args.retain(|v| !empty_or_only_spaces(v))
  }
  Ok(())
}

/// Queries the periphery for the container's host port bindings and writes
/// them into `info.host_ports` in the database. Best-effort: failures are
/// logged at warn-level and do not propagate, so a transient periphery error
/// never fails an otherwise-successful create/update.
///
/// This mirrors the readback performed by the drain migration path
/// (`bin/core/src/server/drain.rs:324`).
async fn read_back_host_ports(
  server: &Server,
  container_name: &str,
  deployment_id: &str,
) {
  let periphery = match periphery_client(server).await {
    Ok(p) => p,
    Err(e) => {
      warn!(
        "ReadContainerPorts: failed to connect to periphery for Deployment {container_name} | {e:#}"
      );
      return;
    }
  };
  let response = match periphery
    .request(ReadContainerPorts {
      container_name: container_name.to_string(),
    })
    .await
  {
    Ok(r) => r,
    Err(e) => {
      warn!(
        "ReadContainerPorts: query failed for Deployment {container_name} | {e:#}"
      );
      return;
    }
  };
  let arr: Vec<_> = response
    .ports
    .into_iter()
    .map(|p| {
      database::mungos::mongodb::bson::doc! {
        "container": p.container as i32,
        "host": p.host as i32
      }
    })
    .collect();
  let host_ports_bson =
    database::mungos::mongodb::bson::to_bson(&arr)
      .unwrap_or(database::mungos::mongodb::bson::Bson::Null);
  if let Err(e) = database::mungos::by_id::update_one_by_id(
    &db_client().deployments,
    deployment_id,
    database::mungos::update::Update::Set(
      database::mungos::mongodb::bson::doc! {
        "info.host_ports": host_ports_bson
      },
    ),
    None,
  )
  .await
  {
    warn!(
      "ReadContainerPorts: failed to persist host_ports for Deployment {container_name} | {e:#}"
    );
  }
}

pub async fn setup_deployment_execution(
  deployment: &str,
  user: &User,
  required_permissions: PermissionLevelAndSpecifics,
) -> anyhow::Result<(Deployment, Server)> {
  let deployment = get_check_permissions::<Deployment>(
    deployment,
    user,
    required_permissions,
  )
  .await?;

  let server =
    get_server_for_command(&deployment.config.server_id).await?;

  Ok((deployment, server))
}

/// Default HTTP bridge port for ingress periphery nodes. Each
/// periphery listens on `127.0.0.1:{bridge_port}` for Iroh-routed
/// traffic; Caddy reverse_proxies to it. The actual port is a
/// periphery config value not exposed on the Core-side `Server`
/// entity, so the default (8443) is used.
const DEFAULT_BRIDGE_PORT: u16 = 8443;

/// Best-effort: set up DNS + Caddy ingress for a deployment with
/// `http_proxy`.
///
/// 1. Select a healthy ingress-enabled node.
/// 2. Create a DNS A/AAAA record pointing the deployment subdomain
///    at the ingress node's public IPs.
/// 3. Build the complete Caddy JSON config (all http_proxy routes)
///    and push it to the ingress Periphery via `ReloadCaddyConfig`.
async fn try_setup_ingress(
  deployment: &Deployment,
  http_proxy: &HttpProxyConfig,
) -> anyhow::Result<()> {
  use crate::ingress::{
    failover::select_new_ingress_node,
    management::create_deployment_dns_record,
  };

  let core_cfg = core_config().ingress.clone();
  let base_domain = core_cfg
    .dns
    .base_domain
    .as_deref()
    .filter(|d| !d.is_empty())
    .ok_or_else(|| {
      anyhow::anyhow!(
        "ingress.dns.base_domain not configured — cannot set up ingress"
      )
    })?;
  let fqdn = format!("{}.{}", http_proxy.subdomain, base_domain);

  // Find the deployment's server for the Iroh endpoint_id.
  let server =
    get_server_for_command(&deployment.config.server_id).await?;
  let target_endpoint_id = server.info.endpoint_id.clone();
  if target_endpoint_id.is_empty() {
    anyhow::bail!(
      "Server {} has no endpoint_id — cannot route ingress traffic",
      server.id
    );
  }

  // Find the target host port matching http_proxy.container_port.
  let host_port = deployment
    .info
    .host_ports
    .iter()
    .find(|p| p.container == http_proxy.container_port)
    .map(|p| p.host)
    .ok_or_else(|| {
      anyhow::anyhow!(
        "No host port found for container port {} on deployment {}. \
         ReadContainerPorts readback may not have completed.",
        http_proxy.container_port,
        deployment.name
      )
    })?;

  // Select a healthy ingress node.
  let ingress_node = select_new_ingress_node("").await?;

  // Create DNS record(s) pointing to the ingress node.
  create_deployment_dns_record(
    &deployment.id,
    &http_proxy.subdomain,
    &ingress_node.id,
    ingress_node.config.public_ipv4.as_deref(),
    ingress_node.config.public_ipv6.as_deref(),
    &core_cfg,
    60,
  )
  .await?;

  // Build + push Caddy config (all routes, including this one).
  let routes = build_ingress_routes(
    base_domain,
    &core_cfg.dns.cloudflare_api_token,
  )
  .await?;
  let caddy_config = build_caddy_config(
    &routes,
    &core_cfg
      .dns
      .cloudflare_api_token
      .clone()
      .unwrap_or_default(),
    DEFAULT_BRIDGE_PORT,
  );

  let periphery = periphery_client(&ingress_node).await?;
  periphery
    .request(periphery_client::api::ReloadCaddyConfig {
      config: caddy_config,
    })
    .await?;

  info!(
    "Set up ingress for deployment {}: {} -> endpoint {}:{}",
    deployment.name, fqdn, target_endpoint_id, host_port
  );
  Ok(())
}

/// Best-effort: delete DNS records + push updated Caddy config
/// (without the deleted route).
async fn try_teardown_ingress(
  deployment_id: &str,
) -> anyhow::Result<()> {
  use crate::ingress::management::delete_deployment_dns_records;

  let core_cfg = core_config().ingress.clone();

  // Delete DNS records at the provider + in the database.
  delete_deployment_dns_records(deployment_id, &core_cfg).await?;

  // Push updated Caddy config (without the deleted route).
  let base_domain = core_cfg
    .dns
    .base_domain
    .as_deref()
    .filter(|d| !d.is_empty())
    .ok_or_else(|| {
      anyhow::anyhow!(
        "ingress.dns.base_domain not configured — cannot rebuild Caddy config"
      )
    })?;
  let routes = build_ingress_routes(
    base_domain,
    &core_cfg.dns.cloudflare_api_token,
  )
  .await?;
  let caddy_config = build_caddy_config(
    &routes,
    &core_cfg
      .dns
      .cloudflare_api_token
      .clone()
      .unwrap_or_default(),
    DEFAULT_BRIDGE_PORT,
  );

  // Find the ingress node and push.
  let ingress_node =
    crate::ingress::failover::select_new_ingress_node("").await?;
  let periphery = periphery_client(&ingress_node).await?;
  periphery
    .request(periphery_client::api::ReloadCaddyConfig {
      config: caddy_config,
    })
    .await?;

  info!("Tore down ingress for deployment {}", deployment_id);
  Ok(())
}

/// Build Caddy routes for **all** deployments that have `http_proxy`
/// configured. Each route maps the deployment's FQDN to the target
/// server's Iroh endpoint_id + host port.
///
/// This is called on every create/delete so the pushed config always
/// reflects the current set of proxied deployments.
async fn build_ingress_routes(
  base_domain: &str,
  _cloudflare_api_token: &Option<String>,
) -> anyhow::Result<Vec<CaddyRoute>> {
  use database::mungos::{find::find_collect, mongodb::bson::doc};

  // Query all deployments that have http_proxy set.
  let deployments: Vec<Deployment> = find_collect(
    &db_client().deployments,
    doc! { "config.http_proxy": { "$ne": null } },
    None,
  )
  .await
  .context("failed to query deployments with http_proxy")?;

  let mut routes = Vec::new();
  for dep in deployments {
    let Some(http_proxy) = &dep.config.http_proxy else {
      continue;
    };
    // Get the server for this deployment to find endpoint_id.
    let server =
      get_server_for_command(&dep.config.server_id).await.ok();
    let Some(server) = server else {
      warn!(
        "build_ingress_routes: could not get server for deployment {} (server_id={}), skipping",
        dep.name, dep.config.server_id
      );
      continue;
    };
    let endpoint_id = server.info.endpoint_id.clone();
    if endpoint_id.is_empty() {
      continue;
    }
    // Find the host port matching the container_port.
    let host_port = dep
      .info
      .host_ports
      .iter()
      .find(|p| p.container == http_proxy.container_port)
      .map(|p| p.host);
    let Some(host_port) = host_port else {
      warn!(
        "build_ingress_routes: no host port for container port {} on deployment {}, skipping",
        http_proxy.container_port, dep.name
      );
      continue;
    };
    routes.push(CaddyRoute {
      hostname: format!("{}.{}", http_proxy.subdomain, base_domain),
      target_endpoint_id: endpoint_id,
      target_port: host_port,
    });
  }
  Ok(routes)
}
