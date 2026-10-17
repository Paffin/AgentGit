use std::sync::Arc;

use ag_changeset::{Changeset, Op};
use ag_object::hash_blob;
use ag_tree::Tree;
use ag_types::{FileType, Identity, Limits, Namespace, RepoPath, StreamName, Timestamp};
use serde_json::json;

use super::*;
use crate::clock::ManualClock;
use crate::testing::TestMeta;

mod jobs;
mod semantics;

fn meta() -> TestMeta {
    TestMeta::new(clock())
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(Timestamp::from_millis(1_000_000)))
}

fn cs(parents: Vec<ChangesetId>, file: &str) -> (ChangesetId, Changeset) {
    let c = Changeset::build(
        Namespace::new("t/t").unwrap(),
        parents,
        Identity::new("user:t").unwrap(),
        Timestamp::from_millis(0),
        format!("add {file}\n\nbody"),
        vec![Op::Put {
            path: RepoPath::new(file).unwrap(),
            content: hash_blob(file.as_bytes()),
            file_type: FileType::Regular,
            executable: false,
        }],
        &Limits::default(),
    )
    .unwrap();
    (c.id(), c)
}

#[test]
fn schema_is_created_once_and_versioned() {
    let first = TestMeta::shared(clock());
    first.write(|tx| tx.meta_set("k", "v")).unwrap();
    let again = first.reopen().unwrap();
    assert_eq!(again.read(|tx| tx.meta_get("k")).unwrap().as_deref(), Some("v"));
    assert_eq!(again.read(|tx| tx.meta_get("schema")).unwrap().as_deref(), Some(SCHEMA_VERSION.to_string().as_str()));
    assert_eq!(SCHEMA_VERSION, 2, "bump the migration tests together with the version");
    assert!(again.read(|tx| tx.meta_get("created_at")).unwrap().is_some());
}

#[test]
fn refuses_newer_schema() {
    let m = TestMeta::shared(clock());
    if let Some(path) = m.sqlite_path() {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        assert!(matches!(Meta::open(&path), Err(MetaError::UnsupportedSchema { found: 99, .. })));
    } else {
        m.write(|tx| tx.raw_execute("UPDATE meta SET value = '99' WHERE key = 'schema'", &[])).unwrap();
        assert!(matches!(m.reopen(), Err(MetaError::UnsupportedSchema { found: 99, .. })));
    }
}

