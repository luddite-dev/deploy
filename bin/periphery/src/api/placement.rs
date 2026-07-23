use std::collections::HashSet;

use command::{CommandOptions, run_standard_command};
use komodo_client::entities::deployment::AssignedPort;
use mogh_resolver::Resolve;
use netstat2::{
  AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState,
  get_sockets_info,
};

use periphery_client::api::placement::{
  CheckHostPorts, CheckHostPortsResponse, ReadContainerPorts,
  ReadContainerPortsResponse,
};

use crate::{api::Args, docker::container_cli};

impl Resolve<Args> for CheckHostPorts {
  #[instrument("CheckHostPorts")]
  async fn resolve(
    self,
    _args: &Args,
  ) -> anyhow::Result<CheckHostPortsResponse> {
    let sockets = get_sockets_info(
      AddressFamilyFlags::all(),
      ProtocolFlags::TCP,
    )?;
    let bound: HashSet<u16> = sockets
      .into_iter()
      .filter_map(|s| match s.protocol_socket_info {
        ProtocolSocketInfo::Tcp(tcp)
          if tcp.local_port > 0 && tcp.state == TcpState::Listen =>
        {
          Some(tcp.local_port)
        }
        _ => None,
      })
      .collect();
    let free = self
      .ports
      .into_iter()
      .filter(|p| !bound.contains(p))
      .collect();
    Ok(CheckHostPortsResponse { free })
  }
}

impl Resolve<Args> for ReadContainerPorts {
  #[instrument("ReadContainerPorts")]
  async fn resolve(
    self,
    _args: &Args,
  ) -> anyhow::Result<ReadContainerPortsResponse> {
    // Try exact container name first (works when compose sets explicit
    // container_name or for multi-replica services where ComposeUp
    // returns the full name with replica suffix).
    let stdout = match inspect_container(&self.container_name).await {
      Some(out) => out,
      None => {
        // Fall back to pattern matching: list running containers and
        // find one whose name matches `^{container_name}-?[0-9]*$`.
        // This handles the common case where Docker Compose v2 creates
        // single-replica containers as `{project}-{service}-1` but
        // ComposeUp returns `{project}-{service}` as the container_name.
        let actual_name =
          resolve_container_name_by_pattern(&self.container_name)
            .await
            .ok_or_else(|| {
              anyhow::anyhow!(
                "no running container found matching '{}'",
                self.container_name
              )
            })?;
        inspect_container(&actual_name).await.ok_or_else(|| {
          anyhow::anyhow!(
            "container inspect failed for resolved name '{}'",
            actual_name
          )
        })?
      }
    };

    let parsed: Vec<serde_json::Value> =
      serde_json::from_str(&stdout)?;
    let mut ports = Vec::new();
    if let Some(container) = parsed.first() {
      if let Some(network) = container.get("NetworkSettings") {
        if let Some(ports_map) = network.get("Ports") {
          if let Some(obj) = ports_map.as_object() {
            for (key, bindings) in obj {
              let container_port: u16 = key
                .split('/')
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
              if let Some(arr) = bindings.as_array() {
                for binding in arr {
                  if let Some(host_port) =
                    binding.get("HostPort").and_then(|v| v.as_str())
                  {
                    ports.push(AssignedPort {
                      container: container_port,
                      host: host_port.parse().unwrap_or(0),
                    });
                  }
                }
              }
            }
          }
        }
      }
    }
    Ok(ReadContainerPortsResponse { ports })
  }
}

/// Run `<cli> inspect --format json <name>` and return stdout on success.
async fn inspect_container(name: &str) -> Option<String> {
  let cmd =
    format!("{} inspect --format json {}", container_cli(), name);
  let output =
    run_standard_command(&cmd, CommandOptions::default()).await;
  if output.success() && !output.stdout.is_empty() {
    Some(output.stdout)
  } else {
    None
  }
}

/// List running containers via `<cli> ps --format json` and find one
/// whose name matches `^{pattern}-?[0-9]*$` (mirrors core's
/// `compose_container_match_regex`).
async fn resolve_container_name_by_pattern(
  pattern: &str,
) -> Option<String> {
  let regex_str = format!("^{pattern}-?[0-9]*$");
  let regex = regex::Regex::new(&regex_str).ok()?;

  let cmd = format!("{} ps --format json", container_cli());
  let output =
    run_standard_command(&cmd, CommandOptions::default()).await;
  if !output.success() {
    return None;
  }

  // `docker/podman ps --format json` outputs one JSON object per line
  // (not a JSON array). Parse line-by-line.
  for line in output.stdout.lines() {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
      // `Names` is an array of strings (docker), or `Names` is a
      // single string (podman). Handle both.
      let matched = val
        .get("Names")
        .and_then(|names| {
          if let Some(arr) = names.as_array() {
            arr.iter().find_map(|n| {
              n.as_str().map(|s| {
                // Strip leading '/' (docker prefixes container names)
                s.trim_start_matches('/')
              })
            })
          } else {
            names.as_str().map(|s| s.trim_start_matches('/'))
          }
        })
        .is_some_and(|name| regex.is_match(name));

      if matched {
        // Return the first matching container name (stripped of '/')
        if let Some(name) = val.get("Names").and_then(|names| {
          if let Some(arr) = names.as_array() {
            arr.first().and_then(|n| n.as_str())
          } else {
            names.as_str()
          }
        }) {
          return Some(name.trim_start_matches('/').to_string());
        }
      }
    }
  }
  None
}
