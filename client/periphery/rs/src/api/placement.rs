use komodo_client::entities::deployment::AssignedPort;
use mogh_resolver::Resolve;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(CheckHostPortsResponse)]
#[error(anyhow::Error)]
pub struct CheckHostPorts {
  pub ports: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckHostPortsResponse {
  pub free: Vec<u16>,
}

//

#[derive(Debug, Clone, Serialize, Deserialize, Resolve)]
#[response(ReadContainerPortsResponse)]
#[error(anyhow::Error)]
pub struct ReadContainerPorts {
  pub container_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReadContainerPortsResponse {
  pub ports: Vec<AssignedPort>,
}
