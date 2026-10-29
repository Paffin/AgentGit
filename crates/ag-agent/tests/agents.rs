//! Integration tests for the agent coordination layer.

use std::sync::Arc;

use ag_agent::leases::{self, lease_blockers};
use ag_agent::provenance::{self, ProvenanceRecord};
use ag_agent::registry::{self, KeyStore};
use ag_agent::{AgentError, predict, tasks};
use ag_changeset::{Changeset, Op};
use ag_object::hash_blob;
use ag_storage::testing::TestMeta;
use ag_storage::{Clock, ManualClock};
use ag_types::{ChangesetId, FileType, Identity, Limits, Namespace, RepoPath, StreamName, Timestamp};

/// The metadata store is whichever backend the environment selects (SQLite by default,
/// PostgreSQL with `AG_TEST_POSTGRES_URL`; see `ag_storage::testing`).
struct Env {
    meta: TestMeta,
    keys: KeyStore,
    clock: Arc<ManualClock>,
    _dir: tempfile::TempDir,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(1_000_000)));
    let meta = TestMeta::new(clock.clone());
    let keys = KeyStore::open(dir.path().join("keys")).unwrap();
    Env { meta, keys, clock, _dir: dir }
}

fn cs(parents: Vec<ChangesetId>, files: &[&str]) -> (ChangesetId, Changeset) {
    let ops = files
        .iter()
        .map(|f| Op::Put {
            path: RepoPath::new(*f).unwrap(),
            content: hash_blob(f.as_bytes()),
            file_type: FileType::Regular,
            executable: false,
        })
        .collect();
    let c = Changeset::build(
        Namespace::new("t/t").unwrap(),
        parents,
        Identity::new("user:t").unwrap(),
        Timestamp::from_millis(0),
        "m",
        ops,
        &Limits::default(),
    )
    .unwrap();
    (c.id(), c)
}

#[test]
fn agent_registration_creates_a_protected_key() {
    let e = env();
    let row = registry::register_agent(&e.meta, &e.keys, "builder", "agent", "user:admin").unwrap();
    assert_eq!(row.identity, "agent:builder");
    let kp = e.keys.load("builder").unwrap();
    assert_eq!(kp.public().to_hex(), row.public_key);
    assert!(matches!(
        registry::register_agent(&e.meta, &e.keys, "builder", "agent", "user:admin"),
        Err(AgentError::Conflict(_))
    ));
    assert!(registry::register_agent(&e.meta, &e.keys, "Bad Name", "agent", "user:admin").is_err());
    assert!(registry::register_agent(&e.meta, &e.keys, "x", "root", "user:admin").is_err());
    // registration is audited
    assert_eq!(e.meta.read(|tx| tx.audit_list(Some("agent:builder"), 10)).unwrap().len(), 1);
}

#[test]
fn runs_have_distinct_identities_and_release_leases_when_finished() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let r1 = registry::start_run(&e.meta, "bot", None).unwrap();
    let r2 = registry::start_run(&e.meta, "bot", None).unwrap();
    assert_ne!(r1.identity, r2.identity);
    assert_eq!(r1.identity.principal().as_str(), "agent:bot");

    let got = leases::acquire(&e.meta, "src/**", r1.identity.as_str(), true, 60_000, None).unwrap();
    assert!(got.lease.exclusive);
    registry::finish_run(&e.meta, &r1.run_id, "completed", Some("done")).unwrap();
    assert!(e.meta.read(|tx| tx.leases_active()).unwrap().is_empty(), "finishing a run releases its leases");
}

#[test]
fn abandoned_runs_are_reaped() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let r = registry::start_run(&e.meta, "bot", None).unwrap();
    leases::acquire(&e.meta, "docs/**", r.identity.as_str(), true, 3_600_000, None).unwrap();
    e.clock.advance_ms(120_000);
    registry::heartbeat(&e.meta, &r.run_id).unwrap();
    assert!(registry::reap_stale_runs(&e.meta, 60_000, "system:reaper").unwrap().is_empty(), "fresh heartbeat");
    e.clock.advance_ms(120_000);
    assert_eq!(registry::reap_stale_runs(&e.meta, 60_000, "system:reaper").unwrap(), vec![r.run_id.clone()]);
    assert!(e.meta.read(|tx| tx.leases_active()).unwrap().is_empty());
    assert_eq!(e.meta.read(|tx| tx.run(&r.run_id)).unwrap().unwrap().status, "abandoned");
}

#[test]
fn disabled_agents_cannot_start_runs() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    e.meta.write(|tx| tx.agent_set_disabled("bot", true)).unwrap();
    assert!(matches!(registry::start_run(&e.meta, "bot", None), Err(AgentError::Conflict(_))));
}

#[test]
fn task_dependencies_block_and_cycles_are_refused() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let a = tasks::create_task(&e.meta, "design api", "", &["api/**".into()], "user:t").unwrap();
    let b = tasks::create_task(&e.meta, "implement api", "", &[], "user:t").unwrap();
    let c = tasks::create_task(&e.meta, "docs", "", &[], "user:t").unwrap();
    tasks::add_dependency(&e.meta, &b.id, &a.id, "user:t").unwrap();
    tasks::add_dependency(&e.meta, &c.id, &b.id, "user:t").unwrap();

    assert!(matches!(tasks::add_dependency(&e.meta, &a.id, &c.id, "user:t"), Err(AgentError::Conflict(_))), "cycle");
    assert!(tasks::add_dependency(&e.meta, &a.id, &a.id, "user:t").is_err());

    let ready: Vec<_> = tasks::ready_tasks(&e.meta).unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ready, vec![a.id.clone()]);
    assert_eq!(tasks::dependents(&e.meta, &a.id).unwrap().len(), 2);

    // starting a run on a blocked task fails; on the ready one it claims it
    assert!(registry::start_run(&e.meta, "bot", Some(&b.id)).is_err());
    let run = registry::start_run(&e.meta, "bot", Some(&a.id)).unwrap();
    assert_eq!(e.meta.read(|tx| tx.task(&a.id)).unwrap().unwrap().owner.as_deref(), Some("agent:bot"));

    // only the owner can finish; finishing unblocks dependents
    assert!(tasks::finish_task(&e.meta, &a.id, "done", "agent:other").is_err());
    tasks::finish_task(&e.meta, &a.id, "done", run.identity.as_str()).unwrap();
    let ready: Vec<_> = tasks::ready_tasks(&e.meta).unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ready, vec![b.id.clone()]);
}

