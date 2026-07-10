use std::str::FromStr;

use anyhow::Context;
use database::mungos::{
  by_id::update_one_by_id,
  find::find_collect,
  mongodb::bson::{self, Bson, doc, oid::ObjectId},
  update::Update,
};
use interpolate::Interpolator;
use komodo_client::entities::{
  build::{Build, ImageRegistryConfig},
  deployment::{
    Deployment, DeploymentImage, extract_registry_domain,
  },
  repo::Repo,
  server::{Server, ServerDesiredState, ServerState},
  stack::Stack,
};
use periphery_client::api::{
  compose::{ComposeExecution, ComposeUp},
  container::{RemoveContainer, RunContainer},
  placement::ReadContainerPorts,
  volume_backup::RestoreVolume,
};

use crate::{
  backup::{
    backup_deployment_volumes, backup_destination,
    backup_stack_volumes, parse_stack_volumes,
  },
  helpers::{
    periphery_client,
    query::{VariablesAndSecrets, get_variables_and_secrets},
    registry_token, stack_git_token,
  },
  placement, resource,
  state::db_client,
};

pub async fn run_drain_controller() {
  loop {
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    if let Err(e) = tick().await {
      tracing::warn!("Drain controller tick failed: {e}");
    }
  }
}

async fn tick() -> anyhow::Result<()> {
  let servers: Vec<Server> =
    find_collect(&db_client().servers, doc! {}, None).await?;

  for server in &servers {
    if server.config.desired_state != ServerDesiredState::Drain {
      continue;
    }
    if matches!(server.info.state, ServerState::Drained) {
      continue;
    }
    // Transition to Draining if not already. Drain transitions are monotone:
    // once Draining, the health poller's Ok/NotOk writes are overridden back
    // to Draining on the next tick until we reach Drained.
    if !matches!(server.info.state, ServerState::Draining) {
      update_one_by_id(
        &db_client().servers,
        &server.id,
        Update::Set(doc! { "info.state": "Draining" }),
        None,
      )
      .await?;
    }

    // Find idle (non-migrating) deployments on this server.
    let deployments: Vec<Deployment> = find_collect(
      &db_client().deployments,
      doc! {
        "info.assigned_server": &server.id,
        "info.migration_state": Bson::Null,
      },
      None,
    )
    .await?;

    // Find idle (non-migrating) stacks on this server, in parallel.
    let stacks: Vec<Stack> = find_collect(
      &db_client().stacks,
      doc! {
        "info.assigned_server": &server.id,
        "info.migration_state": Bson::Null,
      },
      None,
    )
    .await?;

    if deployments.is_empty() && stacks.is_empty() {
      update_one_by_id(
        &db_client().servers,
        &server.id,
        Update::Set(doc! { "info.state": "Drained" }),
        None,
      )
      .await?;
      continue;
    }

    if let Some(deployment) = deployments.first() {
      if let Err(e) = migrate_deployment(&deployment.id, None).await {
        tracing::warn!(
          "Deployment migration failed for {}: {e}",
          deployment.id
        );
        mark_deployment_failed(&deployment.id, &e.to_string()).await;
      }
    }

    if let Some(stack) = stacks.first() {
      if let Err(e) = migrate_stack(&stack.id, None).await {
        tracing::warn!(
          "Stack migration failed for {}: {e}",
          stack.id
        );
        mark_stack_failed(&stack.id, &e.to_string()).await;
      }
    }
    continue;
  }

  Ok(())
}

