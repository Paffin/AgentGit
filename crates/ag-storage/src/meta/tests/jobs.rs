//! The job queue, on whichever backend the environment selects.

use std::sync::{Arc, Mutex};

use ag_types::Timestamp;
use serde_json::json;

use super::{clock, meta};
use crate::clock::Clock;
use crate::meta::{DEFAULT_JOB_ATTEMPTS, JobState, MetaError, NewJob};
use crate::testing::TestMeta;

fn t(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

#[test]
fn jobs_are_idempotent_by_dedupe_key_and_claimed_in_order() {
    let clock = clock();
    let m = TestMeta::new(clock.clone());
    let a = m.write(|tx| tx.job_enqueue("index", &json!({"n": 1}), Some("k1"), t(0))).unwrap();
    let again = m.write(|tx| tx.job_enqueue("index", &json!({"n": 999}), Some("k1"), t(0))).unwrap();
    assert_eq!(again.id, a.id, "same key, same job");
    assert_eq!(again.payload["n"], 1, "the original payload is kept");
    let b = m.write(|tx| tx.job_enqueue("index", &json!({"n": 2}), None, t(0))).unwrap();
    let c = m.write(|tx| tx.job_enqueue("index", &json!({"n": 3}), None, t(0))).unwrap();
    assert_ne!(b.id, c.id, "no key, no dedupe");
    assert_eq!(m.read(|tx| tx.count_rows("jobs")).unwrap(), 3);
    assert_eq!((a.state, a.attempts, a.max_attempts), (JobState::Queued, 0, DEFAULT_JOB_ATTEMPTS));

    clock.advance_ms(10);
    let first = m.write(|tx| tx.job_claim(&["index"], "w1", 1_000)).unwrap().unwrap();
    assert_eq!(first.id, a.id, "oldest first");
    assert_eq!((first.state, first.attempts, first.claimed_by.as_deref()), (JobState::Claimed, 1, Some("w1")));
    assert_eq!(first.claimed_until, Some(t(1_000_000 + 10 + 1_000)));
    let second = m.write(|tx| tx.job_claim(&[], "w2", 1_000)).unwrap().unwrap();
    assert_eq!(second.id, b.id, "an empty kind list claims any kind");
    assert!(m.write(|tx| tx.job_claim(&["other-kind"], "w3", 1_000)).unwrap().is_none(), "kind filter");
}

#[test]
fn jobs_wait_for_run_after_and_leases_are_reaped() {
    let clock = clock();
    let m = TestMeta::new(clock.clone());
    let later = m.write(|tx| tx.job_enqueue("k", &json!({}), None, t(1_000_000 + 5_000))).unwrap();
    assert!(m.write(|tx| tx.job_claim(&["k"], "w1", 1_000)).unwrap().is_none(), "not due yet");
    clock.advance_ms(5_000);
    let got = m.write(|tx| tx.job_claim(&["k"], "w1", 1_000)).unwrap().unwrap();
    assert_eq!(got.id, later.id);

    // w1 stops heartbeating; before the lease ends nobody else gets the job
    clock.advance_ms(999);
    assert!(m.write(|tx| tx.job_claim(&["k"], "w2", 1_000)).unwrap().is_none());
    // a heartbeat extends the lease
    m.write(|tx| tx.job_heartbeat(&later.id, "w1", 1_000)).unwrap();
    clock.advance_ms(999);
    assert!(m.write(|tx| tx.job_claim(&["k"], "w2", 1_000)).unwrap().is_none(), "the heartbeat kept it");
    // the lease runs out: w2 takes over, w1 is told it lost the job
    clock.advance_ms(2);
    let taken = m.write(|tx| tx.job_claim(&["k"], "w2", 1_000)).unwrap().unwrap();
    assert_eq!((taken.id.as_str(), taken.attempts, taken.claimed_by.as_deref()), (later.id.as_str(), 2, Some("w2")));
    assert!(taken.error.as_deref().unwrap().contains("lease expired"));
    assert!(matches!(m.write(|tx| tx.job_heartbeat(&later.id, "w1", 1_000)), Err(MetaError::NotFound(_))));
    assert!(matches!(m.write(|tx| tx.job_complete(&later.id, "w1", &json!({}))), Err(MetaError::NotFound(_))));
    let done = m.write(|tx| tx.job_complete(&later.id, "w2", &json!({"ok": true}))).unwrap();
    assert_eq!((done.state, done.result.unwrap()["ok"].clone()), (JobState::Succeeded, json!(true)));
    assert!(m.write(|tx| tx.job_complete(&later.id, "w2", &json!({}))).is_err(), "only once");
}

#[test]
fn failing_jobs_retry_until_the_attempts_run_out() {
    let clock = clock();
    let m = TestMeta::new(clock.clone());
    let job = m
        .write(|tx| {
            tx.job_enqueue_with(&NewJob {
                kind: "flaky",
                payload: &json!({}),
                dedupe_key: None,
                run_after: t(0),
                max_attempts: 3,
            })
        })
        .unwrap();
    for attempt in 1..=2 {
        let c = m.write(|tx| tx.job_claim(&["flaky"], "w", 10_000)).unwrap().unwrap();
        assert_eq!(c.attempts, attempt);
        let f = m.write(|tx| tx.job_fail(&job.id, "w", &format!("boom {attempt}"), Some(2_000))).unwrap();
        assert_eq!((f.state, f.error.as_deref()), (JobState::Queued, Some(format!("boom {attempt}").as_str())));
        assert_eq!(f.run_after, t(clock.now().as_millis() + 2_000), "retry no sooner than asked");
        assert!(m.write(|tx| tx.job_claim(&["flaky"], "w", 10_000)).unwrap().is_none(), "backing off");
        clock.advance_ms(2_000);
    }
    m.write(|tx| tx.job_claim(&["flaky"], "w", 10_000)).unwrap().unwrap();
    let last = m.write(|tx| tx.job_fail(&job.id, "w", "boom 3", Some(0))).unwrap();
    assert_eq!((last.state, last.attempts), (JobState::Failed, 3), "no attempts left");

    // `None` means "do not retry", even with attempts left
    let j2 = m.write(|tx| tx.job_enqueue("flaky", &json!({}), None, t(0))).unwrap();
    m.write(|tx| tx.job_claim(&["flaky"], "w", 10_000)).unwrap().unwrap();
    assert_eq!(m.write(|tx| tx.job_fail(&j2.id, "w", "hopeless", None)).unwrap().state, JobState::Failed);

    // a lease that expires on the last attempt fails the job instead of requeueing it
    let j3 = m
        .write(|tx| {
            tx.job_enqueue_with(&NewJob {
                kind: "once",
                payload: &json!({}),
                dedupe_key: None,
                run_after: t(0),
                max_attempts: 1,
            })
        })
        .unwrap();
    m.write(|tx| tx.job_claim(&["once"], "w", 100)).unwrap().unwrap();
    clock.advance_ms(101);
    assert!(m.write(|tx| tx.job_claim(&["once"], "w2", 100)).unwrap().is_none());
    let reaped = m.read(|tx| tx.job_get(&j3.id)).unwrap().unwrap();
    assert_eq!(reaped.state, JobState::Failed);
    assert!(reaped.error.unwrap().contains("lease expired"));
}

#[test]
fn jobs_can_be_cancelled_listed_and_looked_up() {
    let m = meta();
    let a = m.write(|tx| tx.job_enqueue("k", &json!({}), None, t(0))).unwrap();
    let b = m.write(|tx| tx.job_enqueue("k", &json!({}), None, t(0))).unwrap();
    m.write(|tx| tx.job_claim(&["k"], "w", 10_000)).unwrap().unwrap(); // claims `a`
    let cancelled = m.write(|tx| tx.job_cancel(&a.id)).unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert!(
        matches!(m.write(|tx| tx.job_heartbeat(&a.id, "w", 1)), Err(MetaError::NotFound(_))),
        "the worker finds out it was cancelled"
    );
    assert!(matches!(m.write(|tx| tx.job_cancel(&a.id)), Err(MetaError::InvalidTransition { .. })));
    assert!(matches!(m.write(|tx| tx.job_cancel("job_nope")), Err(MetaError::NotFound(_))));
    let queued = m.read(|tx| tx.job_list(Some(JobState::Queued), 10)).unwrap();
    assert_eq!(queued.iter().map(|j| &j.id).collect::<Vec<_>>(), [&b.id]);
    assert_eq!(m.read(|tx| tx.job_list(None, 10)).unwrap().len(), 2);
    assert_eq!(m.read(|tx| tx.job_list(None, 1)).unwrap().len(), 1);
    assert!(m.read(|tx| tx.job_get("job_nope")).unwrap().is_none());
    assert!(m.write(|tx| tx.job_enqueue("", &json!({}), None, t(0))).is_err());
    assert!(m.write(|tx| tx.job_claim(&[], "", 1)).is_err());
    assert!(m.write(|tx| tx.job_claim(&[], "w", 0)).is_err());
}

#[test]
fn concurrent_claimers_never_receive_the_same_job() {
    let setup = TestMeta::shared(clock());
    const JOBS: usize = 40;
    for i in 0..JOBS {
        setup.write(|tx| tx.job_enqueue("work", &json!({ "i": i }), Some(&format!("job-{i}")), t(0))).unwrap();
    }
    let claimed = Arc::new(Mutex::new(Vec::new()));
    let handles: Vec<_> = (0..8)
        .map(|w| {
            let (m, claimed) = (setup.reopen().unwrap(), claimed.clone());
            std::thread::spawn(move || {
                let worker = format!("worker-{w}");
                loop {
                    match m.write(|tx| tx.job_claim(&["work"], &worker, 60_000)) {
                        Ok(Some(job)) => {
                            claimed.lock().unwrap().push(job.id.clone());
                            m.write(|tx| tx.job_complete(&job.id, &worker, &json!({}))).unwrap();
                        }
                        Ok(None) => return,
                        Err(e) if e.is_retriable() => continue,
                        Err(e) => panic!("{e}"),
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let mut ids = claimed.lock().unwrap().clone();
    assert_eq!(ids.len(), JOBS, "every job was claimed");
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), JOBS, "…exactly once");
    assert_eq!(setup.read(|tx| tx.job_list(Some(JobState::Succeeded), 100)).unwrap().len(), JOBS);
}

#[test]
fn expired_leases_are_reported_when_they_are_reaped_and_jobs_are_found_by_key_and_counted() {
    let clock = clock();
    let m = TestMeta::new(clock.clone());
    let payload = json!({});
    let new = |attempts: u32, key: &'static str| NewJob {
        kind: "k",
        payload: &payload,
        dedupe_key: Some(key),
        run_after: t(0),
        max_attempts: attempts,
    };
    let retried = m.write(|tx| tx.job_enqueue_with(&new(3, "retried"))).unwrap();
    let last_try = m.write(|tx| tx.job_enqueue_with(&new(1, "last-try"))).unwrap();
    let healthy = m.write(|tx| tx.job_enqueue_with(&new(3, "healthy"))).unwrap();
    // Claimed in enqueue order: the first two with short leases, the third with a long one.
    for lease_ms in [100, 100, 60_000] {
        m.write(|tx| tx.job_claim(&["k"], "w", lease_ms)).unwrap().unwrap();
    }
    assert_eq!(m.read(|tx| tx.job_count(JobState::Claimed)).unwrap(), 3);
    assert_eq!(m.read(|tx| tx.job_by_key("healthy")).unwrap().unwrap().id, healthy.id);
    assert!(m.read(|tx| tx.job_by_key("nobody")).unwrap().is_none());

    assert!(m.write(|tx| tx.job_reap_expired()).unwrap().is_empty(), "nothing has expired yet");
    clock.advance_ms(101);
    // Two leases ran out: one job has attempts left, one has not. The caller learns of both - it is how a
    // crashed worker's permanent failure becomes visible at all.
    let mut reaped = m.write(|tx| tx.job_reap_expired()).unwrap();
    reaped.sort_by(|a, b| a.dedupe_key.cmp(&b.dedupe_key));
    let seen: Vec<_> = reaped.iter().map(|j| (j.dedupe_key.as_deref().unwrap(), j.state)).collect();
    assert_eq!(seen, [("last-try", JobState::Failed), ("retried", JobState::Queued)]);
    assert!(m.write(|tx| tx.job_reap_expired()).unwrap().is_empty(), "reaped once, reported once");
    assert_eq!(m.read(|tx| tx.job_get(&last_try.id)).unwrap().unwrap().state, JobState::Failed);
    assert_eq!(m.read(|tx| tx.job_get(&retried.id)).unwrap().unwrap().state, JobState::Queued);
    let counts = m
        .read(|tx| {
            Ok((tx.job_count(JobState::Queued)?, tx.job_count(JobState::Claimed)?, tx.job_count(JobState::Failed)?))
        })
        .unwrap();
    assert_eq!(counts, (1, 1, 1));
}