#[test]
fn a_version_1_database_migrates_forward_and_keeps_its_data() {
    // Build a database exactly as release 0.1 left it: schema 1 only.
    let dir = std::env::temp_dir().join(crate::testing::unique("agtest-v1"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("meta.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(migrate::SQLITE_MIGRATIONS[0].sql).unwrap();
        conn.execute("INSERT INTO meta (key, value) VALUES ('created_at', '5'), ('schema', '1')", []).unwrap();
        conn.execute(
            "INSERT INTO streams (name, head, version, created_at, created_by) VALUES ('main', NULL, 0, 1, 'user:t')",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let m = Meta::open(&path).unwrap();
    assert_eq!(m.read(|tx| tx.meta_get("schema")).unwrap().as_deref(), Some("2"));
    assert_eq!(m.read(|tx| tx.meta_get("created_at")).unwrap().as_deref(), Some("5"), "creation time is kept");
    assert!(m.read(|tx| tx.stream("main")).unwrap().is_some(), "existing rows survive");
    let job = m.write(|tx| tx.job_enqueue("k", &json!({}), None, Timestamp::from_millis(0))).unwrap();
    assert_eq!(job.state, JobState::Queued, "the jobs table exists after migrating");
    let version: i64 =
        rusqlite::Connection::open(&path).unwrap().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(version, 2);
    drop(m);
    Meta::open(&path).unwrap(); // opening again changes nothing
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn register_changeset_computes_generation_and_requires_parents() {
    let m = meta();
    let (a, ca) = cs(vec![], "a");
    let (b, cb) = cs(vec![a], "b");
    let (side, cside) = cs(vec![a], "s");
    let (mrg, cm) = cs(vec![b, side], "m");

    // child before parent is refused
    assert!(matches!(m.write(|tx| tx.register_changeset(&b, &cb)), Err(MetaError::ParentNotReady(_))));
    assert_eq!(m.write(|tx| tx.register_changeset(&a, &ca)).unwrap(), 1);
    assert_eq!(m.write(|tx| tx.register_changeset(&b, &cb)).unwrap(), 2);
    assert_eq!(m.write(|tx| tx.register_changeset(&side, &cside)).unwrap(), 2);
    assert_eq!(m.write(|tx| tx.register_changeset(&mrg, &cm)).unwrap(), 3);
    // idempotent
    assert_eq!(m.write(|tx| tx.register_changeset(&mrg, &cm)).unwrap(), 3);

    m.read(|tx| {
        assert_eq!(tx.parents(&mrg)?, vec![b, side]);
        let info = tx.changeset_info(&mrg)?.unwrap();
        assert_eq!(info.generation, 3);
        assert_eq!(info.summary, "add m");
        assert!(tx.touches_path(&mrg, "m")?);
        assert!(!tx.touches_path(&mrg, "a")?);
        assert_eq!(tx.count_changesets()?, 4);
        Ok(())
    })
    .unwrap();

    // the Graph impl agrees with ag-dag algorithms
    assert_eq!(ag_dag::merge_bases(&*m, &b, &side).unwrap(), vec![a]);
    assert!(ag_dag::is_ancestor(&*m, &a, &mrg).unwrap());
}

#[test]
fn prefix_resolution() {
    let m = meta();
    let (a, ca) = cs(vec![], "a");
    m.write(|tx| tx.register_changeset(&a, &ca)).unwrap();
    let hex = a.to_hex();
    assert_eq!(m.read(|tx| tx.resolve_prefix(&hex[..8], 5)).unwrap(), vec![a]);
    assert!(m.read(|tx| tx.resolve_prefix("ffffffffffff", 5)).unwrap().is_empty() || hex.starts_with("ffffffffffff"));
    assert!(m.read(|tx| tx.resolve_prefix("XYZ", 5)).is_err());
}

#[test]
fn stream_cas_semantics() {
    let m = meta();
    let (a, ca) = cs(vec![], "a");
    let (b, cb) = cs(vec![a], "b");
    m.write(|tx| {
        tx.register_changeset(&a, &ca)?;
        tx.register_changeset(&b, &cb)?;
        tx.create_stream(&StreamName::new("main").unwrap(), "user:t")?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        m.write(|tx| tx.create_stream(&StreamName::new("main").unwrap(), "user:t")),
        Err(MetaError::AlreadyExists(_))
    ));

    let s0 = m.read(|tx| tx.stream("main")).unwrap().unwrap().state;
    assert_eq!(s0, StreamState { head: None, version: 0 });

    let s1 = m.write(|tx| tx.cas_stream("main", &s0, &a, Some("op_1"), "user:t", "first")).unwrap();
    assert_eq!(s1, StreamState { head: Some(a), version: 1 });

    // stale token is rejected, state unchanged
    let err = m.write(|tx| tx.cas_stream("main", &s0, &b, None, "user:t", "stale")).unwrap_err();
    assert!(matches!(err, MetaError::CasMismatch { expected: 0, actual: 1, .. }));
    assert_eq!(m.read(|tx| tx.stream("main")).unwrap().unwrap().state, s1);

    // unpublished changesets cannot become heads
    let (ghost, _) = cs(vec![], "ghost");
    assert!(m.write(|tx| tx.cas_stream("main", &s1, &ghost, None, "user:t", "x")).is_err());

    let s2 = m.write(|tx| tx.cas_stream("main", &s1, &b, None, "user:t", "second")).unwrap();
    assert_eq!(s2.version, 2);
    let log = m.read(|tx| tx.reflog("main", 10)).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].new_head, Some(b));
    assert_eq!(log[0].old_head, Some(a));
    assert_eq!(log[1].operation_id.as_deref(), Some("op_1"));
}

#[test]
fn failed_write_rolls_back_everything() {
    let m = meta();
    let (a, ca) = cs(vec![], "a");
    let r: Result<(), MetaError> = m.write(|tx| {
        tx.register_changeset(&a, &ca)?;
        tx.audit_append("user:t", "x", "y", &json!({}))?;
        Err(MetaError::Invalid("boom".into()))
    });
    assert!(r.is_err());
    assert_eq!(m.read(|tx| tx.count_changesets()).unwrap(), 0);
    assert_eq!(m.read(|tx| tx.count_rows("audit")).unwrap(), 0);
}

#[test]
fn intents_are_idempotent_by_key() {
    let m = meta();
    let payload = json!({"head": "x"});
    let new = |op: &'static str, key: &'static str, hash: &'static str| NewIntent {
        operation_id: op,
        idempotency_key: key,
        request_hash: hash,
        kind: "publish",
        actor: "user:t",
        stream: Some("main"),
        payload: &payload,
    };
    let (i1, created) = m.write(|tx| tx.intent_create(&new("op_00000001", "key-1", "h1"))).unwrap();
    assert!(created);
    // replay with the same key and request returns the original, not a new intent
    let (i2, created) = m.write(|tx| tx.intent_create(&new("op_00000002", "key-1", "h1"))).unwrap();
    assert!(!created);
    assert_eq!(i2.operation_id, i1.operation_id);
    // same key, different request
    assert!(matches!(
        m.write(|tx| tx.intent_create(&new("op_00000003", "key-1", "OTHER"))),
        Err(MetaError::IdempotencyKeyReused(_))
    ));
    // duplicate operation id with a different key
    assert!(matches!(
        m.write(|tx| tx.intent_create(&new("op_00000001", "key-2", "h2"))),
        Err(MetaError::AlreadyExists(_))
    ));
}

#[test]
fn intent_transitions_are_guarded() {
    let m = meta();
    let payload = json!({});
    m.write(|tx| {
        tx.intent_create(&NewIntent {
            operation_id: "op_00000001",
            idempotency_key: "k",
            request_hash: "h",
            kind: "publish",
            actor: "user:t",
            stream: None,
            payload: &payload,
        })
    })
    .unwrap();
    let row = m
        .write(|tx| tx.intent_transition("op_00000001", &[IntentState::Prepared], IntentState::Uploaded, None, None))
        .unwrap();
    assert_eq!(row.state, IntentState::Uploaded);
    // cannot skip back
    assert!(matches!(
        m.write(|tx| tx.intent_transition("op_00000001", &[IntentState::Prepared], IntentState::Verified, None, None)),
        Err(MetaError::InvalidTransition { .. })
    ));
    let done = m
        .write(|tx| {
            tx.intent_transition(
                "op_00000001",
                &[IntentState::Uploaded],
                IntentState::Integrated,
                Some(&json!({"head": "h"})),
                None,
            )
        })
        .unwrap();
    assert!(done.state.is_terminal());
    assert_eq!(done.result.unwrap()["head"], "h");
}

#[test]
fn staged_objects_are_protected_until_intent_finishes_and_grace_passes() {
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(10_000)));
    let m = TestMeta::new(clock.clone());
    let payload = json!({});
    let obj = hash_blob(b"staged").object_id();
    m.write(|tx| {
        tx.intent_create(&NewIntent {
            operation_id: "op_00000001",
            idempotency_key: "k",
            request_hash: "h",
            kind: "publish",
            actor: "user:t",
            stream: None,
            payload: &payload,
        })?;
        tx.intent_add_object("op_00000001", &obj)
    })
    .unwrap();
    let since = Timestamp::from_millis(9_999);
    assert_eq!(m.read(|tx| tx.protected_intent_objects(since)).unwrap(), vec![obj]);
    m.write(|tx| tx.intent_transition("op_00000001", &[IntentState::Prepared], IntentState::Aborted, None, None))
        .unwrap();
    // terminal but updated recently -> still protected
    assert_eq!(m.read(|tx| tx.protected_intent_objects(since)).unwrap(), vec![obj]);
    clock.advance_ms(100_000);
    assert!(m.read(|tx| tx.protected_intent_objects(Timestamp::from_millis(100_000))).unwrap().is_empty());
}