pub async fn migrate_deployment(
  deployment_id: &str,
  target_server_id: Option<&str>,
) -> anyhow::Result<()> {
  let mut deployment: Deployment = find_collect(
    &db_client().deployments,
    doc! { "_id": ObjectId::from_str(deployment_id)
    .context("Invalid deployment id ObjectId")? },
    None,
  )
  .await?
  .into_iter()
  .next()
  .context("Deployment not found")?;

  let source_server_id = deployment.info.assigned_server.clone();

  // Step 1: Mark as migrating. MigrationState is tagged
  // `#[serde(tag = "type", content = "params")]`, so build the discriminator
  // document manually to match that shape.
  let migration_doc = doc! {
    "type": "Migrating",
    "params": {
      "target_server_id": target_server_id.unwrap_or(""),
      "started_at": chrono::Utc::now().timestamp(),
    }
  };
  update_one_by_id(
    &db_client().deployments,
    deployment_id,
    Update::Set(doc! { "info.migration_state": migration_doc }),
    None,
  )
  .await?;

  // Step 2: Backup volumes on source.
  if !deployment.config.volumes.is_empty() {
    backup_deployment_volumes(deployment_id)
      .await
      .context("Backup failed during migration")?;
  }

  // Step 3: Pick target. pick_target takes host ports (u16), not PortMappings.
  let hint = target_server_id.unwrap_or("");
  let fixed_ports: Vec<u16> = deployment
    .config
    .ports
    .iter()
    .filter_map(|p| p.host)
    .collect();
  let target_id = placement::pick_target(&fixed_ports, hint)
    .await
    .context("Failed to pick target for migration")?;

  let target_server: Server = find_collect(
    &db_client().servers,
    doc! { "_id": ObjectId::from_str(&target_id)
    .context("Invalid target server id ObjectId")? },
    None,
  )
  .await?
  .into_iter()
  .next()
  .context("Target server not found")?;
  let target_periphery = periphery_client(&target_server).await?;

  // Step 4: Restore volumes on target.
  let dest = backup_destination()
    .context("Backup destination not configured")?;
  for vm in &deployment.config.volumes {
    let last_backup =
      deployment.info.last_backup.get(&vm.volume).with_context(
        || format!("No backup found for volume {}", vm.volume),
      )?;
    match target_periphery
      .request(RestoreVolume {
        deployment_id: deployment_id.to_string(),
        volume_name: vm.volume.clone(),
        source_key: last_backup.s3_key.clone(),
        destination: dest.clone(),
      })
      .await
    {
      Ok(_) => {}
      Err(e) => {
        return Err(e).context(format!(
          "RestoreVolume RPC failed for vol {}",
          vm.volume
        ));
      }
    }
  }

  // Step 5: Reassign to target by writing config.server_id.
  update_one_by_id(
    &db_client().deployments,
    deployment_id,
    Update::Set(doc! { "config.server_id": &target_id }),
    None,
  )
  .await?;

  // Step 5.5: Deploy container on target. The migration must actually start
  // the container on the target node — otherwise ReadContainerPorts and all
  // post-migration operations would fail on a non-existent container.
  deployment.config.server_id = target_id.clone();

  // Resolve Build images to concrete image strings (same as Deploy handler).
  let registry_token = match &deployment.config.image {
    DeploymentImage::Build { build_id, version } => {
      let build = resource::get::<Build>(build_id).await?;
      let image_name = build
        .get_image_names()
        .first()
        .context("No image name could be created from build")?
        .clone();
      let version = if version.is_none() {
        build.config.version
      } else {
        *version
      };
      let version_str = if build.config.image_tag.is_empty() {
        version.to_string()
      } else {
        format!("{version}-{}", build.config.image_tag)
      };
      deployment.config.image = DeploymentImage::Image {
        image: format!("{image_name}:{version_str}"),
      };
      let first_registry = build
        .config
        .image_registry
        .first()
        .unwrap_or(ImageRegistryConfig::static_default());
      if first_registry.domain.is_empty() {
        None
      } else {
        if deployment.config.image_registry_account.is_empty() {
          deployment.config.image_registry_account =
            first_registry.account.to_string();
        }
        if !deployment.config.image_registry_account.is_empty() {
          registry_token(
            &first_registry.domain,
            &deployment.config.image_registry_account,
          )
          .await?
        } else {
          None
        }
      }
    }
    DeploymentImage::Image { image } => {
      let domain = extract_registry_domain(image)?;
      if !deployment.config.image_registry_account.is_empty() {
        registry_token(
          &domain,
          &deployment.config.image_registry_account,
        )
        .await?
      } else {
        None
      }
    }
  };

  // Interpolate secrets (same as Deploy handler).
  let replacers = if !deployment.config.skip_secret_interp {
    let VariablesAndSecrets { variables, secrets } =
      get_variables_and_secrets().await?;
    let mut interpolator =
      Interpolator::new(Some(&variables), &secrets);
    interpolator.interpolate_deployment(&mut deployment)?;
    interpolator.secret_replacers.into_iter().collect()
  } else {
    Vec::new()
  };

  // Send RunContainer to target periphery.
  target_periphery
    .request(RunContainer {
      deployment: deployment.clone(),
      stop_signal: Some(deployment.config.termination_signal),
      stop_time: Some(deployment.config.termination_timeout),
      registry_token,
      replacers,
    })
    .await
    .context(
      "Failed to deploy container on target during migration",
    )?;

  // Step 6: Read back container ports on target.
  let host_ports_bson = {
    let r = target_periphery
      .request(ReadContainerPorts {
        container_name: deployment.name.clone(),
      })
      .await
      .context(
        "ReadContainerPorts failed on target after migration",
      )?;
    let arr: Vec<_> = r
      .ports
      .into_iter()
      .map(|p| doc! { "container": p.container as i32, "host": p.host as i32 })
      .collect();
    bson::to_bson(&arr).unwrap_or(Bson::Null)
  };

  // Step 7: Stop container on source (best-effort; source may be unreachable).
  if !source_server_id.is_empty() {
    let source_server: Option<Server> = find_collect(
      &db_client().servers,
      doc! { "_id": ObjectId::from_str(&source_server_id).ok() },
      None,
    )
    .await?
    .into_iter()
    .next();
    if let Some(source_server) = source_server {
      if let Ok(source_periphery) =
        periphery_client(&source_server).await
      {
        let _ = source_periphery
          .request(RemoveContainer {
            name: deployment.name.clone(),
            signal: Some(deployment.config.termination_signal),
            time: Some(deployment.config.termination_timeout),
          })
          .await;
      }
    }
  }

  // Step 8: Commit — set assigned_server, host_ports, clear migration_state.
  update_one_by_id(
    &db_client().deployments,
    deployment_id,
    Update::Set(doc! {
      "info.assigned_server": &target_id,
      "info.host_ports": host_ports_bson,
      "info.migration_state": Bson::Null,
    }),
    None,
  )
  .await?;

  Ok(())
}

