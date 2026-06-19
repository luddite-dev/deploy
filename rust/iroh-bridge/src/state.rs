use std::{collections::VecDeque, sync::Arc};

use tokio::sync::Mutex;

use crate::messages::{DesiredDeployment, DesiredDispatch, ObservedDeployment, ObservedDispatch};

#[derive(Clone)]
pub struct AppState {
    identity: Arc<Mutex<String>>,
    desired_outbound: Arc<Mutex<VecDeque<DesiredDispatch>>>,
    desired_inbound: Arc<Mutex<VecDeque<DesiredDeployment>>>,
    observed_outbound: Arc<Mutex<VecDeque<ObservedDispatch>>>,
    observed_inbound: Arc<Mutex<VecDeque<ObservedDeployment>>>,
}

impl AppState {
    pub fn new(identity: String) -> Self {
        Self {
            identity: Arc::new(Mutex::new(identity)),
            desired_outbound: Arc::new(Mutex::new(VecDeque::new())),
            desired_inbound: Arc::new(Mutex::new(VecDeque::new())),
            observed_outbound: Arc::new(Mutex::new(VecDeque::new())),
            observed_inbound: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub async fn identity(&self) -> String {
        self.identity.lock().await.clone()
    }

    pub async fn set_identity(&self, identity: String) {
        *self.identity.lock().await = identity;
    }

    pub async fn push_desired_outbound(&self, dispatch: DesiredDispatch) {
        self.desired_outbound.lock().await.push_back(dispatch);
    }

    pub async fn take_next_desired_outbound(&self) -> Option<DesiredDispatch> {
        self.desired_outbound.lock().await.pop_front()
    }

    pub async fn push_desired_inbound(&self, deployment: DesiredDeployment) {
        self.desired_inbound.lock().await.push_back(deployment);
    }

    pub async fn take_desired_inbound(&self) -> Vec<DesiredDeployment> {
        self.desired_inbound.lock().await.drain(..).collect()
    }

    pub async fn push_observed_outbound(&self, dispatch: ObservedDispatch) {
        self.observed_outbound.lock().await.push_back(dispatch);
    }

    pub async fn take_next_observed_outbound(&self) -> Option<ObservedDispatch> {
        self.observed_outbound.lock().await.pop_front()
    }

    pub async fn push_observed_inbound(&self, deployment: ObservedDeployment) {
        self.observed_inbound.lock().await.push_back(deployment);
    }

    pub async fn take_observed_inbound(&self) -> Vec<ObservedDeployment> {
        self.observed_inbound.lock().await.drain(..).collect()
    }
}
