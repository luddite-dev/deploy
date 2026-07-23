use std::collections::HashMap;

use anyhow::Context;
use database::mungos::mongodb::Collection;
use formatting::format_serror;
use indexmap::IndexSet;
use komodo_client::{
  api::write::RefreshStackCache,
  entities::{
    Operation, ResourceTarget, ResourceTargetVariant,
    deployment::AssignedPort,
    permission::{PermissionLevel, SpecificPermission},
    repo::Repo,
    resource::Resource,
    server::Server,
    stack::{
      PartialStackConfig, Stack, StackConfig, StackConfigDiff,
      StackHttpProxyConfig, StackInfo, StackListItem,
      StackListItemInfo, StackQuerySpecifics, StackServiceNames,
      StackServiceWithUpdate, StackState,
    },
    to_docker_compatible_name,
    update::Update,
    user::{User, stack_user},
  },
};
use mogh_resolver::Resolve;
use periphery_client::api::{
  compose::ComposeExecution, placement::ReadContainerPorts,
};

use crate::{
  api::write::WriteArgs,
  config::core_config,
  helpers::{
    periphery_client,
    query::{get_server_for_command, get_stack_state},
    repo_link,
  },
  ingress::{
    config::build_caddy_config,
    failover::select_new_ingress_node,
    management::{create_stack_dns_record, delete_stack_dns_records},
  },
  monitor::refresh_server_cache,
  resource::deployment::{DEFAULT_BRIDGE_PORT, build_ingress_routes},
  state::{
    action_states, all_resources_cache, db_client,
    server_status_cache, stack_status_cache,
  },
};

use super::get_check_permissions;

impl super::KomodoResource for Stack {
  type Config = StackConfig;
  type PartialConfig = PartialStackConfig;
  type ConfigDiff = StackConfigDiff;
  type Info = StackInfo;
  type ListItem = StackListItem;
  type QuerySpecifics = StackQuerySpecifics;

  fn resource_type() -> ResourceTargetVariant {
    ResourceTargetVariant::Stack
  }

  fn resource_target(id: impl Into<String>) -> ResourceTarget {
    ResourceTarget::Stack(id.into())
  }

  fn validated_name(name: &str) -> String {
    to_docker_compatible_name(name)
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
    &db_client().stacks
  }

