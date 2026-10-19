//! The server's own OpenID Connect validation, against a REAL identity provider: Keycloak.
//!
//! `security.rs` tests the validator against a small provider built into the test (a test double: it can sign
//! anything, including tokens a real provider would never issue). These tests use tokens a real Keycloak issued,
//! signed with the key it generates and publishes, and the server finds that key the way production does: through
//! discovery (`.well-known/openid-configuration`, then the JWKS it names). Nothing is configured by hand.
//!
//! They need a running Keycloak with the realm in `apps/web/e2e/keycloak/agentgit-realm.json`, so they are
//! `#[ignore]`d in a plain `cargo test` and run by the "real identity provider" job of `.github/workflows/web.yml`:
//!
//! ```text
//! docker run --rm -p 8180:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
//!   -v $PWD/apps/web/e2e/keycloak:/opt/keycloak/data/import quay.io/keycloak/keycloak:26.0.7 start-dev --import-realm
//! AG_TEST_KEYCLOAK_URL=http://localhost:8180 cargo test -p agentgit-server --test oidc_keycloak -- --ignored
//! ```

mod common;

use std::time::Duration;

use ag_server::config::{GroupRole, OidcProvider};
use common::*;
use serde_json::{Value, json};

fn keycloak() -> String {
    std::env::var("AG_TEST_KEYCLOAK_URL")
        .unwrap_or_else(|_| panic!("AG_TEST_KEYCLOAK_URL is not set (see the top of this file)"))
        .trim_end_matches('/')
        .to_string()
}

fn issuer() -> String {
    format!("{}/realms/agentgit", keycloak())
}

/// What Keycloak issues for `username` through the password grant of `client`: `(id_token, access_token)`.
async fn login(client: &str, username: &str) -> (String, String) {
    let url = format!("{}/protocol/openid-connect/token", issuer());
    let (client, username) = (client.to_string(), username.to_string());
    tokio::task::spawn_blocking(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let mut res = agent
            .post(&url)
            .send_form([
                ("grant_type", "password"),
                ("client_id", client.as_str()),
                ("username", username.as_str()),
                ("password", &format!("{username}-password")),
                ("scope", "openid"),
            ])
            .expect("Keycloak is reachable");
        let status = res.status().as_u16();
        let body: Value = res.body_mut().read_json().expect("a JSON answer");
        assert_eq!(status, 200, "Keycloak refused a login through client {client}: {body}");
        (
            body["id_token"].as_str().expect("an id_token (scope openid)").to_string(),
            body["access_token"].as_str().expect("an access_token").to_string(),
        )
    })
    .await
    .unwrap()
}

fn provider(audience: &str, leeway_secs: u64) -> OidcProvider {
    OidcProvider {
        issuer: issuer(),
        audience: audience.into(),
        // Discovery: the server asks Keycloak where its keys are.
        jwks_uri: None,
        jwks_file: None,
        username_claim: "preferred_username".into(),
        groups_claim: "groups".into(),
        algorithms: vec!["RS256".into()],
        group_roles: vec![
            GroupRole { group: "staff".into(), role: "reader".into(), repo: "acme/*".into() },
            GroupRole { group: "platform".into(), role: "admin".into(), repo: "acme/*".into() },
        ],
        http_timeout_secs: 10,
        jwks_refresh_secs: 30,
        jwks_max_age_secs: 3600,
        leeway_secs,
    }
}

/// Flips one character in the middle of the part of a JWT that is `which` (0 header, 1 payload, 2 signature).
fn tamper(jwt: &str, which: usize) -> String {
    let mut parts: Vec<String> = jwt.split('.').map(str::to_string).collect();
    let mut chars: Vec<char> = parts[which].chars().collect();
    let i = chars.len() / 2;
    chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
    parts[which] = chars.into_iter().collect();
    parts.join(".")
}

