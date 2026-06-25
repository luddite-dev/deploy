use anyhow::Context;
use komodo_client::entities::{
  permission::PermissionLevelAndSpecifics, server::Server,
  stack::Stack, user::User,
};
use regex::Regex;

use crate::{
  helpers::query::get_server_for_command,
  permission::get_check_permissions,
};

pub mod execute;
pub mod remote;
pub mod services;

pub async fn setup_stack_execution(
  stack: &str,
  user: &User,
  permissions: PermissionLevelAndSpecifics,
) -> anyhow::Result<(Stack, Server)> {
  let stack =
    get_check_permissions::<Stack>(stack, user, permissions).await?;

  let server =
    get_server_for_command(&stack.config.server_id).await?;

  Ok((stack, server))
}

pub fn compose_container_match_regex(
  container_name: &str,
) -> anyhow::Result<Regex> {
  let regex = format!("^{container_name}-?[0-9]*$");
  Regex::new(&regex).with_context(|| {
    format!("failed to construct valid regex from {regex}")
  })
}
