//! Wire-level behaviour of the protocol crate: error mapping and the JSON form of messages.

use std::collections::HashSet;

use ag_protocol::v1::{ErrorInfo, Event, FileChange, IntegrationOutcome, ObjectChunk};
use ag_protocol::{ErrorCode, ProtocolError, VERSION};
use prost::Message;

#[test]
fn error_codes_are_unique_parseable_and_mapped() {
    let mut seen = HashSet::new();
    for code in ErrorCode::ALL {
        assert!(seen.insert(code.as_str()), "duplicate wire string {code}");
        assert_eq!(ErrorCode::parse(code.as_str()), Some(*code));
        assert!((400..600).contains(&code.http_status()));
    }
    assert_eq!(ErrorCode::parse("nope"), None);
    assert!(ErrorCode::RateLimited.retryable());
    assert!(!ErrorCode::PermissionDenied.retryable());
}

#[test]
fn errors_survive_a_grpc_status_round_trip() {
    let e = ProtocolError::new(ErrorCode::StreamMoved, "stream 'main' moved")
        .with_detail("stream", "main")
        .with_retry_after(250)
        .with_request_id("req_1");
    let status = e.to_status();
    assert_eq!(status.code(), tonic::Code::Aborted);
    let back = ProtocolError::from_status(&status);
    assert_eq!(back, e);

    // A status from some other peer still maps to a sensible code.
    let plain = tonic::Status::unauthenticated("no token");
    assert_eq!(ProtocolError::from_status(&plain).code, ErrorCode::Unauthenticated);

    // A client's own deadline (tonic reports it as a cancelled call) is a deadline, which a caller may retry; a
    // call cancelled for any other reason is not.
    let expired = tonic::Status::cancelled("Timeout expired");
    assert_eq!(ProtocolError::from_status(&expired).code, ErrorCode::DeadlineExceeded);
    let other = tonic::Status::cancelled("the client went away");
    assert_eq!(ProtocolError::from_status(&other).code, ErrorCode::Internal);
}

#[test]
fn messages_round_trip_through_protobuf() {
    let info = ErrorInfo { code: "not_found".into(), message: "x".into(), retryable: false, ..Default::default() };
    let bytes = info.encode_to_vec();
    assert_eq!(ErrorInfo::decode(bytes.as_slice()).unwrap(), info);
}

#[test]
fn json_mapping_uses_base64_and_embedded_json() {
    let chunk = ObjectChunk { object_id: "ab".into(), total_size: 3, data: vec![1, 2, 3], last: true };
    let json = serde_json::to_value(&chunk).unwrap();
    assert_eq!(json["data"], "AQID");
    let back: ObjectChunk = serde_json::from_value(json).unwrap();
    assert_eq!(back, chunk);

    let change: FileChange = serde_json::from_str(r#"{"path":"a.txt","op":"put","content":"aGk"}"#).unwrap();
    assert_eq!(change.content, b"hi");

    let ev = Event {
        seq: 7,
        topic: "stream.updated".into(),
        repo: "acme/web".into(),
        timestamp_ms: 1,
        payload_json: r#"{"stream":"main"}"#.into(),
    };
    let json = serde_json::to_value(&ev).unwrap();
    assert_eq!(json["payload"]["stream"], "main");
    assert!(json.get("payload_json").is_none());
    let back: Event = serde_json::from_value(json).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&back.payload_json).unwrap()["stream"], "main");
}

#[test]
fn missing_fields_default_so_old_clients_and_new_servers_interoperate() {
    let outcome: IntegrationOutcome = serde_json::from_str(r#"{"status":"invalid","reason":"x"}"#).unwrap();
    assert_eq!(outcome.status, "invalid");
    assert!(outcome.blockers.is_empty());
    // Unknown fields from a newer peer are ignored by default.
    let _: IntegrationOutcome = serde_json::from_str(r#"{"status":"blocked","future_field":1}"#).unwrap();
}

#[test]
fn version_is_stable() {
    assert_eq!(VERSION, "agwp.v1");
}

#[test]
fn review_head_precondition_is_optional_and_survives_both_transports() {
    let legacy: ag_protocol::v1::ReviewProposalRequest =
        serde_json::from_str(r#"{"repo":"acme/web","id":"p1","decision":"approve"}"#).unwrap();
    assert!(legacy.expected_head.is_empty());
    let guarded = ag_protocol::v1::ReviewProposalRequest { expected_head: "inspected-head".into(), ..legacy };
    let bytes = guarded.encode_to_vec();
    assert_eq!(ag_protocol::v1::ReviewProposalRequest::decode(bytes.as_slice()).unwrap(), guarded);
    let json = serde_json::to_string(&guarded).unwrap();
    assert_eq!(serde_json::from_str::<ag_protocol::v1::ReviewProposalRequest>(&json).unwrap(), guarded);
}