  async fn to_list_item(
    stack: Resource<Self::Config, Self::Info>,
  ) -> Self::ListItem {
    let status = stack_status_cache().get(&stack.id).await;
    let state = if action_states()
      .stack
      .get(&stack.id)
      .await
      .map(|s| s.get().map(|s| s.deploying))
      .transpose()
      .ok()
      .flatten()
      .unwrap_or_default()
    {
      StackState::Deploying
    } else {
      status.as_ref().map(|s| s.curr.state).unwrap_or_default()
    };
    let project_name = stack.project_name(false);
    let services = status
      .as_ref()
      .map(|s| {
        s.curr
          .services
          .iter()
          .map(|current_service| {
            let update_available = current_service
              .image_digests
              .as_ref()
              .map(|current_digests| {
                stack
                  .info
                  .latest_services
                  .iter()
                  .find_map(|latest_service| {
                    if current_service.service
                      == latest_service.service_name
                    {
                      latest_service
                        .image_digest
                        .as_ref()?
                        .update_available(current_digests)
                        .into()
                    } else {
                      None
                    }
                  })
                  .unwrap_or_default()
              })
              .unwrap_or_default();
            StackServiceWithUpdate {
              service: current_service.service.clone(),
              image: current_service.image.clone(),
              update_available,
            }
          })
          .collect::<Vec<_>>()
      })
      .unwrap_or_default();

    let default_git = (
      stack.config.git_provider,
      stack.config.repo,
      stack.config.branch,
      stack.config.git_https,
    );
    let (git_provider, repo, branch, git_https) =
      if stack.config.linked_repo.is_empty() {
        default_git
      } else {
        all_resources_cache()
          .load()
          .repos
          .get(&stack.config.linked_repo)
          .map(|r| {
            (
              r.config.git_provider.clone(),
              r.config.repo.clone(),
              r.config.branch.clone(),
              r.config.git_https,
            )
          })
          .unwrap_or(default_git)
      };

    // This is only true if it is KNOWN to be true. so other cases are false.
    let (project_missing, status) =
      if matches!(state, StackState::Down | StackState::Unknown) {
        (false, None)
      } else if !stack.config.server_id.is_empty()
        && let Some(status) = server_status_cache()
          .get(&stack.config.server_id)
          .await
          .as_ref()
      {
        if let Some(docker) = &status.docker {
          if let Some(project) = docker
            .projects
            .iter()
            .find(|project| project.name == project_name)
          {
            (false, project.status.clone())
          } else {
            // The project doesn't exist
            (true, None)
          }
        } else {
          (false, None)
        }
      } else {
        (false, None)
      };

    let all = all_resources_cache().load();
    let server_name = all
      .servers
      .get(&stack.config.server_id)
      .map(|server| server.name.clone())
      .unwrap_or_default();

    StackListItem {
      name: stack.name,
      id: stack.id,
      template: stack.template,
      tags: stack.tags,
      resource_type: ResourceTargetVariant::Stack,
      info: StackListItemInfo {
        state,
        status,
        services,
        project_missing,
        file_contents: !stack.config.file_contents.is_empty(),
        server_id: stack.config.server_id,
        server_name,
        linked_repo: stack.config.linked_repo,
        missing_files: stack.info.missing_files,
        files_on_host: stack.config.files_on_host,
        repo_link: repo_link(
          &git_provider,
          &repo,
          &branch,
          git_https,
        ),
        git_provider,
        repo,
        branch,
        latest_hash: stack.info.latest_hash,
        deployed_hash: stack.info.deployed_hash,
      },
    }
  }

  async fn busy(id: &String) -> anyhow::Result<bool> {
    action_states()
      .stack
      .get(id)
      .await
      .unwrap_or_default()
      .busy()
  }

  // CREATE

