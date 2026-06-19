use std::time::Duration;

use iroh_bridge::{
    messages::{DeploymentSpec, DesiredDeployment, DesiredDispatch, ObservedDeployment, ObservedDispatch},
    network::Network,
    state::AppState,
};
use tokio::time::timeout;

#[tokio::test]
async fn desired_state_and_observed_status_cross_the_iroh_transport() {
    let master_state = AppState::new(String::new());
    let agent_state = AppState::new(String::new());

    let master = Network::bind(master_state.clone()).await.unwrap();
    let agent = Network::bind(agent_state.clone()).await.unwrap();

    timeout(Duration::from_secs(30), master.refresh_identity())
        .await
        .expect("master refresh should not hang")
        .unwrap();
    timeout(Duration::from_secs(30), agent.refresh_identity())
        .await
        .expect("agent refresh should not hang")
        .unwrap();

    master_state.push_desired_outbound(DesiredDispatch {
        endpoint_addr_json: agent_state.identity().await,
        deployment: DesiredDeployment {
            node_id: "node-a".into(),
            version: 1,
            spec: DeploymentSpec {
                name: "web".into(),
                compose_yaml: "services:\n  web:\n    image: nginx:latest\n".into(),
            },
            deleted: false,
        },
    }).await;

    timeout(Duration::from_secs(30), master.flush_outbound_once())
        .await
        .expect("master flush should not hang")
        .unwrap();

    let desired = agent_state.take_desired_inbound().await;
    assert_eq!(desired.len(), 1);
    assert_eq!(desired[0].spec.name, "web");

    agent_state.push_observed_outbound(ObservedDispatch {
        endpoint_addr_json: master_state.identity().await,
        deployment: ObservedDeployment {
            node_id: "node-a".into(),
            name: "web".into(),
            applied_version: 1,
            state: "succeeded".into(),
            message: None,
        },
    }).await;

    timeout(Duration::from_secs(30), agent.flush_outbound_once())
        .await
        .expect("agent flush should not hang")
        .unwrap();

    let observed = master_state.take_observed_inbound().await;
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].name, "web");
}