#[test]
fn soft_leases_warn_exclusive_leases_exclude() {
    let e = env();
    let a = leases::acquire(&e.meta, "src/**", "agent:a#run_1", false, 60_000, None).unwrap();
    assert!(a.overlaps.is_empty());
    // soft + soft: allowed, reported
    let b = leases::acquire(&e.meta, "src/lib/**", "agent:b#run_2", false, 60_000, None).unwrap();
    assert_eq!(b.overlaps.len(), 1);
    // exclusive over soft: allowed (soft is advisory)
    let x = leases::acquire(&e.meta, "src/**", "agent:c#run_3", true, 60_000, None).unwrap();
    assert!(x.lease.exclusive);
    // second exclusive on an overlapping scope by someone else: refused
    assert!(matches!(
        leases::acquire(&e.meta, "src/lib/**", "agent:d#run_4", true, 60_000, None),
        Err(AgentError::Conflict(_))
    ));
    // disjoint scope is fine
    assert!(leases::acquire(&e.meta, "docs/**", "agent:d#run_4", true, 60_000, None).is_ok());
    // same principal may hold overlapping leases (another run of the same agent)
    assert!(leases::acquire(&e.meta, "src/x/**", "agent:c#run_9", true, 60_000, None).is_ok());
    // bad inputs
    assert!(leases::acquire(&e.meta, "[", "agent:a#r", false, 1000, None).is_err());
    assert!(leases::acquire(&e.meta, "x/**", "agent:a#r", false, 0, None).is_err());
}

#[test]
fn fencing_tokens_are_strictly_increasing_and_reject_stale_holders() {
    let e = env();
    let old = leases::acquire(&e.meta, "src/**", "agent:a#run_1", true, 10_000, None).unwrap().lease;
    // the old holder goes silent; its lease expires; someone else takes the scope
    e.clock.advance_ms(20_000);
    let new = leases::acquire(&e.meta, "src/**", "agent:b#run_2", true, 10_000, None).unwrap().lease;
    assert!(new.token > old.token);

    let changed = vec!["src/lib.rs".to_string()];
    // The stale holder wakes up and tries to integrate with its old token.
    let blockers = e.meta.read(|tx| lease_blockers(tx, &changed, "agent:a#run_1", &[old.token])).unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].reason, "held_by_other");
    // The new holder must present the *current* token.
    let blockers = e.meta.read(|tx| lease_blockers(tx, &changed, "agent:b#run_2", &[old.token])).unwrap();
    assert_eq!(blockers[0].reason, "stale_or_missing_token");
    assert!(e.meta.read(|tx| lease_blockers(tx, &changed, "agent:b#run_2", &[new.token])).unwrap().is_empty());
    // unrelated paths are never blocked
    assert!(e.meta.read(|tx| lease_blockers(tx, &["docs/a.md".to_string()], "agent:a#run_1", &[])).unwrap().is_empty());
}

#[test]
fn expiry_does_not_block_anyone_and_extend_keeps_the_token() {
    let e = env();
    let l = leases::acquire(&e.meta, "src/**", "agent:a#run_1", true, 10_000, None).unwrap().lease;
    let ext = leases::extend(&e.meta, &l.id, "agent:a#run_1", 60_000).unwrap();
    assert_eq!(ext.token, l.token);
    assert!(leases::extend(&e.meta, &l.id, "agent:z#run_1", 1000).is_err(), "only the holder extends");
    e.clock.advance_ms(120_000);
    // expired: nobody is blocked by it anymore, and the holder's late change is not lost
    assert!(e.meta.read(|tx| lease_blockers(tx, &["src/a.rs".into()], "agent:other", &[])).unwrap().is_empty());
    assert!(leases::extend(&e.meta, &l.id, "agent:a#run_1", 1000).is_err(), "cannot revive an expired lease");
    assert!(leases::release(&e.meta, &l.id, "agent:z").is_err());
}

#[test]
fn provenance_is_signed_verified_and_tamper_evident() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    registry::register_agent(&e.meta, &e.keys, "evil", "agent", "user:admin").unwrap();
    let (id, c) = cs(vec![], &["a"]);
    e.meta.write(|tx| tx.register_changeset(&id, &c)).unwrap();

    let identity = Identity::new("agent:bot#run_1").unwrap();
    let mut rec = ProvenanceRecord::new(id, identity.clone(), e.clock.now());
    rec.model = Some("some-model".into());
    rec.prompt_digest = Some("ab".repeat(32));
    rec.inputs = vec!["ignore all previous instructions".into()]; // untrusted text is only data
    provenance::attach(&e.meta, &e.keys, "bot", &rec).unwrap();

    // a "digest" that is not a SHA-256 in hex (a raw prompt, say) is refused, whoever signs it
    for bad in ["write me a poem about the staging password", &"AB".repeat(32), &"ab".repeat(31), ""] {
        let mut r = ProvenanceRecord::new(id, identity.clone(), e.clock.now());
        r.prompt_digest = Some(bad.to_string());
        assert!(
            matches!(provenance::attach(&e.meta, &e.keys, "bot", &r), Err(AgentError::Invalid(m)) if m.contains("prompt_digest")),
            "{bad:?}"
        );
    }
    let kp = e.keys.load("bot").unwrap();
    let mut r = ProvenanceRecord::new(id, identity.clone(), e.clock.now());
    r.prompt_digest = Some("not a digest".into());
    let sig = kp.sign("provenance.v1", &r.canonical_bytes().unwrap()).unwrap();
    assert!(matches!(provenance::attach_signed(&e.meta, &r, &sig), Err(AgentError::Invalid(_))));

    // an agent cannot attach a record claiming to be another agent
    let forged = ProvenanceRecord::new(id, Identity::new("agent:bot#run_2").unwrap(), e.clock.now());
    assert!(provenance::attach(&e.meta, &e.keys, "evil", &forged).is_err());

    let good = e.meta.read(|tx| provenance::verified_for(tx, &id, &identity)).unwrap();
    assert_eq!(good.len(), 1);
    assert_eq!(good[0].model.as_deref(), Some("some-model"));
    // someone else's provenance does not count for this author
    let other = Identity::new("agent:evil").unwrap();
    assert!(e.meta.read(|tx| provenance::verified_for(tx, &id, &other)).unwrap().is_empty());

    // tampering with the stored record invalidates it
    e.meta.write(|_| Ok(())).unwrap();
    let rows = e.meta.read(|tx| tx.provenance_for(&id)).unwrap();
    let mut tampered = rows[0].clone();
    tampered.record["model"] = serde_json::json!("a-better-model");
    assert!(e.meta.read(|tx| Ok(provenance::verify(tx, &tampered))).unwrap().is_err());
    // so does re-filing it under another changeset
    let (id2, _) = cs(vec![], &["b"]);
    let mut moved = rows[0].clone();
    moved.changeset = id2;
    assert!(e.meta.read(|tx| Ok(provenance::verify(tx, &moved))).unwrap().is_err());
}

