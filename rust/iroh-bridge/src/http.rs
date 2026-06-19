use axum::{extract::State, http::StatusCode, routing::{get, post}, Json, Router};

use crate::{
    messages::{DesiredDeployment, DesiredDispatch, IdentityResponse, ObservedDeployment, ObservedDispatch},
    state::AppState,
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/identity", get(identity))
        .route("/v1/master/publish", post(queue_desired))
        .route("/v1/master/reports", get(take_observed))
        .route("/v1/agent/messages", get(take_desired))
        .route("/v1/agent/report", post(queue_observed))
        .with_state(state)
}

async fn identity(State(state): State<AppState>) -> Json<IdentityResponse> {
    Json(IdentityResponse {
        endpoint_addr_json: state.identity().await,
    })
}

async fn queue_desired(
    State(state): State<AppState>,
    Json(dispatch): Json<DesiredDispatch>,
) -> (StatusCode, Json<DesiredDispatch>) {
    state.push_desired_outbound(dispatch.clone()).await;
    (StatusCode::ACCEPTED, Json(dispatch))
}

async fn take_desired(State(state): State<AppState>) -> Json<Vec<DesiredDeployment>> {
    Json(state.take_desired_inbound().await)
}

async fn queue_observed(
    State(state): State<AppState>,
    Json(dispatch): Json<ObservedDispatch>,
) -> (StatusCode, Json<ObservedDispatch>) {
    state.push_observed_outbound(dispatch.clone()).await;
    (StatusCode::ACCEPTED, Json(dispatch))
}

async fn take_observed(State(state): State<AppState>) -> Json<Vec<ObservedDeployment>> {
    Json(state.take_observed_inbound().await)
}
