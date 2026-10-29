//! A proposal can enter only one terminal state under concurrent callers.
#![allow(missing_docs)]
use ag_integrate::{InitOptions, IntegrateRequest, Outcome, Repository};
use ag_storage::Clock;
use ag_types::{Identity, Namespace, StreamName, Timestamp};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

struct PauseClose {
    reached: mpsc::Sender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
    calls: AtomicUsize,
}
impl Clock for PauseClose {
    fn now(&self) -> Timestamp {
        if std::thread::current().name() == Some("closer") && self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.reached.send(()).unwrap();
            self.resume.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        }
        Timestamp::now()
    }
}

#[test]
fn a_delayed_close_cannot_replace_integration_or_duplicate_a_close() {
    for integrate in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let author = Identity::new("user:author").unwrap();
        let admin = Identity::new("user:admin").unwrap();
        let repo = Repository::init(
            dir.path(),
            &InitOptions::new(
                Namespace::new("acme/terminal").unwrap(),
                StreamName::new("main").unwrap(),
                author.clone(),
            ),
        )
        .unwrap();
        let head = repo.commit(vec![], &author, "root", vec![]).unwrap();
        let proposal = repo.propose("main", &head, "change", "", &author, None).unwrap();
        let (reached_tx, reached) = mpsc::channel();
        let (resume, wait) = mpsc::channel();
        let slow = Repository::open_with_clock(
            dir.path(),
            Arc::new(PauseClose { reached: reached_tx, resume: Mutex::new(wait), calls: AtomicUsize::new(0) }),
        )
        .unwrap();
        let id = proposal.id.clone();
        let actor = admin.clone();
        let thread =
            std::thread::Builder::new().name("closer".into()).spawn(move || slow.close_proposal(&id, &actor)).unwrap();
        reached.recv_timeout(Duration::from_secs(10)).unwrap();
        if integrate {
            let result = repo
                .integrate(&IntegrateRequest {
                    proposal_id: proposal.id.clone(),
                    actor: admin,
                    idempotency_key: None,
                    fencing_tokens: vec![],
                })
                .unwrap();
            assert!(matches!(result.outcome, Outcome::Integrated { .. }));
        } else {
            repo.close_proposal(&proposal.id, &admin).unwrap();
        }
        resume.send(()).unwrap();
        let delayed = thread.join().unwrap();
        assert!(delayed.is_err(), "a stale close must not acknowledge another terminal transition");
        let final_row = repo.find_proposal(&proposal.id).unwrap();
        assert_eq!(final_row.state, if integrate { "integrated" } else { "closed" });
        let audits = repo.meta().read(|tx| tx.audit_list(Some("proposal.close"), 100)).unwrap();
        assert_eq!(audits.len(), if integrate { 0 } else { 1 });
    }
}
