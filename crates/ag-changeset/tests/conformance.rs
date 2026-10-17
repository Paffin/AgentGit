//! Checks the Rust encoder against vectors produced by the independent
//! Python reference implementation in `tests/conformance/gen_vectors.py`.

use ag_changeset::{Changeset, Op};
use ag_object::hash_blob;
use ag_types::{ChangesetId, FileType, Identity, Limits, Namespace, RepoPath, Timestamp};
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/vectors.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("vectors.json")).unwrap()
}

#[test]
fn blob_ids_match_reference() {
    for b in vectors()["blobs"].as_array().unwrap() {
        let content = b["content_utf8"].as_str().unwrap();
        assert_eq!(hash_blob(content.as_bytes()).to_hex(), b["id"].as_str().unwrap());
    }
}

#[test]
fn changeset_encoding_and_ids_match_reference() {
    let v = vectors();
    let list = v["changesets"].as_array().unwrap();
    assert!(!list.is_empty());
    for c in list {
        let name = c["name"].as_str().unwrap();
        let ops: Vec<Op> = c["ops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| {
                let path = RepoPath::new(o["path"].as_str().unwrap()).unwrap();
                match o["op"].as_str().unwrap() {
                    "put" => Op::Put {
                        path,
                        content: hash_blob(o["content"].as_str().unwrap().as_bytes()),
                        file_type: if o["type"].as_str() == Some("symlink") {
                            FileType::Symlink
                        } else {
                            FileType::Regular
                        },
                        executable: o["executable"].as_bool().unwrap_or(false),
                    },
                    _ => Op::Delete { path },
                }
            })
            .collect();
        let parents = c["parents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().parse::<ChangesetId>().unwrap())
            .collect();
        let cs = Changeset::build(
            Namespace::new(c["namespace"].as_str().unwrap()).unwrap(),
            parents,
            Identity::new(c["author"].as_str().unwrap()).unwrap(),
            Timestamp::from_millis(c["timestamp_ms"].as_i64().unwrap()),
            c["message"].as_str().unwrap(),
            ops,
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(hex::encode(cs.encode()), c["payload_hex"].as_str().unwrap(), "{name}: payload");
        assert_eq!(cs.id().to_hex(), c["id"].as_str().unwrap(), "{name}: id");

        // And the reference bytes decode back to the same value.
        let bytes = hex::decode(c["payload_hex"].as_str().unwrap()).unwrap();
        assert_eq!(Changeset::decode(&bytes, &Limits::default()).unwrap(), cs, "{name}: decode");
    }
}
