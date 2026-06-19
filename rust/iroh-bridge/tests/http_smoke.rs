use axum::{body::Body, http::{Request, StatusCode}};
use tower::ServiceExt;

use iroh_bridge::{
    http::router,
    messages::{DeploymentSpec, DesiredDeployment},
    state::AppState,
};

#[tokio::test]
async fn identity_is_visible_and_publish_queues_desired_outbound() {
    let state = AppState::new(r#"{"node_id":"local-sidecar"}"#.to_string());
    let app = router(state.clone());

    let identity = Request::get("/v1/identity")
        .body(Body::empty())
        .unwrap();
    let identity_res = app.clone().oneshot(identity).await.unwrap();
    assert_eq!(identity_res.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(identity_res.into_body(), 1024).await.unwrap();
    let body = std::str::from_utf8(&body_bytes).unwrap();
    assert!(body.contains("local-sidecar"));

    let payload = r#"{"endpoint_addr_json":"{\"node_id\":\"agent-sidecar\"}","deployment":{"node_id":"node-a","version":1,"spec":{"name":"web","compose_yaml":"services: {}\n"},"deleted":false}}"#;
    let publish = Request::post("/v1/master/publish")
        .header("content-type", "application/json")
        .body(Body::from(payload))
        .unwrap();
    let publish_res = app.oneshot(publish).await.unwrap();
    assert_eq!(publish_res.status(), StatusCode::ACCEPTED);

    let queued = state
        .take_next_desired_outbound()
        .await
        .expect("queued desired outbound");
    assert_eq!(queued.deployment.spec.name, "web");
    assert_eq!(queued.endpoint_addr_json, r#"{"node_id":"agent-sidecar"}"#);
}

#[tokio::test]
async fn report_polls_desired_inbound_and_report_queues_observed_outbound() {
    let state = AppState::new(String::new());
    let app = router(state.clone());

    state
        .push_desired_inbound(DesiredDeployment {
            node_id: "node-a".into(),
            version: 1,
            spec: DeploymentSpec {
                name: "web".into(),
                compose_yaml: "services: {}\n".into(),
            },
            deleted: false,
        })
        .await;

    let poll = Request::get("/v1/agent/messages")
        .body(Body::empty())
        .unwrap();
    let poll_res = app.clone().oneshot(poll).await.unwrap();
    assert_eq!(poll_res.status(), StatusCode::OK);

    let report_payload = r#"{"endpoint_addr_json":"{\"node_id\":\"master-sidecar\"}","deployment":{"node_id":"node-a","name":"web","applied_version":1,"state":"succeeded","message":null}}"#;
    let report = Request::post("/v1/agent/report")
        .header("content-type", "application/json")
        .body(Body::from(report_payload))
        .unwrap();
    let report_res = app.oneshot(report).await.unwrap();
    assert_eq!(report_res.status(), StatusCode::ACCEPTED);

    let outbound = state
        .take_next_observed_outbound()
        .await
        .expect("queued observed outbound");
    assert_eq!(outbound.deployment.state, "succeeded");
}