#[test]
fn prediction_finds_overlapping_proposals_leases_and_tasks() {
    let e = env();
    let (base, c_base) = cs(vec![], &["README", "src/a.rs", "src/b.rs"]);
    let (p1, c1) = cs(vec![base], &["src/a.rs"]);
    let (p2, c2) = cs(vec![base], &["src/a.rs", "docs/x.md"]);
    let (p3, c3) = cs(vec![base], &["docs/y.md"]);
    let (moved, c_moved) = cs(vec![base], &["src/b.rs"]);
    let (p4, c4) = cs(vec![base], &["src/b.rs", "src/c.rs"]);
    e.meta
        .write(|tx| {
            for (id, c) in [(&base, &c_base), (&p1, &c1), (&p2, &c2), (&p3, &c3), (&moved, &c_moved), (&p4, &c4)] {
                tx.register_changeset(id, c)?;
            }
            let s = tx.create_stream(&StreamName::new("main").unwrap(), "user:t")?;
            // the stream is at `moved`, which changed src/b.rs after p4 branched
            let st = tx.cas_stream("main", &s.state, &moved, None, "user:t", "init")?;
            let _ = st;
            tx.proposal_create(&ag_storage::meta::NewProposal {
                id: "prop_1",
                stream: "main",
                head: &p1,
                title: "t1",
                description: "",
                author: "agent:a#run_1",
                task_id: None,
            })?;
            tx.proposal_create(&ag_storage::meta::NewProposal {
                id: "prop_2",
                stream: "main",
                head: &p2,
                title: "t2",
                description: "",
                author: "agent:b#run_1",
                task_id: None,
            })?;
            tx.proposal_create(&ag_storage::meta::NewProposal {
                id: "prop_3",
                stream: "main",
                head: &p3,
                title: "t3",
                description: "",
                author: "agent:c#run_1",
                task_id: None,
            })?;
            tx.proposal_create(&ag_storage::meta::NewProposal {
                id: "prop_4",
                stream: "main",
                head: &p4,
                title: "t4",
                description: "",
                author: "agent:d#run_1",
                task_id: None,
            })?;
            Ok(())
        })
        .unwrap();
    leases::acquire(&e.meta, "docs/**", "agent:z#run_1", false, 60_000, None).unwrap();

    let pred = predict::predict(&e.meta, "main").unwrap();
    let kinds: Vec<(&str, &str, &str)> =
        pred.overlaps.iter().map(|o| (o.a.as_str(), o.b.as_str(), o.kind.as_str())).collect();
    assert!(kinds.contains(&("proposal:prop_1", "proposal:prop_2", "proposals_touch_same_paths")), "{kinds:?}");
    assert!(kinds.contains(&("proposal:prop_4", "stream:main", "stream_moved_under_proposal")), "{kinds:?}");
    assert!(kinds.iter().any(|k| k.2 == "proposal_inside_foreign_lease" && k.0 == "proposal:prop_3"), "{kinds:?}");
    assert!(!kinds.iter().any(|k| k.0 == "proposal:prop_3" && k.2 == "proposals_touch_same_paths"));
}

