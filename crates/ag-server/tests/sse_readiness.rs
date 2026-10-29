//! An idle SSE connection must deliver bytes before the first heartbeat.
mod common;
use common::*;
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::time::Duration;

#[tokio::test]
async fn an_authorized_idle_subscription_immediately_writes_a_comment() {
    let server = TestServer::start().await;
    assert_eq!(server.admin().post("/v1/repos", json!({"name":"acme/quiet"})).await.status, 200);
    let url = format!("{}/v1/repos/acme/quiet/events?heartbeat_ms=30000", server.base);
    let first = tokio::task::spawn_blocking(move || {
        let agent: ureq::Agent =
            ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(2))).build().into();
        let mut response = agent.get(url).header("authorization", format!("Bearer {ADMIN_TOKEN}")).call().unwrap();
        assert_eq!(response.status(), 200);
        let mut line = String::new();
        BufReader::new(response.body_mut().as_reader()).read_line(&mut line).map(|_| line)
    })
    .await
    .unwrap();
    assert!(first.is_ok(), "idle feed waited for a heartbeat: {first:?}");
    assert!(first.unwrap().starts_with(':'), "readiness must be an SSE comment, not a sequenced event");
    server.stop().await;
}