#[tokio::test]
#[ignore = "needs a Keycloak: AG_TEST_KEYCLOAK_URL (run by the real identity provider CI job)"]
async fn a_token_keycloak_issued_is_accepted_and_its_groups_become_roles() {
    let srv = TestServer::start_with(|cfg| cfg.auth.oidc.push(provider("agentgit-api", 30))).await;
    let caps = srv.client("").get("/v1/capabilities").await;
    assert!(caps.json["auth_methods"].as_array().unwrap().iter().any(|m| m == "oidc"));

    // dana is in the platform group: an administrator of acme/*.
    let (dana_id, _) = login("agentgit-api", "dana").await;
    let dana = srv.client(&dana_id);
    let who = dana.get("/v1/whoami").await;
    assert_eq!(who.status, 200, "{}", who.json);
    assert_eq!(who.json["identity"], "user:dana");
    assert_eq!(who.json["method"], "oidc");
    let made = dana.post("/v1/repos", json!({"name": "acme/sso"})).await;
    assert_eq!(made.status, 200, "{}", made.json);

    // alice is staff: a reader, who can see that repository but not administer anything.
    let (alice_id, _) = login("agentgit-api", "alice").await;
    let alice = srv.client(&alice_id);
    assert_eq!(alice.get("/v1/whoami").await.json["identity"], "user:alice");
    assert_eq!(alice.get("/v1/repos/acme/sso").await.status, 200);
    let denied = alice.post("/v1/repos", json!({"name": "acme/not-allowed"})).await;
    assert_eq!(denied.status, 403, "{}", denied.json);

    // nobody is in no group: a valid identity with no role at all, to whom the repository does not exist.
    let (nobody_id, _) = login("agentgit-api", "nobody").await;
    let nobody = srv.client(&nobody_id);
    assert_eq!(nobody.get("/v1/whoami").await.status, 200);
    assert_eq!(nobody.get("/v1/repos/acme/sso").await.status, 404);
    srv.stop().await;
}

#[tokio::test]
#[ignore = "needs a Keycloak: AG_TEST_KEYCLOAK_URL (run by the real identity provider CI job)"]
async fn tokens_that_are_wrong_in_any_way_are_refused() {
    let srv = TestServer::start_with(|cfg| cfg.auth.oidc.push(provider("agentgit-api", 30))).await;
    let (good, access) = login("agentgit-api", "dana").await;
    assert_eq!(srv.client(&good).get("/v1/whoami").await.status, 200);

    // A signature that is not Keycloak's, a payload changed after signing (promote yourself by editing the
    // groups), a header changed after signing.
    for (what, token) in [
        ("a damaged signature", tamper(&good, 2)),
        ("a payload edited after signing", tamper(&good, 1)),
        ("a header edited after signing", tamper(&good, 0)),
        ("not a token", "not.a.jwt".to_string()),
    ] {
        let r = srv.client(&token).get("/v1/whoami").await;
        assert_eq!(r.status, 401, "{what} must be refused: {}", r.json);
    }

    // The access token Keycloak issues in the same exchange names another audience ("account"): the server was
    // told to accept tokens for "agentgit-api" and nothing else.
    let r = srv.client(&access).get("/v1/whoami").await;
    assert_eq!(r.status, 401, "an access token for another audience must be refused: {}", r.json);

    // A token for another client of the same realm is refused too: same issuer, same keys, wrong audience.
    let (web, _) = login("agentgit-api-short", "dana").await;
    let r = srv.client(&web).get("/v1/whoami").await;
    assert_eq!(r.status, 401, "a token for client agentgit-api-short must be refused: {}", r.json);
    srv.stop().await;
}

#[tokio::test]
#[ignore = "needs a Keycloak: AG_TEST_KEYCLOAK_URL (run by the real identity provider CI job)"]
async fn a_token_stops_working_when_it_expires() {
    // No tolerance for clock skew, so that the token's own `exp` (three seconds after it was issued) decides.
    let srv = TestServer::start_with(|cfg| cfg.auth.oidc.push(provider("agentgit-api-short", 0))).await;
    let (token, _) = login("agentgit-api-short", "alice").await;
    let http = srv.client(&token);
    assert_eq!(http.get("/v1/whoami").await.status, 200, "fresh");

    tokio::time::sleep(Duration::from_secs(5)).await;
    let r = http.get("/v1/whoami").await;
    assert_eq!(r.status, 401, "the token expired: {}", r.json);
    assert_eq!(r.json["error"]["code"], "unauthenticated", "{}", r.json);

    // A new login gives a working token again.
    let (again, _) = login("agentgit-api-short", "alice").await;
    assert_eq!(srv.client(&again).get("/v1/whoami").await.status, 200);
    srv.stop().await;
}
