use axum::{body::Body, http::{Request, StatusCode}};
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