#[test]
fn outbox_is_at_least_once() {
    let m = meta();
    let id = m.write(|tx| tx.outbox_push("changeset.published", &json!({"id": "x"}))).unwrap();
    let pending = m.read(|tx| tx.outbox_pending(10)).unwrap();
    assert_eq!(pending.len(), 1);
    m.write(|tx| tx.outbox_fail(id, "handler crashed")).unwrap();
    let pending = m.read(|tx| tx.outbox_pending(10)).unwrap();
    assert_eq!(pending[0].attempts, 1);
    assert_eq!(pending[0].last_error.as_deref(), Some("handler crashed"));
    m.write(|tx| tx.outbox_ack(id)).unwrap();
    assert!(m.read(|tx| tx.outbox_pending(10)).unwrap().is_empty());
    assert_eq!(m.read(|tx| tx.outbox_history(10)).unwrap().len(), 1);
}

#[test]
fn audit_chain_detects_tampering() {
    let m = meta();
    for i in 0..5 {
        m.write(|tx| tx.audit_append("user:t", "act", &format!("t{i}"), &json!({"i": i}))).unwrap();
    }
    let rep = m.read(|tx| tx.audit_verify()).unwrap();
    assert_eq!((rep.entries, rep.first_bad), (5, None));

    // An attacker edits an old row directly in the database.
    m.write(|tx| tx.raw_execute("UPDATE audit SET details = '{\"i\":99}' WHERE seq = 3", &[])).unwrap();
    let rep = m.read(|tx| tx.audit_verify()).unwrap();
    assert_eq!(rep.first_bad, Some(3));

    // Deleting a row breaks the chain at the following entry.
    m.write(|tx| {
        tx.raw_execute("UPDATE audit SET details = '{\"i\":2}' WHERE seq = 3", &[])?;
        tx.raw_execute("DELETE FROM audit WHERE seq = 2", &[])
    })
    .unwrap();
    let rep = m.read(|tx| tx.audit_verify()).unwrap();
    assert_eq!(rep.first_bad, Some(3));
}

