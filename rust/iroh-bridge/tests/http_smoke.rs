use axum::{body::{Body, to_bytes}, http::{Request, StatusCode}};
use tower::ServiceExt;

use iroh_bridge::{http::router, state::AppState};

#[tokio::test]
async fn publish_route_queues_outbound_message_and_identity_is_visible() {
    let state = AppState::new("{\"node_id\":\"local-sidecar\"}".to_string());
    let app = router(state.clone());

    let identity = Request::get("/v1/identity").body(Body::empty()).unwrap();
    let identity_res = app.clone().oneshot(identity).await.unwrap();
    assert_eq!(identity_res.status(), StatusCode::OK);

    let publish = Request::post("/v1/master/publish")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"endpoint_addr_json":"{\"node_id\":\"agent-sidecar\"}","deployment":{"node_id":"node-a","version":1,"spec":{"name":"web","compose_yaml":"services: {}\n"},"deleted":false}}"#))
        .unwrap();

    let publish_res = app.oneshot(publish).await.unwrap();
    assert_eq!(publish_res.status(), StatusCode::ACCEPTED);

    let queued = state.take_next_desired_outbound().await.expect("queued desired outbound");
    assert_eq!(queued.deployment.spec.name, "web");
    assert_eq!(queued.endpoint_addr_json, r#"{"node_id":"agent-sidecar"}"#);
}

#[tokio::test]
async fn take_observed_reports_returns_empty_array_when_no_inbound() {
    let state = AppState::new("{\"node_id\":\"local-sidecar\"}".to_string());
    let app = router(state);

    let res = app
        .oneshot(Request::get("/v1/master/reports").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"[]");
}

#[tokio::test]
async fn report_route_queues_observed_outbound() {
    let state = AppState::new("{\"node_id\":\"local-sidecar\"}".to_string());
    let app = router(state.clone());

    let report = Request::post("/v1/agent/report")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"endpoint_addr_json":"{\"node_id\":\"master-sidecar\"}","deployment":{"node_id":"node-a","name":"web","applied_version":3,"state":"succeeded"}}"#))
        .unwrap();

    let res = app.oneshot(report).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let queued = state.take_next_observed_outbound().await.expect("queued observed outbound");
    assert_eq!(queued.deployment.node_id, "node-a");
    assert_eq!(queued.deployment.name, "web");
    assert_eq!(queued.deployment.applied_version, 3);
    assert_eq!(queued.deployment.state, "succeeded");
    assert!(queued.deployment.message.is_none());
    assert_eq!(queued.endpoint_addr_json, r#"{"node_id":"master-sidecar"}"#);
}

#[tokio::test]
async fn take_desired_messages_returns_empty_array_when_no_inbound() {
    let state = AppState::new("{\"node_id\":\"local-sidecar\"}".to_string());
    let app = router(state);

    let res = app
        .oneshot(Request::get("/v1/agent/messages").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"[]");
}