pub async fn migrate_stack(
  stack_id: &str,
  _target_server_id: Option<&str>,
) -> anyhow::Result<()> {
  let mut stack: Stack = find_collect(
    &db_client().stacks,
    doc! { "_id": ObjectId::from_str(stack_id)
    .context("Invalid stack id ObjectId")? },
    None,
  )
  .await?
  .into_iter()
  .next()
  .context("Stack not found")?;

  let source_server_id = stack.info.assigned_server.clone();

  // Step 1: Mark as migrating.
  let migration_doc = doc! {
    "type": "Migrating",
    "params": {
      "target_server_id": _target_server_id.unwrap_or(""),
      "started_at": chrono::Utc::now().timestamp(),
    }
  };
  update_one_by_id(
    &db_client().stacks,
    stack_id,
    Update::Set(doc! { "info.migration_state": migration_doc }),
    None,
  )
  .await?;

  // Step 2: Backup volumes on source.
  if !stack.config.file_contents.is_empty() {
    backup_stack_volumes(stack_id)
      .await
      .context("Backup failed during stack migration")?;
    // Re-fetch the stack so we see the fresh last_backup map.
    stack = find_collect(
      &db_client().stacks,
      doc! { "_id": ObjectId::from_str(stack_id)
      .context("Invalid stack id ObjectId")? },
      None,
    )
    .await?
    .into_iter()
    .next()
    .context("Stack not found after backup")?;
  }

  // Step 3: Pick target. Stacks use compose files (no typed port list),
  // so we pick any healthy server.
  let target_id =
    placement::pick_target(&[], _target_server_id.unwrap_or(""))
      .await
      .context("Failed to pick target for stack migration")?;

  let target_server: Server = find_collect(
    &db_client().servers,
    doc! { "_id": ObjectId::from_str(&target_id)
    .context("Invalid target server id ObjectId")? },
    None,
  )
  .await?
  .into_iter()
  .next()
  .context("Target server not found")?;
  let target_periphery = periphery_client(&target_server).await?;

  // Step 4: Restore volumes on target.
  let dest = backup_destination()
    .context("Backup destination not configured")?;
  let project = stack.project_name(false);
  let volumes = parse_stack_volumes(&stack.config.file_contents)?;
  for vol_name in &volumes {
    let last_backup =
      stack.info.last_backup.get(vol_name).with_context(|| {
        format!("No backup found for volume {}", vol_name)
      })?;
    let podman_vol_name = format!("{project}_{vol_name}");
    target_periphery
      .request(RestoreVolume {
        deployment_id: stack_id.to_string(),
        volume_name: podman_vol_name,
        source_key: last_backup.s3_key.clone(),
        destination: dest.clone(),
      })
      .await
      .context(format!(
        "RestoreVolume RPC failed for vol {}",
        vol_name
      ))?;
  }

  // Step 5: Reassign to target.
  update_one_by_id(
    &db_client().stacks,
    stack_id,
    Update::Set(doc! { "config.server_id": &target_id }),
    None,
  )
  .await?;
  stack.config.server_id = target_id.clone();

  // Step 6: Deploy on target via ComposeUp.
  let mut repo = if !stack.config.files_on_host
    && !stack.config.linked_repo.is_empty()
  {
    resource::get::<Repo>(&stack.config.linked_repo)
      .await?
      .into()
  } else {
    None
  };

  let git_token = stack_git_token(&mut stack, repo.as_mut()).await?;
  let registry_token = registry_token(
    &stack.config.registry_provider,
    &stack.config.registry_account,
  )
  .await?;

  let replacers = if !stack.config.skip_secret_interp {
    let VariablesAndSecrets { variables, secrets } =
      get_variables_and_secrets().await?;
    let mut interpolator =
      Interpolator::new(Some(&variables), &secrets);
    interpolator.interpolate_stack(&mut stack)?;
    interpolator.secret_replacers.into_iter().collect()
  } else {
    Vec::new()
  };

  target_periphery
    .request(ComposeUp {
      stack: stack.clone(),
      services: Vec::new(),
      repo,
      git_token,
      registry_token,
      replacers,
    })
    .await
    .context("Failed to deploy stack on target during migration")?;

  // Step 7: Destroy on source (best-effort; source may be unreachable).
  if !source_server_id.is_empty() {
    let source_server: Option<Server> = find_collect(
      &db_client().servers,
      doc! { "_id": ObjectId::from_str(&source_server_id).ok() },
      None,
    )
    .await?
    .into_iter()
    .next();
    if let Some(source_server) = source_server {
      if let Ok(source_periphery) =
        periphery_client(&source_server).await
      {
        let _ = source_periphery
          .request(ComposeExecution {
            project: stack.project_name(false),
            command: "down".to_string(),
          })
          .await;
      }
    }
  }

  // Step 8: Commit — set assigned_server, clear migration_state.
  update_one_by_id(
    &db_client().stacks,
    stack_id,
    Update::Set(doc! {
      "info.assigned_server": &target_id,
      "info.migration_state": Bson::Null,
    }),
    None,
  )
  .await?;

  Ok(())
}

async fn mark_deployment_failed(deployment_id: &str, reason: &str) {
  let failed_doc = doc! {
    "type": "Failed",
    "params": {
      "reason": reason,
      "at": chrono::Utc::now().timestamp(),
    }
  };
  let _ = update_one_by_id(
    &db_client().deployments,
    deployment_id,
    Update::Set(doc! { "info.migration_state": failed_doc }),
    None,
  )
  .await;
}

async fn mark_stack_failed(stack_id: &str, reason: &str) {
  let failed_doc = doc! {
    "type": "Failed",
    "params": {
      "reason": reason,
      "at": chrono::Utc::now().timestamp(),
    }
  };
  let _ = update_one_by_id(
    &db_client().stacks,
    stack_id,
    Update::Set(doc! { "info.migration_state": failed_doc }),
    None,
  )
  .await;
}
