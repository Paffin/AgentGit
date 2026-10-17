//! Attestation signing bytes against the independent Python reference.
#![allow(missing_docs)]

use ag_policy::{Attestation, CheckResult};
use ag_types::{Identity, Timestamp, TreeId};
use serde_json::Value;

#[test]
fn attestation_signing_bytes_match_reference() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/vectors.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let list = v["attestations"].as_array().unwrap();
    assert!(list.len() >= 2);
    for a in list {
        let bytes = Attestation::signing_bytes(
            a["id"].as_str().unwrap(),
            &a["tree_id"].as_str().unwrap().parse::<TreeId>().unwrap(),
            a["check"].as_str().unwrap(),
            if a["result"] == "pass" { CheckResult::Pass } else { CheckResult::Fail },
            a["policy_version"].as_u64().unwrap(),
            a["environment_digest"].as_str().unwrap(),
            &Identity::new(a["issuer"].as_str().unwrap()).unwrap(),
            Timestamp::from_millis(a["issued_at_ms"].as_i64().unwrap()),
        );
        assert_eq!(hex::encode(bytes), a["signing_bytes_hex"].as_str().unwrap());
    }
}