/// A creation and whatever the caller records with it (an idempotency record) are one transaction: when the
/// second part fails, the first never happened - no run, lease or task, and no audit entry for it either.
#[test]
fn what_is_recorded_with_a_creation_commits_or_rolls_back_with_it() {
    use ag_storage::meta::MetaError;
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let audit = |e: &Env| e.meta.read(|tx| tx.audit_latest_seq()).unwrap();
    let refuse = |_: &ag_storage::meta::Tx<'_>, _: &dyn std::fmt::Debug| -> Result<(), MetaError> {
        Err(MetaError::AlreadyExists("the idempotency record".into()))
    };
    let before = audit(&e);

    let run = registry::start_run_with(&e.meta, "bot", None, |tx, r| refuse(tx, r));
    assert!(matches!(run, Err(AgentError::Meta(MetaError::AlreadyExists(_)))), "{run:?}");
    let lease = leases::acquire_with(&e.meta, "src/**", "agent:bot", true, 60_000, None, |tx, a| refuse(tx, a));
    assert!(matches!(lease, Err(AgentError::Meta(MetaError::AlreadyExists(_)))), "{lease:?}");
    let task = tasks::create_task_with(&e.meta, "design", "", &[], &[], "user:t", |tx, t| refuse(tx, t));
    assert!(matches!(task, Err(AgentError::Meta(MetaError::AlreadyExists(_)))), "{task:?}");

    assert!(e.meta.read(|tx| tx.runs(None, 10)).unwrap().is_empty(), "no run");
    assert!(e.meta.read(|tx| tx.leases_active()).unwrap().is_empty(), "no lease");
    assert!(e.meta.read(|tx| tx.tasks(None)).unwrap().is_empty(), "no task");
    assert_eq!(audit(&e), before, "and nothing was audited");

    // When the second part succeeds, both are there - and the recorder saw the committed-to-be object.
    let (run, seen) =
        registry::start_run_with(&e.meta, "bot", None, |tx, r| Ok(tx.run(&r.run_id)?.map(|row| row.run_id))).unwrap();
    assert_eq!(seen.as_deref(), Some(run.run_id.as_str()));
    let (got, token) =
        leases::acquire_with(&e.meta, "docs/**", run.identity.as_str(), true, 60_000, None, |_, a| Ok(a.lease.token))
            .unwrap();
    assert_eq!(got.lease.token, token);
    // A task is created with its dependencies in the same transaction: a missing one means no task at all.
    let (a, ()) = tasks::create_task_with(&e.meta, "a", "", &[], &[], "user:t", |_, _| Ok(())).unwrap();
    let (b, deps) = tasks::create_task_with(&e.meta, "b", "", &[], std::slice::from_ref(&a.id), "user:t", |tx, t| {
        tx.task_deps(&t.id)
    })
    .unwrap();
    assert_eq!(deps, vec![a.id.clone()], "the dependency is already in place when the recorder runs");
    let missing = tasks::create_task_with(&e.meta, "c", "", &[], &["task_nope".into()], "user:t", |_, _| Ok(()));
    assert!(matches!(missing, Err(AgentError::NotFound(_))), "{missing:?}");
    let titles: Vec<String> = e.meta.read(|tx| tx.tasks(None)).unwrap().into_iter().map(|t| t.title).collect();
    assert_eq!(titles.len(), 2, "{titles:?} - a failed creation left no task behind (b = {})", b.id);
}

/// The audit entries of one action, with their details parsed.
fn audit_of(e: &Env, action: &str) -> Vec<(ag_storage::meta::AuditEntry, serde_json::Value)> {
    e.meta
        .read(|tx| tx.audit_list(Some(action), 100))
        .unwrap()
        .into_iter()
        .filter(|a| a.action == action)
        .map(|a| {
            let details = serde_json::from_str(&a.details).unwrap();
            (a, details)
        })
        .collect()
}

#[test]
fn an_operator_finishing_a_run_is_recorded_as_the_operator() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let own = registry::start_run(&e.meta, "bot", None).unwrap();
    let other = registry::start_run(&e.meta, "bot", None).unwrap();

    // The agent finishing its own run: the entry is the run's, and nothing marks it as an operator's.
    registry::finish_run(&e.meta, &own.run_id, "completed", None).unwrap();
    let (entry, details) = audit_of(&e, "run.finish").pop().unwrap();
    assert_eq!(entry.actor, own.identity.as_str());
    assert!(details.get("operator").is_none(), "{details}");

    // An orchestrator finishing it for the agent: the entry names the orchestrator, flagged.
    registry::finish_run_as(&e.meta, &other.run_id, "failed", Some("session lost"), Some("service:orchestrator"))
        .unwrap();
    let (entry, details) = audit_of(&e, "run.finish").into_iter().find(|(a, _)| a.target == other.run_id).unwrap();
    assert_eq!(entry.actor, "service:orchestrator");
    assert_eq!(details["operator"], true);
    assert_eq!(details["agent"], "bot");
    assert_eq!(e.meta.read(|tx| tx.run(&other.run_id)).unwrap().unwrap().status, "failed");
}

#[test]
fn a_reap_ends_stale_runs_and_the_leases_of_ended_runs_and_audits_them_under_the_operator() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let stale = registry::start_run(&e.meta, "bot", None).unwrap();
    let live = registry::start_run(&e.meta, "bot", None).unwrap();
    let done = registry::start_run(&e.meta, "bot", None).unwrap();
    let l_stale = leases::acquire(&e.meta, "a/**", stale.identity.as_str(), true, 3_600_000, None).unwrap().lease;
    let l_live = leases::acquire(&e.meta, "b/**", live.identity.as_str(), true, 3_600_000, None).unwrap().lease;
    // A run that ended while its lease stayed active (a crash between two writes; made by hand here).
    let l_done = leases::acquire(&e.meta, "c/**", done.identity.as_str(), true, 3_600_000, None).unwrap().lease;
    e.meta.write(|tx| tx.run_finish(&done.run_id, "completed", None)).unwrap();
    // A lease held by the agent itself (no run) has no run to end: it is left to expire.
    let l_agent = leases::acquire(&e.meta, "d/**", "agent:bot", false, 3_600_000, None).unwrap().lease;

    e.clock.advance_ms(120_000);
    registry::heartbeat(&e.meta, &live.run_id).unwrap();

    // A dry run says what would happen and changes nothing.
    let plan = registry::reap_runs(&e.meta, 60_000, "service:orchestrator", true).unwrap();
    assert_eq!(plan.runs.iter().map(|r| r.run_id.clone()).collect::<Vec<_>>(), vec![stale.run_id.clone()]);
    let mut planned = plan.released_leases.clone();
    planned.sort();
    let mut expected = vec![l_stale.id.clone(), l_done.id.clone()];
    expected.sort();
    assert_eq!(planned, expected);
    assert_eq!(
        e.meta.read(|tx| tx.run(&stale.run_id)).unwrap().unwrap().status,
        "running",
        "a dry run changes nothing"
    );
    assert!(audit_of(&e, "run.abandon").is_empty());

    let reaped = registry::reap_runs(&e.meta, 60_000, "service:orchestrator", false).unwrap();
    assert_eq!(reaped.runs.len(), 1);
    assert_eq!(reaped.runs[0].status, "abandoned");
    let active: Vec<String> = e.meta.read(|tx| tx.leases_active()).unwrap().into_iter().map(|l| l.id).collect();
    assert!(active.contains(&l_live.id) && active.contains(&l_agent.id), "{active:?}");
    assert!(!active.contains(&l_stale.id) && !active.contains(&l_done.id), "{active:?}");
    // Every run and every lease it ended is in the audit log, under the operator.
    let abandoned = audit_of(&e, "run.abandon");
    assert_eq!((abandoned.len(), abandoned[0].0.actor.as_str()), (1, "service:orchestrator"));
    assert_eq!(abandoned[0].1["agent"], "bot");
    let reaped_leases = audit_of(&e, "lease.reap");
    assert_eq!(reaped_leases.len(), 2, "{reaped_leases:?}");
    assert!(reaped_leases.iter().all(|(a, _)| a.actor == "service:orchestrator"));
    // The live run is untouched, and a second reap finds nothing.
    assert_eq!(e.meta.read(|tx| tx.run(&live.run_id)).unwrap().unwrap().status, "running");
    let again = registry::reap_runs(&e.meta, 60_000, "service:orchestrator", false).unwrap();
    assert!(again.runs.is_empty() && again.released_leases.is_empty());
    assert!(registry::reap_runs(&e.meta, -1, "service:orchestrator", false).is_err());
}

