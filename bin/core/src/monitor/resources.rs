use anyhow::Context;
use komodo_client::entities::{
  ImageDigest,
  deployment::{Deployment, DeploymentState},
  docker::{container::ContainerListItem, image::ImageListItem},
  stack::{
    Stack, StackService, StackServiceNames, StackServiceState,
  },
};

use crate::{
  helpers::query::get_stack_state_from_containers,
  stack::{
    compose_container_match_regex,
    services::extract_services_from_stack,
  },
  state::{
    CachedDeploymentStatus, CachedStackStatus, History,
    deployment_status_cache, stack_status_cache,
  },
};

pub async fn update_server_stack_cache(
  stacks: Vec<Stack>,
  containers: &[ContainerListItem],
  images: &[ImageListItem],
) {
  let stack_status_cache = stack_status_cache();
  for stack in stacks {
    let services = extract_services_from_stack(&stack);
    let mut services_with_containers = services.iter().map(|StackServiceNames { service_name, container_name, image, .. }| {
      // Get the container associated with service.
      let container = containers.iter().find(|container| {
        match compose_container_match_regex(container_name)
          .with_context(|| format!("failed to construct container name matching regex for service {service_name}")) 
        {
          Ok(regex) => regex,
          Err(e) => {
            warn!("{e:#}");
            return false
          }
        }.is_match(&container.name)
      }).cloned();

      let (image, image_digests) = container
        .as_ref()
        .and_then(|container| container.image_id.as_ref())
        .and_then(|image_id| {
          images.iter().find(|image| {
            &image.id == image_id
          })
        })
        .map(|image| (
          image.name.clone(),
          Some(ImageDigest::vec(&image.digests)),
        ))
        .unwrap_or((
          if image.contains(':') {
            image.to_string()
          } else {
            format!("{image}:latest")
          },
          None
        ));

      let state = container
        .as_ref()
        .map(|c| c.state.into())
        .unwrap_or(StackServiceState::Unknown);

      StackService {
        stack_id: stack.id.clone(),
        stack_name: stack.name.clone(),
        service: service_name.clone(),
        image: image.clone(),
        container,
        state,
        image_digests,
      }
    }).collect::<Vec<_>>();

    let current_state = get_stack_state_from_containers(
      &stack.config.ignore_services,
      &services,
      containers,
    );

    services_with_containers
      .sort_by(|a, b| a.service.cmp(&b.service));

    let prev_state = stack_status_cache
      .get(&stack.id)
      .await
      .map(|s| s.curr.state);

    stack_status_cache
      .insert(
        stack.id.clone(),
        History {
          curr: CachedStackStatus {
            id: stack.id,
            state: current_state,
            services: services_with_containers,
          },
          prev: prev_state,
        }
        .into(),
      )
      .await;
  }
}

pub async fn update_server_deployment_cache(
  deployments: Vec<Deployment>,
  containers: &[ContainerListItem],
  images: &[ImageListItem],
) {
  let deployment_status_cache = deployment_status_cache();

  for deployment in deployments {
    let container = containers
      .iter()
      .find(|container| container.name == deployment.name)
      .cloned();

    let image_digests = container
      .as_ref()
      .and_then(|container| container.image_id.as_ref())
      .and_then(|image_id| {
        images.iter().find_map(|image| {
          if &image.id == image_id {
            Some(ImageDigest::vec(&image.digests))
          } else {
            None
          }
        })
      });

    let prev_state = deployment_status_cache
      .get(&deployment.id)
      .await
      .map(|s| s.curr.state);

    let current_state = container
      .as_ref()
      .map(|c| c.state.into())
      .unwrap_or(DeploymentState::NotDeployed);

    deployment_status_cache
      .insert(
        deployment.id.clone(),
        History {
          curr: CachedDeploymentStatus {
            id: deployment.id,
            state: current_state,
            container,
            image_digests,
          },
          prev: prev_state,
        }
        .into(),
      )
      .await;
  }
}
