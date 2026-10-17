//! The Web UI as the server serves it: routing precedence, the single-page-app fallback, caching, the content
//! security policy, the switches (`ui.enabled`, `ui.dir`) and the sign-in discovery endpoint.

mod common;

use std::path::Path;

use ag_server::config::ServerConfig;
use common::TestServer;
use serde_json::json;

const INDEX: &str = "<!doctype html><html><body><div id=\"app\"></div><script type=\"module\" src=\"/assets/app-AbCd1234.js\"></script></body></html>";

/// A tiny stand-in for `apps/web/dist`, with pre-compressed variants of the script.
fn fake_dist(dir: &Path) {
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), INDEX).unwrap();
    std::fs::write(dir.join("favicon.svg"), "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    std::fs::write(dir.join("assets/app-AbCd1234.js"), "console.log('app')").unwrap();
    std::fs::write(dir.join("assets/app-AbCd1234.js.br"), b"BROTLI-BYTES").unwrap();
    std::fs::write(dir.join("assets/app-AbCd1234.js.gz"), b"GZIP-BYTES").unwrap();
}

async fn with_dir(f: impl FnOnce(&mut ServerConfig)) -> (TestServer, tempfile::TempDir) {
    let dist = tempfile::tempdir().unwrap();
    fake_dist(dist.path());
    let path = dist.path().to_path_buf();
    let srv = TestServer::start_with(|cfg| {
        cfg.ui.dir = Some(path);
        f(cfg);
    })
    .await;
    (srv, dist)
}

#[tokio::test]
async fn a_directory_override_serves_the_app_and_never_shadows_the_api() {
    let (srv, _dist) = with_dir(|_| {}).await;
    let anon = srv.client("");

    let page = anon.get("/").await;
    assert_eq!(page.status, 200);
    assert_eq!(page.headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(page.headers["cache-control"], "no-cache");
    assert_eq!(String::from_utf8(page.body.clone()).unwrap(), INDEX);

    // A route of the app, as a bookmarked URL: the page again (and the same for a repository with dots in its name).
    for path in
        ["/repos/acme/webapp/proposals/prop_1", "/repos/acme/site.com", "/signin", "/auth/callback?code=x&state=y"]
    {
        let r = anon.get(path).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(String::from_utf8(r.body).unwrap(), INDEX, "{path}");
    }

    // Everything that is the server's own still is.
    let caps = anon.get("/v1/capabilities").await;
    assert_eq!(caps.status, 200);
    assert!(caps.json["protocol_version"].is_string(), "{:?}", caps.json);
    assert_eq!(String::from_utf8(anon.get("/healthz").await.body).unwrap(), "ok");
    assert_eq!(anon.get("/readyz").await.status, 200);
    let spec = anon.get("/openapi.json").await;
    assert_eq!(spec.status, 200);
    assert!(spec.json["paths"]["/v1/auth/config"].is_object());
    let metrics = anon.get("/metrics").await;
    assert_eq!(metrics.status, 200);
    assert!(metrics.headers["content-type"].starts_with("text/plain"));

    // Reserved prefixes that match nothing are API errors, not the page.
    for path in ["/v1/nothing", "/v1/repos/acme/webapp/nothing", "/agwp.v1.Nothing/Call", "/healthz/extra"] {
        let r = anon.get(path).await;
        assert_eq!(r.status, 404, "{path}");
        assert_eq!(r.json["error"]["code"], "not_found", "{path}: {:?}", r.json);
        assert!(!String::from_utf8_lossy(&r.body).contains("<html"), "{path}");
    }
    // A method a route does not have keeps its API answer.
    let wrong = anon.post("/v1/capabilities", json!({})).await;
    assert_eq!(wrong.status, 405);
    assert_eq!(wrong.json["error"]["code"], "invalid_argument");
    srv.stop().await;
}

#[tokio::test]
async fn a_disabled_metrics_endpoint_is_not_replaced_by_the_page() {
    let (srv, _dist) = with_dir(|cfg| cfg.metrics.enabled = false).await;
    let r = srv.client("").get("/metrics").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.json["error"]["code"], "not_found");
    srv.stop().await;
}

