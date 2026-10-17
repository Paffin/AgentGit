//! The same flows over gRPC (HTTP/2): streaming uploads, structured errors, live events.

mod common;

use std::time::Duration;

use ag_protocol::v1;
use ag_protocol::v1::{
    admin_service_client::AdminServiceClient, event_service_client::EventServiceClient,
    integration_service_client::IntegrationServiceClient, object_service_client::ObjectServiceClient,
    proposal_service_client::ProposalServiceClient, protocol_service_client::ProtocolServiceClient,
    repository_service_client::RepositoryServiceClient, stream_service_client::StreamServiceClient,
};
use ag_protocol::{ErrorCode, ProtocolError};
use common::*;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Request, Streaming};

async fn channel(srv: &TestServer) -> Channel {
    Channel::from_shared(srv.base.clone()).unwrap().connect().await.unwrap()
}

fn authed<T>(token: &str, msg: T) -> Request<T> {
    let mut r = Request::new(msg);
    r.metadata_mut().insert("authorization", MetadataValue::try_from(format!("Bearer {token}")).unwrap());
    r
}

#[tokio::test]
async fn grpc_negotiation_errors_and_a_full_change() {
    let srv = TestServer::start().await;
    let ch = channel(&srv).await;

    // Negotiation needs no credentials.
    let hello = ProtocolServiceClient::new(ch.clone())
        .negotiate(Request::new(v1::NegotiateRequest {
            supported_versions: vec!["agwp.v1".into()],
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(hello.protocol_version, "agwp.v1");
    // ... and refuses a client that speaks only a future version.
    let e = ProtocolServiceClient::new(ch.clone())
        .negotiate(Request::new(v1::NegotiateRequest {
            supported_versions: vec!["agwp.v9".into()],
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(ProtocolError::from_status(&e).code, ErrorCode::FailedPrecondition);

    // Everything else needs a credential; the error carries a structured code.
    let e =
        RepositoryServiceClient::new(ch.clone()).list(Request::new(v1::ListReposRequest::default())).await.unwrap_err();
    assert_eq!(e.code(), tonic::Code::Unauthenticated);
    assert_eq!(ProtocolError::from_status(&e).code, ErrorCode::Unauthenticated);

    let mut repos = RepositoryServiceClient::new(ch.clone());
    let repo = repos
        .create(authed(ADMIN_TOKEN, v1::CreateRepoRequest { name: "acme/api".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(repo.namespace, "acme/api");

    // A writer uploads a changeset as one client stream.
    let alice = user_with_role(&srv, "user:alice", "acme/api", "writer").await;
    let c = Client::new("acme/api");
    let (head, objects) = c.commit(vec![], "user:alice", "first", &[("a.txt", b"hello grpc\n")], 1_700_000_000_000);
    let mut msgs = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        let bytes = o.encode_envelope();
        msgs.push(v1::UploadObjectsRequest {
            repo: if i == 0 { "acme/api".into() } else { String::new() },
            chunk: Some(v1::ObjectChunk {
                object_id: o.id().to_string(),
                total_size: bytes.len() as u64,
                data: bytes,
                last: true,
            }),
        });
    }
    let summary = ObjectServiceClient::new(ch.clone())
        .upload(authed(&alice.token, futures_util::stream::iter(msgs)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(summary.stored.len(), 2);

    // A forged object is refused: the bytes must hash to the claimed id.
    let forged = ObjectServiceClient::new(ch.clone())
        .upload(authed(
            &alice.token,
            futures_util::stream::iter(vec![v1::UploadObjectsRequest {
                repo: "acme/api".into(),
                chunk: Some(v1::ObjectChunk {
                    object_id: "00".repeat(32),
                    total_size: 0,
                    data: objects[0].encode_envelope(),
                    last: true,
                }),
            }]),
        ))
        .await
        .unwrap_err();
    assert_eq!(ProtocolError::from_status(&forged).code, ErrorCode::ObjectCorrupt);

    // Missing/present objects.
    let missing = ObjectServiceClient::new(ch.clone())
        .missing(authed(
            &alice.token,
            v1::MissingObjectsRequest {
                repo: "acme/api".into(),
                object_ids: vec![objects[0].id().to_string(), "11".repeat(32)],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(missing.missing, vec!["11".repeat(32)]);

    // Subscribe before anything happens, then watch the change land.
    let mut events = EventServiceClient::new(ch.clone())
        .subscribe(authed(
            ADMIN_TOKEN,
            v1::SubscribeRequest { repo: "acme/api".into(), after_seq: u64::MAX, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner();

    let prop = ProposalServiceClient::new(ch.clone())
        .create(authed(
            &alice.token,
            v1::CreateProposalRequest {
                repo: "acme/api".into(),
                stream: "main".into(),
                head: head.to_string(),
                title: "first".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    let carol = user_with_role(&srv, "user:carol", "acme/api", "integrator").await;
    let res = IntegrationServiceClient::new(ch.clone())
        .integrate(authed(
            &carol.token,
            v1::IntegrateRequest { repo: "acme/api".into(), proposal: prop.id.clone(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(res.outcome.unwrap().status, "integrated");

    let head_now = StreamServiceClient::new(ch.clone())
        .read_head(authed(
            &alice.token,
            v1::ReadHeadRequest { repo: "acme/api".into(), stream: "main".into(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(head_now.head, head.to_string());

    // The live feed delivered both the proposal and the integration, in order.
    let topics = collect_topics(&mut events, "stream.integrate").await;
    assert!(
        topics.contains(&"proposal.create".to_string()) && topics.contains(&"stream.integrate".to_string()),
        "{topics:?}"
    );

    // Role management is an admin-only operation.
    let denied = AdminServiceClient::new(ch.clone())
        .grant_role(authed(
            &alice.token,
            v1::GrantRoleRequest { subject: "user:alice".into(), repo: "acme/api".into(), role: "admin".into() },
        ))
        .await
        .unwrap_err();
    assert_eq!(ProtocolError::from_status(&denied).code, ErrorCode::PermissionDenied);
    let t = std::time::Instant::now();
    srv.stop().await;
    assert!(t.elapsed() < Duration::from_secs(2), "shutdown must end open event streams");
}

async fn collect_topics(stream: &mut Streaming<v1::Event>, until: &str) -> Vec<String> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), stream.message()).await {
            Ok(Ok(Some(ev))) if ev.topic == "heartbeat" => {}
            Ok(Ok(Some(ev))) => {
                let done = ev.topic == until;
                out.push(ev.topic);
                if done {
                    return out;
                }
            }
            other => panic!("event stream ended early: {other:?} after {out:?}"),
        }
    }
    out
}
