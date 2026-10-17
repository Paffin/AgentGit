//! Tree manifests and tree IDs against the independent Python reference
//! (`tests/conformance/gen_vectors.py`).
#![allow(missing_docs)]

use ag_object::hash_blob;
use ag_tree::{Entry, Tree};
use ag_types::{FileType, Limits, RepoPath};
use serde_json::Value;

#[test]
fn tree_manifests_and_ids_match_reference() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/vectors.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let trees = v["trees"].as_array().unwrap();
    assert!(trees.len() >= 2);
    for t in trees {
        let name = t["name"].as_str().unwrap();
        let entries = t["entries"].as_array().unwrap().iter().map(|e| {
            (
                RepoPath::new(e["path"].as_str().unwrap()).unwrap(),
                Entry {
                    content: hash_blob(e["content"].as_str().unwrap().as_bytes()),
                    file_type: if e["type"].as_str() == Some("symlink") {
                        FileType::Symlink
                    } else {
                        FileType::Regular
                    },
                    executable: e["executable"].as_bool().unwrap_or(false),
                },
            )
        });
        let tree = Tree::from_entries(entries, &Limits::default()).unwrap();
        assert_eq!(hex::encode(tree.encode()), t["payload_hex"].as_str().unwrap(), "{name}: manifest");
        assert_eq!(tree.id().to_hex(), t["id"].as_str().unwrap(), "{name}: tree id");
        let bytes = hex::decode(t["payload_hex"].as_str().unwrap()).unwrap();
        assert_eq!(Tree::decode(&bytes, &Limits::default()).unwrap(), tree, "{name}: decode");
    }
}