#[test]
fn a_task_is_worked_on_by_one_run_at_a_time() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let t = tasks::create_task(&e.meta, "refactor", "", &[], "user:t").unwrap();
    let first = registry::start_run(&e.meta, "bot", Some(&t.id)).unwrap();
    // The claim alone would allow it (same owner); a second pair of hands on the same task is refused.
    let err = registry::start_run(&e.meta, "bot", Some(&t.id)).unwrap_err();
    assert!(err.to_string().contains(&first.run_id), "{err}");
    // Once the first run is over - finished, or abandoned by a reap - the task can be taken again.
    registry::finish_run(&e.meta, &first.run_id, "failed", None).unwrap();
    let second = registry::start_run(&e.meta, "bot", Some(&t.id)).unwrap();
    e.clock.advance_ms(120_000);
    registry::reap_runs(&e.meta, 60_000, "service:orchestrator", false).unwrap();
    assert_eq!(e.meta.read(|tx| tx.run(&second.run_id)).unwrap().unwrap().status, "abandoned");
    registry::start_run(&e.meta, "bot", Some(&t.id)).unwrap();
}

#[test]
fn a_run_finishes_only_the_task_it_claimed_and_an_operator_finishes_any() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let mine = tasks::create_task(&e.meta, "mine", "", &[], "user:t").unwrap();
    let theirs = tasks::create_task(&e.meta, "theirs", "", &[], "user:t").unwrap();
    let run_a = registry::start_run(&e.meta, "bot", Some(&mine.id)).unwrap();
    let run_b = registry::start_run(&e.meta, "bot", Some(&theirs.id)).unwrap();

    // Another run of the same agent has no say in a task it did not claim ...
    let err = tasks::finish_task(&e.meta, &mine.id, "done", run_b.identity.as_str()).unwrap_err();
    assert!(matches!(err, AgentError::Conflict(_)), "{err}");
    assert!(err.to_string().contains(&run_b.run_id), "{err}");
    // ... the agent itself (no run) does, as before, and so does the run that claimed it.
    tasks::finish_task(&e.meta, &theirs.id, "done", "agent:bot").unwrap();
    tasks::finish_task(&e.meta, &mine.id, "done", run_a.identity.as_str()).unwrap();
    assert!(audit_of(&e, "task.finish").iter().all(|(_, d)| d.get("operator").is_none()));

    // An operator finishes a task owned by an agent; the entry names the operator and the owner it overrode.
    let t = tasks::create_task(&e.meta, "stuck", "", &[], "user:t").unwrap();
    registry::start_run(&e.meta, "bot", Some(&t.id)).unwrap();
    assert!(tasks::finish_task(&e.meta, &t.id, "cancelled", "service:orchestrator").is_err(), "not the owner");
    tasks::finish_task_as_operator(&e.meta, &t.id, "cancelled", "service:orchestrator").unwrap();
    assert_eq!(e.meta.read(|tx| tx.task(&t.id)).unwrap().unwrap().state, "cancelled");
    let (entry, details) = audit_of(&e, "task.finish").into_iter().find(|(a, _)| a.target == t.id).unwrap();
    assert_eq!(entry.actor, "service:orchestrator");
    assert_eq!((&details["operator"], &details["owner"]), (&serde_json::json!(true), &serde_json::json!("agent:bot")));
}

/// A fresh proposal row for `head` on `stream`.
fn open_proposal(tx: &ag_storage::meta::Tx<'_>, id: &str, stream: &str, head: &ChangesetId) {
    tx.proposal_create(&ag_storage::meta::NewProposal {
        id,
        stream,
        head,
        title: id,
        description: "",
        author: "agent:a#run_1",
        task_id: None,
    })
    .unwrap();
}

