//! Compatibility: this build must still read every repository an earlier build wrote.
//!
//! `tests/compat/fixtures/repo-<label>.tar.gz` is a repository created by the `ag` of that release
//! (`tests/compat/generate.sh`), with `repo-<label>.expected.json` next to it: the identifiers and records that
//! release produced, cross-checked against an independent Python encoder when the fixture was made. The test
//! extracts each fixture, opens it with the code under test and checks that
//!
//! * `fsck` (every object hash, every head recomputed, the audit chain) is clean;
//! * streams, changesets, tree ids and the order of history are exactly the recorded ones;
//! * every merge changeset the old build made is reproduced bit for bit by today's merge (an unintended change
//!   of merge semantics shows up here), including the rename-aware one;
//! * blame follows the rename the same way;
//! * policy versions, signed attestations, agents' provenance, runs, tasks, leases and pins are all readable
//!   and the signatures still verify.
//!
//! Fixtures are append-only: when a persisted format changes on purpose, ADD a fixture (see
//! docs/architecture/compatibility.md); never edit or regenerate an old one. The tests are Unix-only because
//! the fixtures contain a symlink and are unpacked with `tar`.
#![cfg(unix)]
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::Command;

use ag_integrate::{BlameOptions, Repository};
use ag_merge::{Algorithm, BlobIo, IoError, merge_trees_with};
use ag_types::{BlobId, ChangesetId, TreeId};
use serde_json::Value;

struct Fixture {
    label: String,
    expected: Value,
    /// Keeps the extracted repository alive.
    _dir: tempfile::TempDir,
    repo: Repository,
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/fixtures")
}

/// Every fixture checked in, opened with the current code.
fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    let mut tars: Vec<PathBuf> = std::fs::read_dir(fixture_dir())
        .expect("tests/compat/fixtures exists")
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("repo-") && n.ends_with(".tar.gz")
        })
        .collect();
    tars.sort();
    assert!(!tars.is_empty(), "no compatibility fixtures found in {}", fixture_dir().display());
    for tar in tars {
        let name = tar.file_name().unwrap().to_string_lossy().into_owned();
        let label = name.trim_start_matches("repo-").trim_end_matches(".tar.gz").to_string();
        let expected: Value = serde_json::from_slice(
            &std::fs::read(tar.with_file_name(format!("repo-{label}.expected.json")))
                .expect("expected.json next to the fixture"),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let status = Command::new("tar").arg("-xzf").arg(&tar).arg("-C").arg(dir.path()).status().expect("tar");
        assert!(status.success(), "cannot unpack {name}");
        let repo = Repository::open(&dir.path().join("compat"))
            .unwrap_or_else(|e| panic!("fixture {label}: this build cannot open the repository: {e}"));
        out.push(Fixture { label, expected, _dir: dir, repo });
    }
    out
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_else(|| panic!("expected.json: {key} is not a string in {v}"))
}

fn cid(hex: &str) -> ChangesetId {
    hex.parse().unwrap()
}

struct Blobs<'a>(&'a Repository);

impl BlobIo for Blobs<'_> {
    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>, IoError> {
        self.0.blob(id).map_err(|e| Box::new(e) as IoError)
    }

    fn write_blob(&self, data: Vec<u8>) -> Result<BlobId, IoError> {
        self.0.put_blob(data).map_err(|e| Box::new(e) as IoError)
    }
}

#[test]
fn the_repository_verifies() {
    for f in fixtures() {
        let report = f.repo.fsck().unwrap();
        assert!(report.is_ok(), "fixture {}: fsck found {:?}", f.label, report.issues);
        assert!(
            report.objects_checked > 0
                && report.changesets_checked == f.expected["changesets"].as_array().unwrap().len()
        );
        assert_eq!(report.audit_entries, f.expected["audit"]["entries"].as_u64().unwrap(), "fixture {}", f.label);
        assert_eq!(
            report.audit_head,
            s(&f.expected["audit"], "head"),
            "fixture {}: the audit chain head differs",
            f.label
        );
        assert_eq!(f.repo.config().namespace.as_str(), s(&f.expected, "namespace"));
    }
}