#[test]
fn derived_trees_roundtrip_and_detect_cache_corruption() {
    let m = meta();
    let (a, ca) = cs(vec![], "a");
    m.write(|tx| tx.register_changeset(&a, &ca)).unwrap();
    assert_eq!(m.read(|tx| tx.changesets_without_tree(10)).unwrap(), vec![a]);
    let tree = Tree::empty().apply(&ca.ops, &Limits::default()).unwrap();
    let tid = m.write(|tx| tx.put_derived_tree(&a, &tree)).unwrap();
    assert_eq!(tid, tree.id());
    assert_eq!(m.read(|tx| tx.derived_tree(&a)).unwrap().unwrap(), tree);
    assert!(m.read(|tx| tx.changesets_without_tree(10)).unwrap().is_empty());

    // corrupt the cache: the result must be "absent", never a wrong tree
    m.write(|tx| tx.raw_execute("UPDATE trees SET data = ?1", &[Param::from(vec![0u8])])).unwrap();
    assert!(m.read(|tx| tx.derived_tree(&a)).unwrap().is_none());
}

#[test]
fn counters_are_monotonic_and_proposals_track_state() {
    let m = meta();
    let c: Vec<u64> = (0..3).map(|_| m.write(|tx| tx.next_counter("fence")).unwrap()).collect();
    assert_eq!(c, vec![1, 2, 3]);

    let (a, ca) = cs(vec![], "a");
    let (b, cb) = cs(vec![a], "b");
    m.write(|tx| {
        tx.register_changeset(&a, &ca)?;
        tx.register_changeset(&b, &cb)?;
        tx.create_stream(&StreamName::new("main").unwrap(), "user:t")?;
        tx.proposal_create(&NewProposal {
            id: "prop_1",
            stream: "main",
            head: &a,
            title: "t",
            description: "d",
            author: "user:t",
            task_id: None,
        })?;
        tx.review_add("prop_1", "user:r", "approve", "lgtm", &a)?;
        Ok(())
    })
    .unwrap();
    m.write(|tx| tx.proposal_set_head("prop_1", &b)).unwrap();
    let p = m.read(|tx| tx.proposal("prop_1")).unwrap().unwrap();
    assert_eq!(p.head, b);
    let reviews = m.read(|tx| tx.reviews("prop_1")).unwrap();
    assert_eq!(reviews[0].head, a, "review stays bound to the head it approved");
    // only the proposal's *current* head is a retention root
    assert_eq!(m.read(|tx| tx.live_roots()).unwrap(), vec![b]);
    m.write(|tx| tx.proposal_set_state("prop_1", "closed", None)).unwrap();
    assert!(m.read(|tx| tx.proposals_list(Some("open"), 10)).unwrap().is_empty());
}