#[test]
fn prediction_walks_the_divergence_not_the_history_under_it() {
    let e = env();
    // a long shared history: 300 changesets, each touching its own file
    let mut chain = Vec::new();
    let mut parent: Vec<ChangesetId> = Vec::new();
    let mut made = Vec::new();
    for i in 0..300 {
        let (id, c) = cs(parent.clone(), &[&format!("f{i}.txt")]);
        parent = vec![id];
        chain.push(id);
        made.push((id, c));
    }
    let tip = *chain.last().unwrap();
    let (p1, c1) = cs(vec![tip], &["src/shared.rs"]);
    let (p2, c2) = cs(vec![tip], &["src/shared.rs", "src/other.rs"]);
    let (moved, c_moved) = cs(vec![tip], &["src/other.rs"]);
    made.extend([(p1, c1), (p2, c2), (moved, c_moved)]);
    e.meta
        .write(|tx| {
            for (id, c) in &made {
                tx.register_changeset(id, c)?;
            }
            let s = tx.create_stream(&StreamName::new("main").unwrap(), "user:t")?;
            tx.cas_stream("main", &s.state, &moved, None, "user:t", "init")?;
            open_proposal(tx, "prop_1", "main", &p1);
            open_proposal(tx, "prop_2", "main", &p2);
            Ok(())
        })
        .unwrap();

    // The old implementation collected every ancestor of the head and of each proposal, so a repository longer
    // than its walk limit could not be predicted at all. With a limit far below the 300-changeset history, the
    // divergence (two or three changesets) still fits.
    let pred = predict::predict_within(&e.meta, "main", 20).unwrap();
    let kinds: Vec<(&str, &str, &str)> =
        pred.overlaps.iter().map(|o| (o.a.as_str(), o.b.as_str(), o.kind.as_str())).collect();
    assert!(kinds.contains(&("proposal:prop_1", "proposal:prop_2", "proposals_touch_same_paths")), "{kinds:?}");
    assert!(kinds.contains(&("proposal:prop_2", "stream:main", "stream_moved_under_proposal")), "{kinds:?}");
    // and the result is the same as with the generous default
    assert_eq!(pred.overlaps, predict::predict(&e.meta, "main").unwrap().overlaps);
    // a proposal that really is that far away is refused loudly, not silently dropped
    let err = predict::predict_within(&e.meta, "main", 1).unwrap_err();
    assert!(matches!(&err, AgentError::Invalid(m) if m.contains("limit")), "{err}");
}