#[test]
fn streams_history_and_trees_are_read_back_exactly() {
    for f in fixtures() {
        // streams
        let rows = f.repo.meta().read(|tx| tx.list_streams()).unwrap();
        let streams = f.expected["streams"].as_object().unwrap();
        assert_eq!(rows.len(), streams.len(), "fixture {}", f.label);
        for row in &rows {
            let want = &streams[row.name.as_str()];
            assert_eq!(
                row.state.head.map(|h| h.to_hex()),
                want["head"].as_str().map(str::to_string),
                "stream {}",
                row.name
            );
            assert_eq!(row.state.version, want["version"].as_u64().unwrap(), "stream {}", row.name);
        }
        let head = cid(s(&f.expected, "head"));
        assert_eq!(f.repo.resolve_rev("main").unwrap(), head);

        // every changeset: fields, tree id, and the file count of its tree
        for want in f.expected["changesets"].as_array().unwrap() {
            let id = cid(s(want, "id"));
            let cs = f.repo.changeset(&id).unwrap();
            assert_eq!(cs.id(), id, "fixture {}: the changeset hashes to another id today", f.label);
            assert_eq!(cs.author.as_str(), s(want, "author"));
            assert_eq!(cs.timestamp.as_millis(), want["timestamp_ms"].as_i64().unwrap());
            assert_eq!(cs.message, s(want, "message"));
            let parents: Vec<String> = cs.parents.iter().map(|p| p.to_hex()).collect();
            let want_parents: Vec<&str> =
                want["parents"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
            assert_eq!(parents, want_parents);
            let tree = f.repo.tree_of(&id).unwrap();
            assert_eq!(
                tree.id().to_hex(),
                s(want, "tree_id"),
                "fixture {}: tree id of {} changed",
                f.label,
                id.short()
            );
            assert_eq!(tree.len() as u64, want["files"].as_u64().unwrap());
        }

        // log: newest first, the same set, parents before children
        let log = f.repo.log(&head, 1000, None).unwrap();
        let want_ids: std::collections::BTreeSet<String> =
            f.expected["changesets"].as_array().unwrap().iter().map(|c| s(c, "id").to_string()).collect();
        let got_ids: std::collections::BTreeSet<String> = log.iter().map(|e| e.id.to_hex()).collect();
        assert_eq!(got_ids, want_ids);
        assert_eq!(log[0].id, head);
        for w in log.windows(2) {
            assert!(w[0].generation >= w[1].generation, "log must be ordered by generation");
        }

        // the head's files: path -> blob, type, executable
        let tree = f.repo.tree_of(&head).unwrap();
        let files = f.expected["files_at_head"].as_object().unwrap();
        assert_eq!(tree.len(), files.len());
        for (path, e) in tree.iter() {
            let want = &files[path.as_str()];
            assert_eq!(e.content.to_hex(), s(want, "content"), "{path}");
            assert_eq!(e.file_type.to_string(), if s(want, "type") == "file" { "file" } else { "symlink" });
            assert_eq!(e.executable, want["executable"].as_bool().unwrap(), "{path}");
        }
    }
}

#[test]
fn every_merge_the_old_build_made_is_reproduced_by_this_one() {
    for f in fixtures() {
        let merges = f.expected["merge_changesets"].as_array().unwrap();
        assert!(!merges.is_empty(), "the fixture must contain merges");
        let mut renames_seen = 0;
        for m in merges {
            let id = cid(m.as_str().unwrap());
            let cs = f.repo.changeset(&id).unwrap();
            assert_eq!(cs.parents.len(), 2);
            let base = cs
                .message
                .lines()
                .find_map(|l| l.strip_prefix("Merge-Base: "))
                .map(cid)
                .expect("integration merges record their base");
            let algorithm = cs.message.lines().find_map(|l| l.strip_prefix("Merge-Algorithm: ")).unwrap();
            let (base_t, ours_t, theirs_t) = (
                f.repo.tree_of(&base).unwrap(),
                f.repo.tree_of(&cs.parents[0]).unwrap(),
                f.repo.tree_of(&cs.parents[1]).unwrap(),
            );
            // A recorded merge is verified by the algorithm version it names, not by today's.
            let recorded = Algorithm::parse(algorithm).unwrap_or_else(|| {
                panic!("fixture {}: merge {} names an unknown algorithm {algorithm}", f.label, id.short())
            });
            let merged = merge_trees_with(recorded, &base_t, &ours_t, &theirs_t, &Blobs(&f.repo), f.repo.limits())
                .expect("merge runs");
            assert!(
                merged.is_clean(),
                "fixture {}: merge {} now conflicts: {:?}",
                f.label,
                id.short(),
                merged.conflicts
            );
            assert_eq!(
                merged.tree.id(),
                f.repo.tree_of(&id).unwrap().id(),
                "fixture {}: merge {} (recorded as {algorithm}, today {}) produces a different tree - a change of merge semantics \
                 needs an ADR and a new ALGORITHM_VERSION",
                f.label,
                id.short(),
                ag_merge::ALGORITHM_VERSION
            );
            renames_seen += f.repo.changeset_renames(&cs).unwrap().len();
        }
        assert!(renames_seen >= 1, "the fixture holds a rename-aware merge");
    }
}

#[test]
fn blame_follows_renames_as_before() {
    for f in fixtures() {
        let head = cid(s(&f.expected, "head"));
        for (name, path) in [("main", "src/main.rs"), ("helpers", "src/helpers.rs")] {
            let blame = f.repo.blame(&head, path, &BlameOptions::default()).unwrap();
            let want = f.expected["blame"][name].as_array().unwrap();
            assert_eq!(blame.lines.len(), want.len(), "{path}");
            for (got, w) in blame.lines.iter().zip(want) {
                assert_eq!(got.text, s(w, "text"));
                assert_eq!(got.changeset.to_hex(), s(w, "changeset"), "{path}:{} blames another changeset", got.line);
                assert_eq!(got.path, s(w, "path"));
            }
        }
    }
}

#[test]
fn policy_attestations_and_provenance_are_still_valid() {
    for f in fixtures() {
        // policy versions and their canonical documents
        let history = f.repo.policy_history("main").unwrap();
        let want = f.expected["policy_versions"].as_array().unwrap();
        assert_eq!(history.len(), want.len());
        for (row, w) in history.iter().zip(want) {
            assert_eq!(row.version, w["version"].as_u64().unwrap());
            assert_eq!(hex_sha256(row.doc.as_bytes()), s(w, "doc_sha256"));
            assert_eq!(row.doc_hash, s(w, "doc_sha256"), "the stored hash of the document");
            ag_policy::Policy::parse(&row.doc).expect("an old policy document still parses");
        }
        let (current, version) = f.repo.policy("main").unwrap();
        assert_eq!(version, want.len() as u64, "the current policy is the latest version");
        assert!(current.required_validations.iter().any(|v| v.check == "tests"));

        // attestations: the same rows, and the signatures verify
        let mut seen = 0;
        for c in f.expected["changesets"].as_array().unwrap() {
            let tree: TreeId = s(c, "tree_id").parse().unwrap();
            let rows = f.repo.attestations(&tree).unwrap();
            let mut ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
            ids.sort();
            let mut want_ids: Vec<&str> = f.expected["attestations"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|a| s(a, "tree_id") == tree.to_hex())
                .map(|a| s(a, "id"))
                .collect();
            want_ids.sort();
            assert_eq!(ids, want_ids, "attestations of tree {}", tree.short());
            for row in rows {
                let att: ag_policy::Attestation = serde_json::from_value(row.payload.clone()).unwrap();
                att.verify_signature().expect("an old attestation's signature verifies");
                assert_eq!(att.tree_id, tree);
                seen += 1;
            }
        }
        assert_eq!(seen, f.expected["attestations"].as_array().unwrap().len());
        assert!(seen > 0);

        // provenance: signature, registered key and the changeset it is filed under
        let mut verified = 0;
        for c in f.expected["changesets"].as_array().unwrap() {
            let id = cid(s(c, "id"));
            f.repo
                .meta()
                .read(|tx| {
                    for row in tx.provenance_for(&id)? {
                        ag_agent::provenance::verify(tx, &row).expect("old provenance still verifies");
                        verified += 1;
                    }
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(verified, f.expected["agents"]["provenance"].as_array().unwrap().len());
        assert!(verified > 0);
    }
}

#[test]
fn agents_tasks_leases_and_pins_are_readable() {
    for f in fixtures() {
        let (runs, tasks, leases, agents) = f
            .repo
            .meta()
            .read(|tx| Ok((tx.runs(None, 100)?, tx.tasks(None)?, tx.leases_all(100)?, tx.agents()?)))
            .unwrap();
        let want_runs = f.expected["agents"]["runs"].as_array().unwrap();
        assert_eq!(runs.len(), want_runs.len());
        assert_eq!(runs[0].run_id, s(&want_runs[0], "run_id"));
        assert_eq!(runs[0].status, s(&want_runs[0], "status"));
        assert!(agents.iter().any(|a| a.name == "docs-bot"));

        let mut got: Vec<(String, String)> = tasks.iter().map(|t| (t.id.clone(), t.state.clone())).collect();
        got.sort();
        let mut want: Vec<(String, String)> =
            f.expected["tasks"].as_array().unwrap().iter().map(|t| (s(t, "id").into(), s(t, "state").into())).collect();
        want.sort();
        assert_eq!(got, want);
        let deps = f
            .repo
            .meta()
            .read(|tx| tx.task_deps(&tasks.iter().find(|t| t.title == "publish the release").unwrap().id))
            .unwrap();
        assert_eq!(deps.len(), 1, "the dependency between the two tasks");

        let mut got: Vec<(String, bool, u64)> =
            leases.iter().map(|l| (l.scope.clone(), l.exclusive, l.token)).collect();
        got.sort();
        let mut want: Vec<(String, bool, u64)> = f.expected["leases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| (s(l, "scope").into(), l["exclusive"].as_bool().unwrap(), l["token"].as_u64().unwrap()))
            .collect();
        want.sort();
        assert_eq!(got, want);

        let pins = f.repo.pins().unwrap();
        let want_pins = f.expected["pins"].as_array().unwrap();
        assert_eq!(pins.len(), want_pins.len());
        assert_eq!(pins[0].name, s(&want_pins[0], "name"));
        assert_eq!(pins[0].changeset.to_hex(), s(&want_pins[0], "changeset"));
    }
}

#[test]
fn a_fixture_repository_still_accepts_new_work() {
    // Reading is not enough: a later version must be able to continue the history an old one wrote.
    for f in fixtures() {
        let head = f.repo.resolve_rev("main").unwrap();
        let before = f.repo.tree_of(&head).unwrap();
        let blob = f.repo.put_blob(b"added by the current build\n".to_vec()).unwrap();
        let actor = ag_types::Identity::new("user:alice").unwrap();
        let cs = ag_changeset::Changeset::build(
            f.repo.config().namespace.clone(),
            vec![head],
            actor.clone(),
            f.repo.clock().now(),
            "continue an old history",
            vec![ag_changeset::Op::Put {
                path: ag_types::RepoPath::new("NEW.txt").unwrap(),
                content: blob,
                file_type: ag_types::FileType::Regular,
                executable: false,
            }],
            f.repo.limits(),
        )
        .unwrap();
        let id = f.repo.put_changeset(&cs).unwrap();
        assert_eq!(f.repo.tree_of(&id).unwrap().len(), before.len() + 1);
        assert!(f.repo.publish(&id, &actor, None).unwrap().state == ag_integrate::PublishState::Published);
        assert!(f.repo.fsck().unwrap().is_ok(), "fixture {}: fsck after new work", f.label);
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Every fixture - the schema-2 one written by the first alpha and the schema-3 one written after the proposal-paging
/// indexes were added - ends up at the current metadata schema with those indexes once this build has opened it.
#[test]
fn every_fixture_is_at_the_current_metadata_schema_with_the_paging_indexes() {
    for f in fixtures() {
        let current = ag_storage::meta::SCHEMA_VERSION.to_string();
        assert_eq!(
            f.repo.meta().read(|tx| tx.meta_get("schema")).unwrap().as_deref(),
            Some(current.as_str()),
            "fixture {}",
            f.label
        );
        let indexes = f
            .repo
            .meta()
            .read(|tx| {
                tx.raw_query(
                    "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'proposals_by_%created'",
                    &[],
                )
            })
            .unwrap();
        assert_eq!(indexes.len(), 3, "fixture {}", f.label);
    }
}