  fn create_operation() -> Operation {
    Operation::CreateStack
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
    update: &mut Update,
  ) -> anyhow::Result<()> {
    // Write the placement decision into info.assigned_server.
    let assigned_server = created.config.server_id.clone();
    if !assigned_server.is_empty() {
      database::mungos::by_id::update_one_by_id(
        &db_client().stacks,
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
    // TODO(Task 8): ReadContainerPorts readback to populate info.host_ports.
    //
    // DEFERRED: The stack host_ports field is a HashMap<String,
    // Vec<AssignedPort>> keyed by compose service name. Unlike deployments
    // (where the container name == stack.name), stack service containers use
    // a compose project naming convention that is not trivially available
    // at post_create time — the services list is populated by the
    // RefreshStackCache call above, but resolving per-service container
    // names and iterating ReadContainerPorts for each is a larger change.
    //
    // The drain migration path (`bin/core/src/server/drain.rs:553`) also
    // skips host_ports for stacks, confirming there is no existing pattern
    // to mirror. Stack HTTP proxying (via Caddy) can be added in a future
    // task once service-name discovery is implemented.
    if let Err(e) = (RefreshStackCache {
      stack: created.name.clone(),
    })
    .resolve(&WriteArgs {
      user: stack_user().to_owned(),
    })
    .await
    {
      update.push_error_log(
        "Refresh stack cache",
        format_serror(&e.error.context("The stack cache has failed to refresh. This may be due to a misconfiguration of the Stack").into())
      );
    };
    if created.config.server_id.is_empty() {
      return Ok(());
    }
    let Ok(server) =
      get_server_for_command(&created.config.server_id)
        .await
        .inspect_err(|e| {
          warn!(
            "Failed to get Server for Stack {} | {e:#}",
            created.name
          )
        })
    else {
      return Ok(());
    };
    refresh_server_cache(&server, true).await;
    Ok(())
  }

  // UPDATE

  fn update_operation() -> Operation {
    Operation::UpdateStack
  }

  async fn validate_update_config(
    _id: &str,
    config: &mut Self::PartialConfig,
    user: &User,
  ) -> anyhow::Result<()> {
    validate_config(config, user).await
  }

  async fn post_update(
    updated: &Resource<Self::Config, Self::Info>,
    update: &mut Update,
  ) -> anyhow::Result<()> {
    Self::post_create(updated, update).await
  }

  // RENAME

  fn rename_operation() -> Operation {
    Operation::RenameStack
  }

  // DELETE

  fn delete_operation() -> Operation {
    Operation::DeleteStack
  }

  async fn pre_delete(
    stack: &Resource<Self::Config, Self::Info>,
    update: &mut Update,
  ) -> anyhow::Result<()> {
    // If it is Up, it should be taken down
    let state = get_stack_state(stack)
      .await
      .context("failed to get stack state")?;
    if matches!(state, StackState::Down | StackState::Unknown) {
      return Ok(());
    }
    // stack needs to be destroyed
    let server =
      match get_server_for_command(&stack.config.server_id).await {
        Ok(res) => res,
        Err(e) => {
          update.push_error_log(
            "Destroy Stack",
            format_serror(
              &e.context("Failed to retrieve Server from database.")
                .into(),
            ),
          );
          return Ok(());
        }
      };

    if !server.config.enabled {
      update.push_simple_log(
        "Destroy Stack",
        "Skipping stack destroy, Server is disabled.",
      );
      return Ok(());
    }

    let periphery = match periphery_client(&server).await {
      Ok(periphery) => periphery,
      Err(e) => {
        // This case won't ever happen, as periphery_client only fallible if the server is disabled.
        // Leaving it for completeness sake
        update.push_error_log(
          "Destroy Stack",
          format_serror(
            &e.context("Failed to get periphery client").into(),
          ),
        );
        return Ok(());
      }
    };

    match periphery
      .request(ComposeExecution {
        project: stack.project_name(false),
        command: String::from("down --remove-orphans"),
      })
      .await
    {
      Ok(log) => update.logs.push(log),
      Err(e) => update.push_simple_log(
        "Failed to destroy stack",
        format_serror(
          &e.context(
            "failed to destroy stack on periphery server before delete",
          )
          .into(),
        ),
      ),
    };

    Ok(())
  }

  async fn post_delete(
    resource: &Resource<Self::Config, Self::Info>,
    _update: &mut Update,
  ) -> anyhow::Result<()> {
    // Best-effort: tear down DNS + Caddy ingress for stacks that had
    // http_proxy configured. Done first so the route is removed even
    // if later cleanup steps were to fail.
    if resource.config.http_proxy.is_some() {
      if let Err(e) = try_teardown_stack_ingress(&resource.id).await {
        warn!(
          "Failed to tear down ingress for stack {} | {e:#}",
          resource.id
        );
      }
    }
    stack_status_cache().remove(&resource.id).await;
    Ok(())
  }
}

#[instrument("ValidateStackConfig", skip_all)]
async fn validate_config(
  config: &mut PartialStackConfig,
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
    .context("Cannot attach Stack to this Server")?;
    // in case it comes in as name
    config.server_id = Some(server.id);
  }
  // Validate compose YAML: reject bind mounts and Swarm-only keys.
  if let Some(file_contents) = &config.file_contents
    && !file_contents.is_empty()
  {
    crate::resource::stack_validation::validate_compose_yaml(
      file_contents,
    )
    .context("Invalid compose file")?;
  }
  // Only run the placement scheduler when server_id was explicitly part
  // of this partial config (Some). If it's None the caller didn't touch
  // server_id, so re-running the scheduler would overwrite the existing
  // assignment.
  if config.server_id.is_none() {
    return Ok(());
  }
  // Placement scheduling. Stacks carry their service ports inside the
  // compose YAML rather than typed PortMappings, so fixed-port detection
  // is deferred to compose parsing (Task 5). For now we run pick_target
  // with no fixed ports: any healthy server is eligible.
  let hint = config.server_id.clone().unwrap_or_default();
  let chosen = crate::placement::pick_target(&[], &hint)
    .await
    .map_err(|e| anyhow::anyhow!("Placement failed: {e}"))?;
  config.server_id = Some(chosen);
  if let Some(linked_repo) = &config.linked_repo
    && !linked_repo.is_empty()
  {
    let repo = get_check_permissions::<Repo>(
      linked_repo,
      user,
      PermissionLevel::Read.attach(),
    )
    .await
    .context("Cannot attach Repo to this Stack")?;
    // in case it comes in as name
    config.linked_repo = Some(repo.id);
  }
  Ok(())
}

/// Queries the periphery for each service container's host port
/// bindings and writes them into `info.host_ports` in the database.
/// Best-effort: per-service failures are logged at warn-level and
/// skipped, so a transient periphery error never fails an
/// otherwise-successful deploy.
///
/// Mirrors `read_back_host_ports` for deployments
/// (`bin/core/src/resource/deployment.rs:453`), but iterates over
/// every compose service and keys results by service name
/// (`HashMap<String, Vec<AssignedPort>>`).
pub async fn read_back_stack_host_ports(
  stack: &Stack,
  service_names: &[StackServiceNames],
) -> HashMap<String, Vec<AssignedPort>> {
  // Resolve the stack's server (same fallback as try_setup_stack_ingress).
  let server_id = if !stack.config.server_id.is_empty() {
    &stack.config.server_id
  } else {
    &stack.info.assigned_server
  };
  let server = match get_server_for_command(server_id).await {
    Ok(s) => s,
    Err(e) => {
      warn!(
        "ReadContainerPorts: failed to resolve server for Stack {} | {e:#}",
        stack.name
      );
      return HashMap::new();
    }
  };
  let periphery = match periphery_client(&server).await {
    Ok(p) => p,
    Err(e) => {
      warn!(
        "ReadContainerPorts: failed to connect to periphery for Stack {} | {e:#}",
        stack.name
      );
      return HashMap::new();
    }
  };

  let mut host_ports: HashMap<String, Vec<AssignedPort>> =
    HashMap::new();
  for svc in service_names {
    match periphery
      .request(ReadContainerPorts {
        container_name: svc.container_name.clone(),
      })
      .await
    {
      Ok(r) => {
        host_ports.insert(svc.service_name.clone(), r.ports);
      }
      Err(e) => {
        warn!(
          "ReadContainerPorts: query failed for Stack service {} ({}) | {e:#}",
          svc.service_name, svc.container_name
        );
      }
    }
  }

  let host_ports_bson =
    database::mungos::mongodb::bson::to_bson(&host_ports)
      .unwrap_or(database::mungos::mongodb::bson::Bson::Null);
  if let Err(e) = database::mungos::by_id::update_one_by_id(
    &db_client().stacks,
    &stack.id,
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
      "ReadContainerPorts: failed to persist host_ports for Stack {} | {e:#}",
      stack.name
    );
  }