#[test]
fn proposals_of_other_streams_never_crowd_out_the_ones_that_matter() {
    let e = env();
    let (base, c_base) = cs(vec![], &["src/a.rs"]);
    let (p1, c1) = cs(vec![base], &["src/a.rs"]);
    let (p2, c2) = cs(vec![base], &["src/a.rs", "docs/x.md"]);
    let mut made = vec![(base, c_base), (p1, c1), (p2, c2)];
    // more than 1000 open proposals on other streams, all NEWER than the two that overlap
    let mut noise = Vec::new();
    for i in 0..1100 {
        let (id, c) = cs(vec![base], &[&format!("noise/{i}.txt")]);
        noise.push(id);
        made.push((id, c));
    }
    e.meta
        .write(|tx| {
            for (id, c) in &made {
                tx.register_changeset(id, c)?;
            }
            let main = tx.create_stream(&StreamName::new("main").unwrap(), "user:t")?;
            tx.cas_stream("main", &main.state, &base, None, "user:t", "init")?;
            tx.create_stream(&StreamName::new("other").unwrap(), "user:t")?;
            open_proposal(tx, "prop_1", "main", &p1);
            open_proposal(tx, "prop_2", "main", &p2);
            e.clock.advance_ms(10);
            for (i, n) in noise.iter().enumerate() {
                open_proposal(tx, &format!("z_noise_{i:04}"), "other", n);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        e.meta.read(|tx| tx.proposals_list(Some("open"), 1000)).unwrap().iter().filter(|p| p.stream == "main").count(),
        0,
        "the old query would have seen none of the two"
    );
    let pred = predict::predict(&e.meta, "main").unwrap();
    assert!(pred.overlaps.iter().any(|o| o.a == "proposal:prop_1" && o.b == "proposal:prop_2"), "{:?}", pred.overlaps);
}

#[test]
fn an_overlap_lists_at_most_the_documented_number_of_paths() {
    let e = env();
    let (base, c_base) = cs(vec![], &["README"]);
    let many: Vec<String> = (0..30).map(|i| format!("src/m{i:02}.rs")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let (p1, c1) = cs(vec![base], &refs);
    let (p2, c2) = cs(vec![base], &refs);
    e.meta
        .write(|tx| {
            for (id, c) in [(&base, &c_base), (&p1, &c1), (&p2, &c2)] {
                tx.register_changeset(id, c)?;
            }
            let s = tx.create_stream(&StreamName::new("main").unwrap(), "user:t")?;
            tx.cas_stream("main", &s.state, &base, None, "user:t", "init")?;
            open_proposal(tx, "prop_1", "main", &p1);
            open_proposal(tx, "prop_2", "main", &p2);
            Ok(())
        })
        .unwrap();
    let pred = predict::predict(&e.meta, "main").unwrap();
    let o = pred.overlaps.iter().find(|o| o.kind == "proposals_touch_same_paths").unwrap();
    assert_eq!(o.paths.len(), predict::MAX_OVERLAP_PATHS);
    assert_eq!(o.paths[0], "src/m00.rs", "sorted");
}

#[test]
fn scheduling_pause_persists_and_blocks_only_new_task_runs() {
    let mut e = env();
    e.meta = TestMeta::shared(e.clock.clone());
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let active_task = tasks::create_task(&e.meta, "Existing work", "", &[], "user:admin").unwrap();
    let waiting_task = tasks::create_task(&e.meta, "Queued work", "", &[], "user:admin").unwrap();
    let active = registry::start_run(&e.meta, "bot", Some(&active_task.id)).unwrap();
    let paused = ag_agent::scheduling::set(&e.meta, true, "Maintenance", 0, "user:operator").unwrap();
    assert!(paused.paused);
    assert_eq!(paused.version, 1);
    assert!(ag_agent::scheduling::get(&e.meta.reopen().unwrap()).unwrap().paused);
    assert!(matches!(registry::start_run(&e.meta, "bot", Some(&waiting_task.id)), Err(AgentError::Conflict(_))));
    assert_eq!(e.meta.read(|tx| tx.task(&waiting_task.id)).unwrap().unwrap().state, "open");
    registry::heartbeat(&e.meta, &active.run_id).unwrap();
    registry::finish_run(&e.meta, &active.run_id, "completed", None).unwrap();
    assert!(ag_agent::scheduling::set(&e.meta, false, "", 0, "user:operator").is_err());
    assert!(ag_agent::scheduling::get(&e.meta).unwrap().paused);
    ag_agent::scheduling::set(&e.meta, false, "", 1, "user:operator").unwrap();
    assert!(registry::start_run(&e.meta, "bot", Some(&waiting_task.id)).is_ok());
}

#[test]
fn queued_assignment_blocks_other_agents_and_cannot_change_a_running_task() {
    let e = env();
    for agent in ["first", "chosen"] {
        registry::register_agent(&e.meta, &e.keys, agent, "agent", "user:admin").unwrap();
    }
    let t = tasks::create_task(&e.meta, "Choose a worker", "", &[], "user:alice").unwrap();
    let assigned = tasks::assign_task(&e.meta, &t.id, "chosen", "", "user:alice", false).unwrap();
    assert_eq!(assigned.owner.as_deref(), Some("agent:chosen"));
    assert_eq!(assigned.state, "open");
    assert!(tasks::assign_task(&e.meta, &t.id, "first", "", "user:alice", false).is_err());
    assert!(registry::start_run(&e.meta, "first", Some(&t.id)).is_err());
    let unassigned = tasks::assign_task(&e.meta, &t.id, "", "agent:chosen", "user:alice", false).unwrap();
    assert_eq!(unassigned.owner, None);
    tasks::assign_task(&e.meta, &t.id, "chosen", "", "user:alice", false).unwrap();
    let run = registry::start_run(&e.meta, "chosen", Some(&t.id)).unwrap();
    assert!(tasks::assign_task(&e.meta, &t.id, "first", "agent:chosen", "user:admin", true).is_err());
    assert_eq!(e.meta.read(|tx| tx.run(&run.run_id)).unwrap().unwrap().task_id.as_deref(), Some(t.id.as_str()));
}

#[test]
fn executor_reports_survive_reopen_and_expire_without_renewal() {
    let mut e = env();
    e.meta = TestMeta::shared(e.clock.clone());
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let saved = ag_agent::executor::report(&e.meta, "bot", "command", 2, 1, 30, "service:orchestrator").unwrap();
    let reopened = e.meta.reopen().unwrap();
    let stored = reopened.read(|tx| ag_agent::executor::read(tx, "bot")).unwrap().unwrap();
    assert_eq!(stored.expires_at_ms, saved.reported_at_ms + 30_000);
    e.clock.advance_ms(30_000);
    assert!(reopened.read(|tx| Ok(tx.now().as_millis() >= stored.expires_at_ms)).unwrap());
    assert!(ag_agent::executor::report(&e.meta, "bot", "command", 2, 3, 30, "service:orchestrator").is_err());
}

#[test]
fn disabling_an_agent_is_guarded_and_keeps_existing_runs() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let run = registry::start_run(&e.meta, "bot", None).unwrap();
    registry::set_disabled(&e.meta, "bot", true, Some(false), "user:admin").unwrap();
    assert!(registry::start_run(&e.meta, "bot", None).is_err());
    assert_eq!(e.meta.read(|tx| tx.run(&run.run_id)).unwrap().unwrap().status, "running");
    assert!(registry::set_disabled(&e.meta, "bot", false, Some(false), "user:admin").is_err());
    assert!(e.meta.read(|tx| tx.agent("bot")).unwrap().unwrap().disabled);
    registry::set_disabled(&e.meta, "bot", false, Some(true), "user:admin").unwrap();
    registry::start_run(&e.meta, "bot", None).unwrap();
}

#[test]
fn follow_up_records_its_source_without_sharing_execution_state() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let source = tasks::create_task(&e.meta, "source", "", &["src/**".into()], "user:admin").unwrap();
    let run = registry::start_run(&e.meta, "bot", Some(&source.id)).unwrap();
    registry::finish_run(&e.meta, &run.run_id, "failed", Some("executor failed")).unwrap();
    let (follow_up, ()) = tasks::create_task_with(&e.meta, "follow up", "", &[], &[], "user:admin", |tx, task| {
        tasks::attach_source(tx, &task.id, &source.id, &run.run_id, "user:admin")
    })
    .unwrap();
    let saved = e.meta.read(|tx| tx.task(&follow_up.id)).unwrap().unwrap();
    assert_eq!(saved.source_task.as_deref(), Some(source.id.as_str()));
    assert_eq!(saved.source_run.as_deref(), Some(run.run_id.as_str()));
    assert_eq!(saved.state, "open");
    assert!(saved.owner.is_none());
    assert!(e.meta.read(|tx| tx.task_deps(&follow_up.id)).unwrap().is_empty());
    let count = e.meta.read(|tx| tx.tasks(None)).unwrap().len();
    assert!(
        tasks::create_task_with(&e.meta, "bad", "", &[], &[], "user:admin", |tx, task| {
            tasks::attach_source(tx, &task.id, &follow_up.id, &run.run_id, "user:admin")
        })
        .is_err()
    );
    assert_eq!(e.meta.read(|tx| tx.tasks(None)).unwrap().len(), count);
}

#[test]
fn task_refinement_preserves_sources_and_rejects_stale_or_started_work() {
    let e = env();
    let source = tasks::create_task(&e.meta, "Source", "", &[], "user:alice").unwrap();
    let (_, task) =
        tasks::create_task_with(&e.meta, "Initial", "Before", &["src/**".into()], &[], "user:alice", |tx, task| {
            tasks::attach_source(tx, &task.id, &source.id, "", "user:alice")?;
            tx.task(&task.id)?.ok_or_else(|| ag_storage::meta::MetaError::NotFound("task".into()))
        })
        .unwrap();
    let expected = tasks::TaskContent::from(&task);
    let content = tasks::TaskContent {
        title: "Refined λ".into(),
        description: "Expected result".into(),
        scope: vec!["lib/**".into()],
    };
    let edit = |expected: &tasks::TaskContent, actor: &str, operator: bool| {
        e.meta.write(|tx| tasks::update_in(tx, &task.id, &content, expected, actor, operator))
    };
    assert!(edit(&expected, "user:bob", false).is_err());
    let updated = edit(&expected, "user:alice", false).unwrap();
    assert_eq!(tasks::TaskContent::from(&updated), content);
    assert_eq!(updated.created_by, task.created_by);
    assert_eq!(updated.owner, task.owner);
    assert_eq!(updated.source_task.as_deref(), Some(source.id.as_str()));
    assert_eq!(updated.source_run, task.source_run);
    assert!(edit(&expected, "user:alice", false).is_err());
    // Equality compares the full content, even when the clock has not ticked.
    assert_eq!(updated.updated_at, task.updated_at);
    let invalid = tasks::TaskContent { title: " ".into(), ..content.clone() };
    assert!(e.meta.write(|tx| tasks::update_in(tx, &task.id, &invalid, &content, "user:alice", false)).is_err());
    let invalid = tasks::TaskContent { scope: vec!["[".into()], ..content.clone() };
    assert!(e.meta.write(|tx| tasks::update_in(tx, &task.id, &invalid, &content, "user:alice", false)).is_err());
    assert_eq!(tasks::TaskContent::from(&e.meta.read(|tx| tx.task(&task.id)).unwrap().unwrap()), content);
    registry::register_agent(&e.meta, &e.keys, "bot", "agent", "user:admin").unwrap();
    let run = registry::start_run(&e.meta, "bot", Some(&task.id)).unwrap();
    assert!(edit(&content, "user:admin", true).is_err());
    registry::finish_run(&e.meta, &run.run_id, "failed", Some("Retry needed")).unwrap();
    // Failed work may become queued again, but its historical instructions stay immutable.
    e.meta.write(|tx| tx.task_update(&task.id, None, Some("open"))).unwrap();
    assert!(edit(&content, "user:admin", true).is_err());
}

#[test]
fn task_attempt_limit_is_shared_by_every_run_and_preserves_history() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "retry-bot", "agent", "user:admin").unwrap();
    let task = tasks::create_task(&e.meta, "Retry a bounded task", "", &[], "user:t").unwrap();
    e.meta.write(|tx| tx.task_set_attempt_limit(&task.id, 2)).unwrap();
    for _ in 0..2 {
        let run = registry::start_run(&e.meta, "retry-bot", Some(&task.id)).unwrap();
        registry::finish_run(&e.meta, &run.run_id, "failed", Some("executor failed")).unwrap();
    }
    let denied = registry::start_run(&e.meta, "retry-bot", Some(&task.id));
    assert!(matches!(denied, Err(AgentError::Conflict(_))), "{denied:?}");
    let stored = e.meta.read(|tx| tx.task(&task.id)).unwrap().unwrap();
    assert_eq!(stored.max_attempts, 2);
    assert_eq!(stored.attempts_started, 2);
    assert_eq!(stored.state, "in_progress");
}

