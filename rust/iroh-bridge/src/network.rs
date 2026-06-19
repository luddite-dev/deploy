use anyhow::{anyhow, Result};
use iroh::{endpoint::presets, Endpoint, EndpointAddr};

use crate::{
    messages::Envelope,
    state::AppState,
};

const ALPN: &[u8] = b"luddite/control/1";
const ACK: &[u8] = b"ok";

#[derive(Clone)]
pub struct Network {
    endpoint: Endpoint,
    state: AppState,
}

impl Network {
    pub async fn bind(state: AppState) -> Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;

        let network = Self {
            endpoint: endpoint.clone(),
            state: state.clone(),
        };

        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let state = state.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(connection) => {
                            let _ = handle_connection(state, connection).await;
                        }
                        Err(err) => {
                            tracing::warn!(?err, "accept connection failed");
                        }
                    }
                });
            }
        });

        Ok(network)
    }

    pub async fn refresh_identity(&self) -> Result<()> {
        self.endpoint.online().await;
        let addr = self.endpoint.addr();
        let json = serde_json::to_string(&addr)?;
        self.state.set_identity(json).await;
        Ok(())
    }

    pub async fn flush_outbound_once(&self) -> Result<()> {
        if let Some(dispatch) = self.state.take_next_desired_outbound().await {
            self.send(&dispatch.endpoint_addr_json, Envelope::Desired { deployment: dispatch.deployment })
                .await?;
        }
        if let Some(dispatch) = self.state.take_next_observed_outbound().await {
            self.send(&dispatch.endpoint_addr_json, Envelope::Observed { deployment: dispatch.deployment })
                .await?;
        }
        Ok(())
    }

    async fn send(&self, endpoint_addr_json: &str, envelope: Envelope) -> Result<()> {
        let addr: EndpointAddr = serde_json::from_str(endpoint_addr_json)?;
        let conn = self.endpoint.connect(addr, ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(&serde_json::to_vec(&envelope)?).await?;
        send.finish()?;
        let ack = recv.read_to_end(32).await?;
        if ack != ACK {
            return Err(anyhow!("unexpected ack: {:?}", ack));
        }
        conn.close(0u32.into(), b"done");
        Ok(())
    }
}

async fn handle_connection(state: AppState, connection: iroh::endpoint::Connection) -> Result<()> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let payload = recv.read_to_end(1 << 20).await?;
    let envelope: Envelope = serde_json::from_slice(&payload)?;

    match envelope {
        Envelope::Desired { deployment } => state.push_desired_inbound(deployment).await,
        Envelope::Observed { deployment } => state.push_observed_inbound(deployment).await,
    }

    send.write_all(ACK).await?;
    send.finish()?;
    connection.closed().await;
    Ok(())
}
