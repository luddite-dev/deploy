use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentSpec {
    pub name: String,
    pub compose_yaml: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesiredDeployment {
    pub node_id: String,
    pub version: u64,
    pub spec: DeploymentSpec,
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservedDeployment {
    pub node_id: String,
    pub name: String,
    pub applied_version: u64,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesiredDispatch {
    pub endpoint_addr_json: String,
    pub deployment: DesiredDeployment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservedDispatch {
    pub endpoint_addr_json: String,
    pub deployment: ObservedDeployment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityResponse {
    pub endpoint_addr_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    Desired { deployment: DesiredDeployment },
    Observed { deployment: ObservedDeployment },
}