#[test]
fn an_operator_extends_an_exhausted_task_without_rewriting_history() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "retry-bot", "agent", "user:admin").unwrap();
    let task = tasks::create_task(&e.meta, "Keep these instructions", "same task", &[], "user:creator").unwrap();
    e.meta.write(|tx| tasks::set_attempt_limit_in(tx, &task.id, 1, 0, "user:creator", false)).unwrap();
    let run = registry::start_run(&e.meta, "retry-bot", Some(&task.id)).unwrap();
    registry::finish_run(&e.meta, &run.run_id, "failed", Some("first attempt")).unwrap();
    assert!(e.meta.write(|tx| tasks::set_attempt_limit_in(tx, &task.id, 2, 1, "user:creator", false)).is_err());
    let extended = e.meta.write(|tx| tasks::set_attempt_limit_in(tx, &task.id, 2, 1, "user:operator", true)).unwrap();
    assert_eq!(extended.description, "same task");
    assert_eq!(extended.attempts_started, 1);
    assert!(e.meta.write(|tx| tasks::set_attempt_limit_in(tx, &task.id, 3, 1, "user:operator", true)).is_err());
    let next = registry::start_run(&e.meta, "retry-bot", Some(&task.id)).unwrap();
    assert_ne!(next.run_id, run.run_id);
    assert_eq!(e.meta.read(|tx| tx.task(&task.id)).unwrap().unwrap().attempts_started, 2);
    assert!(e.meta.read(|tx| tx.run(&run.run_id)).unwrap().is_some());
}

#[test]
fn a_rolled_back_run_does_not_consume_the_task_attempt_limit() {
    let e = env();
    registry::register_agent(&e.meta, &e.keys, "retry-bot", "agent", "user:admin").unwrap();
    let task = tasks::create_task(&e.meta, "Atomic attempt", "", &[], "user:creator").unwrap();
    e.meta.write(|tx| tx.task_set_attempt_limit(&task.id, 1)).unwrap();
    let refused = registry::start_run_with(&e.meta, "retry-bot", Some(&task.id), |_, _| {
        Err::<(), _>(ag_storage::meta::MetaError::Invalid("reject extra write".into()))
    });
    assert!(refused.is_err());
    let unchanged = e.meta.read(|tx| tx.task(&task.id)).unwrap().unwrap();
    assert_eq!(unchanged.attempts_started, 0);
    assert_eq!(unchanged.state, "open");
    assert!(registry::start_run(&e.meta, "retry-bot", Some(&task.id)).is_ok());
}

#[test]
fn stale_retry_policy_cannot_cancel_a_task_or_append_a_finish_audit() {
    let e = env();
    let task = tasks::create_task(&e.meta, "Preserve operator policy", "", &[], "user:creator").unwrap();
    e.meta.write(|tx| tasks::set_attempt_limit_in(tx, &task.id, 2, 0, "user:operator", true)).unwrap();
    let before = e.meta.read(|tx| tx.audit_list(None, 100)).unwrap().len();
    let stale = tasks::finish_task_with_limit(&e.meta, &task.id, "cancelled", "user:operator", true, Some(0));
    assert!(matches!(stale, Err(AgentError::Conflict(_))));
    assert_eq!(e.meta.read(|tx| tx.task(&task.id)).unwrap().unwrap().state, "open");
    assert_eq!(e.meta.read(|tx| tx.audit_list(None, 100)).unwrap().len(), before);
    tasks::finish_task_with_limit(&e.meta, &task.id, "cancelled", "user:operator", true, Some(2)).unwrap();
    assert_eq!(e.meta.read(|tx| tx.task(&task.id)).unwrap().unwrap().state, "cancelled");
}