  host_ports
}

/// Best-effort: set up DNS + Caddy ingress for a stack with
/// `http_proxy`.
///
/// Mirrors `try_setup_ingress` for deployments:
/// 1. Resolve the stack's assigned server → endpoint_id.
/// 2. Find the proxied service's host port from
///    `info.host_ports[http_proxy.service]`.
/// 3. Select a healthy ingress-enabled node and read its cached
///    public IPs.
/// 4. Create a DNS record keyed by stack_id pointing at the ingress
///    node's public IPs.
/// 5. Build the complete Caddy JSON config (all http_proxy routes,
///    deployments + stacks) and push it to the ingress Periphery via
///    `ReloadCaddyConfig`.
pub async fn try_setup_stack_ingress(
  stack: &Stack,
  http_proxy: &StackHttpProxyConfig,
) -> anyhow::Result<()> {
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

  // Find the stack's server for the Iroh endpoint_id. Prefer
  // config.server_id, fall back to the assigned_server populated by
  // the placement scheduler.
  let server_id = if !stack.config.server_id.is_empty() {
    &stack.config.server_id
  } else {
    &stack.info.assigned_server
  };
  let server = get_server_for_command(server_id).await?;
  let target_endpoint_id = server.info.endpoint_id.clone();
  if target_endpoint_id.is_empty() {
    anyhow::bail!(
      "Server {} has no endpoint_id — cannot route ingress traffic",
      server.id
    );
  }

  // Find the target host port matching http_proxy.container_port on
  // the proxied compose service. Stack host_ports are keyed by
  // service name (HashMap<String, Vec<AssignedPort>>), unlike
  // deployments which use a flat Vec.
  let host_port = stack
    .info
    .host_ports
    .get(&http_proxy.service)
    .and_then(|ports| {
      ports
        .iter()
        .find(|p| p.container == http_proxy.container_port)
    })
    .map(|p| p.host)
    .ok_or_else(|| {
      anyhow::anyhow!(
        "No host port found for container port {} on service {} of \
         stack {}. ReadContainerPorts readback may not have \
         completed.",
        http_proxy.container_port,
        http_proxy.service,
        stack.name
      )
    })?;

  // Select a healthy ingress node.
  let ingress_node = select_new_ingress_node("").await?;

  // Read the ingress node's public IPs from its cached
  // PeripheryInformation (populated on every PollStatus cycle —
  // ~stats_polling_rate cadence, default 5-15s).
  let cache_entry = server_status_cache().get(&ingress_node.id).await;
  let (target_ipv4, target_ipv6) = cache_entry
    .as_ref()
    .and_then(|s| s.periphery_info.as_ref())
    .map(|info| (info.public_ipv4.clone(), info.public_ipv6.clone()))
    .unwrap_or((None, None));

  if target_ipv4.is_none() && target_ipv6.is_none() {
    anyhow::bail!(
      "ingress node {} has no cached public_ipv4/v6 — \
       wait for the next poll cycle (default ~5-15s), or set \
       PERIPHERY_PUBLIC_IPV4 / _IPV6 on the Periphery host and \
       restart it",
      ingress_node.id
    );
  }

  // Create DNS record(s) pointing to the ingress node, keyed by
  // stack_id.
  create_stack_dns_record(
    &stack.id,
    &http_proxy.subdomain,
    &ingress_node.id,
    target_ipv4.as_deref(),
    target_ipv6.as_deref(),
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
    "Set up ingress for stack {}: {} -> endpoint {}:{}",
    stack.name, fqdn, target_endpoint_id, host_port
  );
  Ok(())
}

/// Best-effort: delete DNS records + push updated Caddy config
/// (without the deleted stack's route).
///
/// Mirrors `try_teardown_ingress` for deployments.
async fn try_teardown_stack_ingress(
  stack_id: &str,
) -> anyhow::Result<()> {
  let core_cfg = core_config().ingress.clone();

  // Delete DNS records at the provider + in the database.
  delete_stack_dns_records(stack_id, &core_cfg).await?;

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
  let ingress_node = select_new_ingress_node("").await?;
  let periphery = periphery_client(&ingress_node).await?;
  periphery
    .request(periphery_client::api::ReloadCaddyConfig {
      config: caddy_config,
    })
    .await?;

  info!("Tore down ingress for stack {}", stack_id);
  Ok(())
}
