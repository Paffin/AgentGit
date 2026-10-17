//! The hash-chained audit log against the independent Python reference.
#![allow(missing_docs)]

use std::sync::Arc;

use ag_storage::{ManualClock, Meta};
use ag_types::Timestamp;
use serde_json::Value;

#[test]
fn audit_chain_matches_reference() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/vectors.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let rows = v["audit"].as_array().unwrap();
    assert!(rows.len() >= 3);
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(0)));
    let meta = Meta::open_in_memory(clock.clone()).unwrap();
    for r in rows {
        clock.set(Timestamp::from_millis(r["ts"].as_i64().unwrap()));
        let e = meta
            .write(|tx| {
                tx.audit_append(
                    r["actor"].as_str().unwrap(),
                    r["action"].as_str().unwrap(),
                    r["target"].as_str().unwrap(),
                    &r["details"],
                )
            })
            .unwrap();
        assert_eq!(e.prev_hash, r["prev_hash"].as_str().unwrap());
        assert_eq!(e.hash, r["hash"].as_str().unwrap(), "audit hash of {}", r["action"]);
    }
    assert!(meta.read(|tx| tx.audit_verify()).unwrap().first_bad.is_none());
}
