//! Count quotas are enforced inside the transaction that inserts the row: concurrent creates cannot pass the
//! limit between them. Runs on every backend the environment selects (`scripts/pg-test.sh` for PostgreSQL).
#![allow(missing_docs)]

mod common;

use std::sync::Barrier;

use ag_integrate::Error;
use ag_storage::meta::MetaError;
use ag_types::StreamName;
use common::*;

const THREADS: usize = 16;

fn is_quota(e: &Error) -> bool {
    matches!(e, Error::Meta(MetaError::QuotaExceeded(_)))
}

#[test]
fn concurrent_stream_creates_cannot_pass_the_stream_limit() {
    let f = fx();
    let limit = 5u64; // `main` exists already: four more fit
    let start = Barrier::new(THREADS);
    let results: Vec<Result<_, Error>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|i| {
                let (repo, start) = (&f.repo, &start);
                s.spawn(move || {
                    let name = StreamName::new(format!("s{i}")).unwrap();
                    start.wait();
                    repo.create_stream_capped(&name, None, &id("user:alice"), Some(limit))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let created = results.iter().filter(|r| r.is_ok()).count();
    let refused = results.iter().filter(|r| r.as_ref().is_err_and(is_quota)).count();
    assert_eq!((created, refused), (4, THREADS - 4), "every other outcome is a bug: {results:?}");
    assert_eq!(f.repo.meta().read(|tx| tx.count_streams()).unwrap(), limit);
    // A refused create left nothing behind, and without a limit the same call is unrestricted.
    let extra = StreamName::new("extra").unwrap();
    assert!(is_quota(&f.repo.create_stream_capped(&extra, None, &id("user:alice"), Some(limit)).unwrap_err()));
    assert!(f.repo.meta().read(|tx| tx.stream("extra")).unwrap().is_none());
    f.repo.create_stream_capped(&extra, None, &id("user:alice"), None).unwrap();
}

#[test]
fn concurrent_proposals_cannot_pass_the_open_proposal_limit() {
    let f = fx();
    let limit = 3u64;
    let heads: Vec<_> =
        (0..THREADS).map(|i| f.snap(None, &[("f.txt", &format!("version {i}\n"))], "user:alice")).collect();
    let start = Barrier::new(THREADS);
    let results: Vec<Result<_, Error>> = std::thread::scope(|s| {
        let handles: Vec<_> = heads
            .iter()
            .enumerate()
            .map(|(i, head)| {
                let (repo, start) = (&f.repo, &start);
                s.spawn(move || {
                    start.wait();
                    repo.propose_capped("main", head, &format!("p{i}"), "", &id("user:alice"), None, None, Some(limit))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let created = results.iter().filter(|r| r.is_ok()).count();
    let refused = results.iter().filter(|r| r.as_ref().is_err_and(is_quota)).count();
    assert_eq!((created, refused), (3, THREADS - 3), "every other outcome is a bug: {results:?}");
    assert_eq!(f.repo.meta().read(|tx| tx.proposals_count("open")).unwrap(), limit);

    // Closing one frees exactly one place.
    let first = results.iter().find_map(|r| r.as_ref().ok()).unwrap().0.id.clone();
    f.repo.close_proposal(&first, &id("user:alice")).unwrap();
    let again = f.repo.propose_capped("main", &heads[0], "again", "", &id("user:alice"), None, None, Some(limit));
    assert!(again.is_ok(), "{again:?}");
    let over = f.repo.propose_capped("main", &heads[1], "over", "", &id("user:alice"), None, None, Some(limit));
    assert!(is_quota(&over.unwrap_err()));
}

#[test]
fn a_replayed_proposal_is_served_at_the_limit_and_a_refused_one_leaves_no_trace() {
    let f = fx();
    let (a, b) = (f.snap(None, &[("a", "1\n")], "user:alice"), f.snap(None, &[("b", "2\n")], "user:alice"));
    let alice = id("user:alice");
    let (first, created) = f.repo.propose_capped("main", &a, "a", "", &alice, None, Some("key-1"), Some(1)).unwrap();
    assert!(created);
    // The limit (1) is reached; the same request again is the same proposal, not a refusal...
    let (again, created) = f.repo.propose_capped("main", &a, "a", "", &alice, None, Some("key-1"), Some(1)).unwrap();
    assert_eq!((again.id.as_str(), created), (first.id.as_str(), false));
    // ...a new one is refused, and its idempotency key is not consumed by the refusal.
    let refused = f.repo.propose_capped("main", &b, "b", "", &alice, None, Some("key-2"), Some(1)).unwrap_err();
    assert!(is_quota(&refused), "{refused:?}");
    f.repo.close_proposal(&first.id, &alice).unwrap();
    let (second, created) = f.repo.propose_capped("main", &b, "b", "", &alice, None, Some("key-2"), Some(1)).unwrap();
    assert!(created, "{second:?}");
}
