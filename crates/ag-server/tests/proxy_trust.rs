//! Trusted proxies over real sockets: the forwarding headers decide who is counted for failed logins only when
//! the connection comes from a configured proxy.

mod common;

use common::*;

async fn bad_login(srv: &TestServer, header: &str, value: &str) -> u16 {
    srv.client("not-a-credential").call("GET", "/v1/repos", None, &[(header, value)]).await.status
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trusted_proxy_names_the_client_whose_failed_logins_are_counted() {
    let srv = TestServer::start_with(|cfg| {
        cfg.server.trusted_proxies = vec!["127.0.0.0/8".into()]; // the test client connects from loopback
        cfg.limits.auth_failures_per_second = 1;
        cfg.limits.auth_failure_burst = 2;
    })
    .await;
    // `Forwarded` and `X-Forwarded-For` are both read.
    for (header, a, b) in
        [("x-forwarded-for", "198.51.100.1", "198.51.100.2"), ("forwarded", "for=198.51.100.3", "for=198.51.100.4")]
    {
        let first =
            [bad_login(&srv, header, a).await, bad_login(&srv, header, a).await, bad_login(&srv, header, a).await];
        assert_eq!(first, [401, 401, 429], "{header}");
        assert_eq!(bad_login(&srv, header, b).await, 401, "{header}: another client behind the proxy");
    }
    // A client that is throttled stays throttled with a good credential, and `Retry-After` is on the answer.
    let denied = srv.admin().call("GET", "/v1/repos", None, &[("x-forwarded-for", "198.51.100.1")]).await;
    assert_eq!(denied.status, 429, "{}", denied.json);
    assert!(denied.headers.contains_key("retry-after"), "{:?}", denied.headers);
    // The same credential from another address works.
    assert_eq!(srv.admin().call("GET", "/v1/repos", None, &[("x-forwarded-for", "198.51.100.9")]).await.status, 200);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_trusted_proxy_the_headers_buy_nothing() {
    let srv = TestServer::start_with(|cfg| {
        cfg.limits.auth_failures_per_second = 1;
        cfg.limits.auth_failure_burst = 2;
    })
    .await;
    let mut codes = Vec::new();
    for n in 0..4 {
        codes.push(bad_login(&srv, "x-forwarded-for", &format!("198.51.100.{n}")).await);
    }
    assert_eq!(codes, [401, 401, 429, 429], "every spoofed address is the one loopback peer");
    srv.stop().await;
}
