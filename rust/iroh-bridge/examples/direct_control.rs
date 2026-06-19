//! Toy example: control-plane-style message exchange over Iroh.
//!
//! A "master" sidecar sends a JSON DesiredDeployment envelope to an "agent"
//! sidecar; the agent replies with a JSON ObservedDeployment envelope. This
//! mimics the planned rust/iroh-bridge/src/network.rs flow.

use anyhow::Result;
use iroh::{endpoint::presets, Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};

const ALPN: &[u8] = b"luddite/control/1";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DeploymentSpec {
    name: String,
    compose_yaml: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DesiredDeployment {
    node_id: String,
    version: u64,
    spec: DeploymentSpec,
    deleted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ObservedDeployment {
    node_id: String,
    name: String,
    applied_version: u64,
    state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Envelope {
    Desired { deployment: DesiredDeployment },
    Observed { deployment: ObservedDeployment },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let agent = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind_addr("127.0.0.1:0")?
        .bind()
        .await?;
    let agent_sock = agent.bound_sockets().into_iter().next().unwrap();
    println!("agent bound to {agent_sock}");

    let agent_addr = EndpointAddr::new(agent.id()).with_ip_addr(agent_sock);

    // Round-trip the address through JSON so this also exercises the
    // EndpointAddr serialization that the Go side will see.
    let agent_addr_json = serde_json::to_string(&agent_addr)?;
    let agent_addr_back: EndpointAddr = serde_json::from_str(&agent_addr_json)?;
    println!("agent addr round-trips through JSON OK");

    let agent_task = tokio::spawn(run_agent(agent));

    let master = Endpoint::bind(presets::N0).await?;
    let conn = master.connect(agent_addr_back, ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;

    let desired = Envelope::Desired {
        deployment: DesiredDeployment {
            node_id: "node-a".into(),
            version: 1,
            spec: DeploymentSpec {
                name: "web".into(),
                compose_yaml: "services:\n  web:\n    image: nginx:latest\n".into(),
            },
            deleted: false,
        },
    };
    let payload = serde_json::to_vec(&desired)?;
    send.write_all(&payload).await?;
    send.finish()?;

    let response_bytes = recv.read_to_end(1 << 20).await?;
    let response: Envelope = serde_json::from_slice(&response_bytes)?;
    println!("master received: {response:?}");

    if let Envelope::Observed { deployment } = response {
        assert_eq!(deployment.name, "web");
        assert_eq!(deployment.applied_version, 1);
        assert_eq!(deployment.state, "succeeded");
    } else {
        panic!("expected observed envelope");
    }

    conn.close(0u32.into(), b"done");
    master.close().await;
    agent_task.await??;
    println!("direct control OK");
    Ok(())
}

async fn run_agent(endpoint: Endpoint) -> Result<()> {
    let incoming = endpoint.accept().await.expect("incoming connection");
    let conn = incoming.await?;
    let (mut send, mut recv) = conn.accept_bi().await?;

    let payload = recv.read_to_end(1 << 20).await?;
    let envelope: Envelope = serde_json::from_slice(&payload)?;
    println!("agent received: {envelope:?}");

    let observed = match envelope {
        Envelope::Desired { deployment } => ObservedDeployment {
            node_id: deployment.node_id,
            name: deployment.spec.name,
            applied_version: deployment.version,
            state: "succeeded".into(),
        },
        _ => panic!("expected desired envelope"),
    };

    let response = Envelope::Observed {
        deployment: observed,
    };
    send.write_all(&serde_json::to_vec(&response)?).await?;
    send.finish()?;
    conn.closed().await;
    endpoint.close().await;
    Ok(())
}