#[test]
fn policy_versions_are_append_only() {
    let m = meta();
    let v1 = m.write(|tx| tx.policy_put("main", "{\"a\":1}", "user:admin")).unwrap();
    let v2 = m.write(|tx| tx.policy_put("main", "{\"a\":2}", "user:admin")).unwrap();
    assert_eq!((v1.version, v2.version), (1, 2));
    assert_ne!(v1.doc_hash, v2.doc_hash);
    assert_eq!(m.read(|tx| tx.policy_current("main")).unwrap().unwrap().version, 2);
    assert_eq!(m.read(|tx| tx.policy_version("main", 1)).unwrap().unwrap().doc, "{\"a\":1}");
    assert!(m.read(|tx| tx.policy_current("other")).unwrap().is_none());
}

#[test]
fn concurrent_cas_from_many_connections_never_loses_an_update() {
    let setup = TestMeta::shared(clock());
    // 24 independent changesets; each worker tries to advance the stream to its own.
    let mut chain = Vec::new();
    let mut parent: Vec<ChangesetId> = vec![];
    for i in 0..24 {
        let (id, c) = cs(parent.clone(), &format!("f{i}"));
        setup.write(|tx| tx.register_changeset(&id, &c)).unwrap();
        parent = vec![id];
        chain.push(id);
    }
    setup.write(|tx| tx.create_stream(&StreamName::new("main").unwrap(), "user:t")).unwrap();

    let wins = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let handles: Vec<_> = chain
        .iter()
        .map(|target| {
            let (m, target, wins) = (setup.reopen().unwrap(), *target, wins.clone());
            std::thread::spawn(move || {
                // classic optimistic loop: read, then CAS; retry on mismatch
                for _ in 0..200 {
                    let st = m.read(|tx| tx.stream("main")).unwrap().unwrap().state;
                    match m.write(|tx| tx.cas_stream("main", &st, &target, None, "user:t", "race")) {
                        Ok(_) => {
                            wins.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            return;
                        }
                        Err(MetaError::CasMismatch { .. }) => continue,
                        Err(e) => panic!("{e}"),
                    }
                }
                panic!("starved");
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let st = setup.read(|tx| tx.stream("main")).unwrap().unwrap().state;
    assert_eq!(wins.load(std::sync::atomic::Ordering::SeqCst), 24);
    assert_eq!(st.version, 24, "every successful CAS is counted exactly once");
    let log = setup.read(|tx| tx.reflog("main", 100)).unwrap();
    assert_eq!(log.len(), 24);
    // reflog is a contiguous chain: each entry's old_head is the previous new_head
    let mut chron: Vec<_> = log.into_iter().rev().collect();
    let mut prev = None;
    for e in chron.drain(..) {
        assert_eq!(e.old_head, prev);
        prev = e.new_head;
    }
}

#[test]
fn leases_and_queue_basics() {
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(1_000)));
    let m = TestMeta::new(clock.clone());
    let token = m.write(|tx| tx.next_counter("fencing")).unwrap();
    let lease = LeaseRow {
        id: "lease_1".into(),
        scope: "src/**".into(),
        holder: "agent:a".into(),
        exclusive: true,
        token,
        acquired_at: clock.now(),
        expires_at: Timestamp::from_millis(2_000),
        released_at: None,
        task_id: None,
    };
    m.write(|tx| tx.lease_insert(&lease)).unwrap();
    assert_eq!(m.read(|tx| tx.leases_active()).unwrap().len(), 1);
    clock.advance_ms(5_000);
    assert!(m.read(|tx| tx.leases_active()).unwrap().is_empty(), "expired leases are not active");
    assert_eq!(m.read(|tx| tx.leases_all(10)).unwrap().len(), 1);

    let (a, ca) = cs(vec![], "a");
    m.write(|tx| {
        tx.register_changeset(&a, &ca)?;
        tx.create_stream(&StreamName::new("main").unwrap(), "user:t")?;
        tx.proposal_create(&NewProposal {
            id: "prop_1",
            stream: "main",
            head: &a,
            title: "t",
            description: "d",
            author: "user:t",
            task_id: None,
        })?;
        tx.proposal_create(&NewProposal {
            id: "prop_2",
            stream: "main",
            head: &a,
            title: "t",
            description: "d",
            author: "user:t",
            task_id: None,
        })?;
        Ok(())
    })
    .unwrap();
    let q1 = m.write(|tx| tx.queue_push("main", "prop_1", "user:t")).unwrap();
    m.write(|tx| tx.queue_push("main", "prop_2", "user:t")).unwrap();
    assert!(m.write(|tx| tx.queue_push("main", "prop_1", "user:t")).is_err(), "no duplicates while queued");
    let queued = m.read(|tx| tx.queue_list("main", Some("queued"))).unwrap();
    assert_eq!(queued.iter().map(|q| q.proposal.as_str()).collect::<Vec<_>>(), ["prop_1", "prop_2"]);
    m.write(|tx| tx.queue_finish(q1.id, "done", "ok")).unwrap();
    assert_eq!(m.read(|tx| tx.queue_list("main", Some("queued"))).unwrap().len(), 1);
}

#[test]
fn agents_runs_tasks_provenance() {
    let m = meta();
    m.write(|tx| tx.agent_register("bot", "agent:bot", &"ab".repeat(32), "agent")).unwrap();
    assert!(m.write(|tx| tx.agent_register("bot", "agent:bot", "x", "agent")).is_err());
    assert_eq!(m.read(|tx| tx.agent_by_identity("agent:bot#run_1")).unwrap().unwrap().name, "bot");
    m.write(|tx| tx.run_start("run_1", "bot", None)).unwrap();
    m.write(|tx| tx.run_heartbeat("run_1")).unwrap();
    m.write(|tx| tx.run_finish("run_1", "completed", Some("done"))).unwrap();
    assert!(m.write(|tx| tx.run_heartbeat("run_1")).is_err(), "finished runs cannot heartbeat");

    m.write(|tx| {
        tx.task_create("t1", "one", "", &["src/**".into()], "user:t")?;
        tx.task_create("t2", "two", "", &[], "user:t")?;
        tx.task_dep_add("t2", "t1")
    })
    .unwrap();
    assert_eq!(m.read(|tx| tx.task_deps("t2")).unwrap(), vec!["t1".to_string()]);
    assert_eq!(m.read(|tx| tx.task("t1")).unwrap().unwrap().scope, vec!["src/**".to_string()]);

    let (a, ca) = cs(vec![], "a");
    m.write(|tx| tx.register_changeset(&a, &ca)).unwrap();
    m.write(|tx| tx.provenance_insert(&a, "bot", Some("run_1"), &json!({"model": "m"}), "pk", "sig")).unwrap();
    let p = m.read(|tx| tx.provenance_for(&a)).unwrap();
    assert_eq!(p[0].record["model"], "m");
}