#[tokio::test]
async fn files_have_their_types_cache_rules_encodings_and_validators() {
    let (srv, _dist) = with_dir(|_| {}).await;
    let anon = srv.client("");

    let js = anon.call("GET", "/assets/app-AbCd1234.js", None, &[("accept-encoding", "br, gzip")]).await;
    assert_eq!(js.status, 200);
    assert_eq!(js.headers["content-type"], "text/javascript; charset=utf-8");
    assert_eq!(js.headers["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(js.headers["content-encoding"], "br");
    assert_eq!(js.headers["vary"], "Accept-Encoding");
    assert_eq!(js.body, b"BROTLI-BYTES");
    let gz = anon.call("GET", "/assets/app-AbCd1234.js", None, &[("accept-encoding", "gzip")]).await;
    assert_eq!(gz.headers["content-encoding"], "gzip");
    assert_eq!(gz.body, b"GZIP-BYTES");
    let plain = anon.call("GET", "/assets/app-AbCd1234.js", None, &[("accept-encoding", "identity")]).await;
    assert!(!plain.headers.contains_key("content-encoding"));
    assert_eq!(plain.body, b"console.log('app')");

    let icon = anon.get("/favicon.svg").await;
    assert_eq!(icon.headers["content-type"], "image/svg+xml");
    assert_eq!(icon.headers["cache-control"], "no-cache", "an unhashed file is revalidated, never immutable");

    // Validators: a second request with the tag is answered 304 with no body.
    let etag = icon.headers["etag"].clone();
    let again = anon.call("GET", "/favicon.svg", None, &[("if-none-match", &etag)]).await;
    assert_eq!(again.status, 304);
    assert!(again.body.is_empty());

    // HEAD has the headers and no body.
    let head = anon.call("HEAD", "/", None, &[]).await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert!(head.headers["content-length"].parse::<usize>().unwrap() > 0);

    // A missing asset is a 404 envelope (never the page: that would be cached as a script); so is a path that
    // tries to leave the directory.
    for path in ["/assets/gone-ZZZZ9999.js", "/robots.txt", "/assets/app-AbCd1234.js.br"] {
        let r = anon.get(path).await;
        assert_eq!(r.status, 404, "{path}");
        assert_eq!(r.json["error"]["code"], "not_found", "{path}");
    }
    // Names that could climb out of the directory are never looked up: they are routes of the app, i.e. the page.
    for path in ["/assets/..%2f..%2fetc%2fpasswd", "/%2e%2e/secret", "/..%5csecret"] {
        let r = anon.get(path).await;
        assert!(r.status == 404 || String::from_utf8_lossy(&r.body) == INDEX, "{path}: {}", r.status);
    }
    // Writes are not allowed.
    assert_eq!(anon.post("/favicon.svg", json!({})).await.status, 405);
    srv.stop().await;
}

#[tokio::test]
async fn every_ui_response_carries_a_strict_content_security_policy() {
    let (srv, _dist) = with_dir(|_| {}).await;
    let anon = srv.client("");
    for path in ["/", "/signin", "/assets/app-AbCd1234.js", "/favicon.svg"] {
        let r = anon.get(path).await;
        let csp = &r.headers["content-security-policy"];
        for must in [
            "default-src 'self'",
            "script-src 'self'",
            "connect-src 'self'",
            "object-src 'none'",
            "base-uri 'self'",
            "frame-ancestors 'none'",
        ] {
            assert!(csp.contains(must), "{path}: {csp}");
        }
        // Scripts: only the server's own files. Styles may be inline (WebKit needs it); nothing is ever eval'd.
        assert!(!csp.contains("unsafe-eval") && !csp.contains('*'), "{path}: {csp}");
        assert!(!csp.contains("script-src 'self' 'unsafe-inline'"), "{path}: {csp}");
        assert_eq!(r.headers["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(r.headers["referrer-policy"], "no-referrer", "{path}");
        assert_eq!(r.headers["x-frame-options"], "DENY", "{path}");
    }
    srv.stop().await;
}

#[tokio::test]
async fn the_policy_lets_the_browser_reach_exactly_the_configured_identity_providers() {
    let (srv, _dist) = with_dir(|cfg| {
        cfg.ui.login.push(ag_server::ui::UiLogin {
            token_endpoint: Some("https://tokens.example.net/oauth/token".into()),
            ..ag_server::ui::UiLogin::new("https://sso.example.com/realms/acme", "agentgit-web")
        });
    })
    .await;
    let r = srv.client("").get("/").await;
    assert!(
        r.headers["content-security-policy"]
            .contains("connect-src 'self' https://sso.example.com https://tokens.example.net;"),
        "{}",
        r.headers["content-security-policy"]
    );
    srv.stop().await;
}

#[tokio::test]
async fn the_deprecated_web_dir_spelling_still_works() {
    let dist = tempfile::tempdir().unwrap();
    fake_dist(dist.path());
    let path = dist.path().to_path_buf();
    let srv = TestServer::start_with(|cfg| cfg.server.web_dir = Some(path)).await;
    let r = srv.client("").get("/repos/a/b").await;
    assert_eq!(r.status, 200);
    assert_eq!(String::from_utf8(r.body).unwrap(), INDEX);
    srv.stop().await;
}

#[tokio::test]
async fn a_disabled_ui_leaves_only_the_api() {
    let (srv, _dist) = with_dir(|cfg| cfg.ui.enabled = false).await;
    let anon = srv.client("");
    for path in ["/", "/repos/a/b", "/favicon.svg"] {
        let r = anon.get(path).await;
        assert_eq!(r.status, 404, "{path}");
        assert_eq!(r.json["error"]["code"], "not_found", "{path}");
    }
    assert_eq!(anon.get("/v1/capabilities").await.status, 200);
    assert_eq!(anon.get("/v1/auth/config").await.status, 200);
    srv.stop().await;
}

#[tokio::test]
async fn without_an_override_the_binary_serves_what_it_contains() {
    let srv = TestServer::start().await;
    let anon = srv.client("");
    let page = anon.call("GET", "/", None, &[("accept", "text/html")]).await;
    let robot = anon.call("GET", "/", None, &[("accept", "application/json")]).await;
    if ag_server::ui::embedded() {
        // A binary built after scripts/build_ui.sh: the real app, from memory.
        assert_eq!(page.status, 200);
        let html = String::from_utf8(page.body).unwrap();
        assert!(html.contains("<div id=\"app\">"), "{html}");
        assert!(!html.contains("<script>"), "no inline script");
        let deep = anon.get("/repos/acme/webapp/proposals/x").await;
        assert_eq!(deep.status, 200);
        assert!(deep.headers["content-security-policy"].contains("script-src 'self'"));
        // Every script the page names is embedded and cacheable forever.
        let src = html.split("src=\"").nth(1).and_then(|s| s.split('"').next()).expect("a script tag");
        let js = anon.get(src).await;
        assert_eq!(js.status, 200, "{src}");
        assert_eq!(js.headers["cache-control"], "public, max-age=31536000, immutable");
        assert!(js.headers["content-type"].starts_with("text/javascript"));
    } else {
        // A binary built without the UI says so, to a browser, and keeps the plain answer for API clients.
        assert_eq!(page.status, 503);
        let text = String::from_utf8(page.body).unwrap();
        assert!(text.contains("scripts/build_ui.sh"), "{text}");
        assert_eq!(robot.status, 404);
        assert_eq!(robot.json["error"]["code"], "not_found");
    }
    srv.stop().await;
}

#[tokio::test]
async fn sign_in_discovery_is_public_and_says_token_when_no_provider_is_configured() {
    let srv = TestServer::start().await;
    let r = srv.client("").get("/v1/auth/config").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json, json!({"mode": "token"}));
    srv.stop().await;
}

#[tokio::test]
async fn sign_in_discovery_lists_the_configured_providers() {
    let srv = TestServer::start_with(|cfg| {
        cfg.ui.login.push(ag_server::ui::UiLogin {
            token_endpoint: Some("https://tokens.example.net/oauth/token".into()),
            ..ag_server::ui::UiLogin::new("https://sso.example.com/realms/acme", "agentgit-web")
        });
    })
    .await;
    let r = srv.client("").get("/v1/auth/config").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["cache-control"], "no-cache");
    assert_eq!(r.json["mode"], "oidc");
    let p = &r.json["providers"][0];
    assert_eq!(p["issuer"], "https://sso.example.com/realms/acme");
    assert_eq!(p["client_id"], "agentgit-web");
    assert_eq!(p["name"], "sso.example.com");
    assert_eq!(p["scopes"], json!(["openid", "profile"]));
    assert_eq!(p["token_endpoint"], "https://tokens.example.net/oauth/token");
    assert_eq!(p["token"], "id_token");
    assert!(p.get("authorization_endpoint").is_none(), "unset endpoints are left out: {p}");
    // Nothing secret is in it: the endpoint is public, so it carries no credential and needs none.
    assert!(!r.json.to_string().contains("secret"));
    srv.stop().await;
}
