//! Authentication, delegation, OIDC, TLS, rate limits and quotas.

mod common;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ag_server::config::{GroupRole, OidcProvider};
use common::*;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};

#[tokio::test]
async fn minted_credentials_can_only_narrow_and_always_expire() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    admin.post("/v1/repos", json!({"name": "acme/api"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/*", "writer").await;

    // Alice narrows her own access to one repository and two operations.
    let r = alice
        .post(
            "/v1/credentials",
            json!({"ttl_secs": 600, "scope": {"repos": ["acme/web"], "operations": ["repo.read", "objects.write"]}, "description": "ci upload"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let narrow = srv.client(r.json["token"].as_str().unwrap());
    assert_eq!(narrow.get("/v1/repos/acme/web").await.status, 200);
    assert_eq!(narrow.get("/v1/repos/acme/api").await.status, 403, "outside the credential's repository scope");
    assert_eq!(
        narrow.get("/v1/repos/acme/web/streams").await.status,
        403,
        "stream.read is outside the operation scope"
    );
    assert_eq!(alice.get("/v1/repos/acme/web/streams").await.status, 200, "her own token is unaffected");

    // A narrowed credential cannot mint a wider one, nor one for somebody else.
    let wider =
        narrow.post("/v1/credentials", json!({"ttl_secs": 60, "scope": {"repos": ["acme/web", "acme/api"]}})).await;
    assert_eq!(wider.status, 403, "{}", wider.json);
    let unrestricted = narrow.post("/v1/credentials", json!({"ttl_secs": 60})).await;
    assert_eq!(unrestricted.status, 403, "an unrestricted credential cannot be minted from a scoped one");
    let other = alice.post("/v1/credentials", json!({"subject": "user:bob", "ttl_secs": 60})).await;
    assert_eq!(other.status, 403, "only administrators mint credentials for others");

    // Lifetimes are capped, agents get short ones.
    let too_long = admin.post("/v1/credentials", json!({"subject": "agent:bot", "ttl_secs": 999_999})).await;
    assert_eq!(too_long.status, 400, "{}", too_long.json);
    let child = alice.post("/v1/credentials", json!({"ttl_secs": 30})).await;
    assert!(child.json["credential"]["expires_at_ms"].as_i64().unwrap() > 0);

    // Revocation is immediate; the owner may revoke their own credential.
    let id = r.json["credential"]["id"].as_str().unwrap().to_string();
    let rev = alice.call("DELETE", &format!("/v1/credentials/{id}"), None, &[]).await;
    assert_eq!(rev.status, 200, "{}", rev.json);
    assert_eq!(narrow.get("/v1/repos/acme/web").await.status, 401);

    // Secrets are never listed.
    let list = alice.get("/v1/credentials").await;
    assert!(!list.json.to_string().contains("agt_"), "tokens must never be listed");
    srv.stop().await;
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

struct Oidc {
    issuer: String,
    key: EncodingKey,
    other_key: EncodingKey,
    _handle: tokio::task::JoinHandle<()>,
}

async fn start_oidc() -> Oidc {
    let pem = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/oidc-test-ed25519.pem")).unwrap();
    let other = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/oidc-other-ed25519.pem")).unwrap();
    let key = EncodingKey::from_ed_pem(&pem).unwrap();
    let mut jwk = Jwk::from_encoding_key(&key, Algorithm::EdDSA).unwrap();
    jwk.common.key_id = Some("k1".into());
    let set = Arc::new(serde_json::to_value(JwkSet { keys: vec![jwk] }).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let iss = issuer.clone();
    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let iss = iss.clone();
                async move { axum::Json(json!({"issuer": iss, "jwks_uri": format!("{iss}/jwks")})) }
            }),
        )
        .route(
            "/jwks",
            axum::routing::get(move || {
                let set = set.clone();
                async move { axum::Json((*set).clone()) }
            }),
        );
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Oidc { issuer, key, other_key: EncodingKey::from_ed_pem(&other).unwrap(), _handle: handle }
}

fn token(_oidc: &Oidc, key: &EncodingKey, claims: Value) -> String {
    let mut h = Header::new(Algorithm::EdDSA);
    h.kid = Some("k1".into());
    encode(&h, &claims, key).unwrap()
}

#[tokio::test]
async fn oidc_tokens_are_validated_strictly_and_groups_map_to_roles() {
    let oidc = start_oidc().await;
    let issuer = oidc.issuer.clone();
    let srv = TestServer::start_with(move |cfg| {
        cfg.auth.oidc.push(OidcProvider {
            issuer,
            audience: "agentgit".into(),
            jwks_uri: None,
            jwks_file: None,
            username_claim: "preferred_username".into(),
            groups_claim: "groups".into(),
            algorithms: vec!["EdDSA".into()],
            group_roles: vec![GroupRole { group: "platform".into(), role: "admin".into(), repo: "*".into() }],
        });
    })
    .await;
    let good = json!({"iss": oidc.issuer, "aud": "agentgit", "exp": now_secs() + 600, "sub": "u-1", "preferred_username": "dana", "groups": ["platform"]});

    let caps = srv.client("").get("/v1/capabilities").await;
    assert!(caps.json["auth_methods"].as_array().unwrap().iter().any(|m| m == "oidc"));

    // A valid token authenticates; the platform group makes dana an administrator.
    let dana = srv.client(&token(&oidc, &oidc.key, good.clone()));
    let who = dana.get("/v1/whoami").await;
    assert_eq!(who.status, 200, "{}", who.json);
    assert_eq!(who.json["identity"], "user:dana");
    assert_eq!(who.json["method"], "oidc");
    let r = dana.post("/v1/repos", json!({"name": "acme/sso"})).await;
    assert_eq!(r.status, 200, "{}", r.json);

    // The same person without the group is an ordinary, unprivileged user.
    let mut no_group = good.clone();
    no_group["groups"] = json!([]);
    no_group["preferred_username"] = json!("erin");
    let erin = srv.client(&token(&oidc, &oidc.key, no_group));
    assert_eq!(erin.get("/v1/whoami").await.status, 200);
    assert_eq!(erin.get("/v1/repos/acme/sso").await.status, 404);

    // Every way of being wrong is rejected.
    let mut expired = good.clone();
    expired["exp"] = json!(now_secs() - 3600);
    let mut wrong_aud = good.clone();
    wrong_aud["aud"] = json!("someone-else");
    let mut wrong_iss = good.clone();
    wrong_iss["iss"] = json!("http://evil.example");
    let mut no_exp = good.clone();
    no_exp.as_object_mut().unwrap().remove("exp");
    for (name, tok) in [
        ("expired", token(&oidc, &oidc.key, expired)),
        ("wrong audience", token(&oidc, &oidc.key, wrong_aud)),
        ("foreign issuer", token(&oidc, &oidc.key, wrong_iss)),
        ("missing exp", token(&oidc, &oidc.key, no_exp)),
        ("signed with another key", token(&oidc, &oidc.other_key, good.clone())),
    ] {
        let r = srv.client(&tok).get("/v1/whoami").await;
        assert_eq!(r.status, 401, "{name} must be rejected: {}", r.json);
    }
    // An `alg: none` token is not accepted.
    let none = {
        use base64::Engine as _;
        let b = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.", b.encode(r#"{"alg":"none","typ":"JWT"}"#), b.encode(good.to_string()))
    };
    assert_eq!(srv.client(&none).get("/v1/whoami").await.status, 401);
    srv.stop().await;
}

#[tokio::test]
async fn requests_are_rate_limited_per_caller_with_a_retry_hint() {
    let srv = TestServer::start_with(|cfg| {
        cfg.limits.requests_per_second = 5;
        cfg.limits.burst = 5;
    })
    .await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/web"})).await;
    let alice = user_with_role(&srv, "user:alice", "acme/web", "reader").await;
    let mut limited = None;
    for _ in 0..40 {
        let r = alice.get("/v1/repos/acme/web").await;
        if r.status == 429 {
            limited = Some(r);
            break;
        }
    }
    let r = limited.expect("a burst of requests must hit the limiter");
    assert_eq!(r.json["error"]["code"], "rate_limited");
    assert!(r.json["error"]["retry_after_ms"].as_i64().unwrap() > 0);
    let metrics = srv.client("").get("/metrics").await;
    assert!(String::from_utf8_lossy(&metrics.body).contains("agwp_rate_limited_total"));
    srv.stop().await;
}

#[tokio::test]
async fn repository_quotas_are_enforced() {
    let srv = TestServer::start().await;
    let admin = srv.admin();
    admin.post("/v1/repos", json!({"name": "acme/small"})).await;
    let r = admin
        .call(
            "PATCH",
            "/v1/repos/acme/small",
            Some(json!({"quota": {"max_total_object_bytes": 2000, "max_streams": 2, "max_open_proposals": 1}})),
            &[],
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.json);
    let alice = user_with_role(&srv, "user:alice", "acme/small", "writer").await;
    let c = Client::new("acme/small");

    // The first small change fits ...
    let (h1, o1) = c.commit(vec![], "user:alice", "one", &[("a.txt", b"small")], 1_700_000_000_000);
    upload(&alice, "acme/small", &o1).await;
    // ... a large blob does not.
    let big = vec![b'x'; 5000];
    let (_h2, o2) = c.commit(vec![h1], "user:alice", "big", &[("big.bin", &big)], 1_700_000_001_000);
    let blob = &o2[0];
    let r = alice.put_bytes(&format!("/v1/repos/acme/small/objects/{}", blob.id()), blob.encode_envelope()).await;
    assert_eq!(r.status, 429, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "quota_exceeded");

    // Stream and proposal quotas.
    assert_eq!(alice.post("/v1/repos/acme/small/streams", json!({"name": "dev"})).await.status, 200);
    let r = alice.post("/v1/repos/acme/small/streams", json!({"name": "dev2"})).await;
    assert_eq!(r.json["error"]["code"], "quota_exceeded", "{}", r.json);
    let p1 = alice
        .post("/v1/repos/acme/small/proposals", json!({"stream": "main", "head": h1.to_string(), "title": "one"}))
        .await;
    assert_eq!(p1.status, 200, "{}", p1.json);
    let p2 = alice
        .post("/v1/repos/acme/small/proposals", json!({"stream": "dev", "head": h1.to_string(), "title": "two"}))
        .await;
    assert_eq!(p2.json["error"]["code"], "quota_exceeded", "{}", p2.json);
    srv.stop().await;
}

#[tokio::test]
async fn tls_serves_https_and_grpc_with_the_same_certificate() {
    use ag_protocol::v1::protocol_service_client::ProtocolServiceClient;
    use tonic::transport::{Certificate, Channel, ClientTlsConfig};

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.signing_key.serialize_pem()).unwrap();
    let (c2, k2) = (cert_path.clone(), key_path.clone());
    ag_server::server::install_crypto_provider();
    let srv = TestServer::start_with(move |cfg| {
        cfg.server.tls_cert = Some(c2);
        cfg.server.tls_key = Some(k2);
    })
    .await;
    let addr = srv.base.trim_start_matches("http://").to_string();

    let ca = Certificate::from_pem(cert.cert.pem());
    let channel = Channel::from_shared(format!("https://localhost:{}", addr.rsplit(':').next().unwrap()))
        .unwrap()
        .tls_config(ClientTlsConfig::new().ca_certificate(ca).domain_name("localhost"))
        .unwrap()
        .connect()
        .await
        .expect("gRPC over TLS");
    let hello = ProtocolServiceClient::new(channel)
        .negotiate(ag_protocol::v1::NegotiateRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(hello.protocol_version, "agwp.v1");

    // Plaintext requests to the TLS port are not served.
    let r = tokio::task::spawn_blocking(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        agent.get(format!("http://{addr}/healthz")).call().map(|r| r.status().as_u16())
    })
    .await
    .unwrap();
    assert!(r.is_err() || r.unwrap() != 200, "plaintext must not work on the TLS port");
    srv.stop().await;
}
