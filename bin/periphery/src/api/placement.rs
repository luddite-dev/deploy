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
    let cmd = format!(
      "{} inspect --format json {}",
      container_cli(),
      self.container_name
    );
    let output =
      run_standard_command(&cmd, CommandOptions::default()).await;
    if !output.success() {
      anyhow::bail!("container inspect failed: {}", output.stderr);
    }
    let parsed: Vec<serde_json::Value> =
      serde_json::from_str(&output.stdout)?;
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
