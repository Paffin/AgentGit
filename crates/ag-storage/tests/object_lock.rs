//! The cross-process object lock backed by PostgreSQL advisory locks.
//!
//! Needs a server: `scripts/pg-test.sh cargo test -p ag-storage --test object_lock`.
//! Without `AG_TEST_POSTGRES_URL` these print a notice and pass, unless
//! `AG_REQUIRE_BACKENDS=1`.
#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ag_storage::meta::{Meta, Param};
use ag_storage::testing::{TempSchema, postgres_url, unique};
use ag_storage::{ObjectLock, PostgresAdvisoryLock, SystemClock};
use ag_types::Limits;

fn url() -> Option<String> {
    let u = postgres_url();
    if u.is_none() {
        eprintln!("skipped: AG_TEST_POSTGRES_URL is not set (use scripts/pg-test.sh)");
    }
    u
}

fn kill_sessions_of(url: &str, app: &str) {
    let schema = TempSchema::new(url);
    let m = Meta::open_postgres(url, schema.name(), Arc::new(SystemClock), Limits::default()).unwrap();
    m.read(|tx| {
        tx.raw_query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = ?1",
            &[Param::from(app)],
        )
        .map(|_| ())
    })
    .unwrap();
}

#[test]
fn writers_share_and_the_collector_excludes_them_across_processes() {
    let Some(url) = url() else { return };
    let name = unique("lock");
    let a = PostgresAdvisoryLock::connect(&url, &name).unwrap();
    let b = PostgresAdvisoryLock::connect(&url, &name).unwrap();
    let w1 = a.shared().unwrap();
    let w2 = b.shared().unwrap(); // another process, same mode: no conflict
    let w3 = a.shared().unwrap(); // and another writer thread in the first process

    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let collector = std::thread::spawn(move || {
        let _g = b.exclusive().unwrap();
        flag.store(true, Ordering::SeqCst);
    });
    std::thread::sleep(Duration::from_millis(400));
    assert!(!done.load(Ordering::SeqCst), "the collector waits for every writer, in every process");
    drop((w1, w3));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!done.load(Ordering::SeqCst), "…including the last one");
    drop(w2);
    collector.join().unwrap();
    assert!(done.load(Ordering::SeqCst));
}

#[test]
fn the_collector_excludes_writers_of_other_processes_until_it_is_done() {
    let Some(url) = url() else { return };
    let name = unique("lock");
    let collector = PostgresAdvisoryLock::connect(&url, &name).unwrap();
    let writer = PostgresAdvisoryLock::connect(&url, &name).unwrap();
    let held = collector.exclusive().unwrap();
    let got = Arc::new(AtomicBool::new(false));
    let flag = got.clone();
    let t = std::thread::spawn(move || {
        let _g = writer.shared().unwrap();
        flag.store(true, Ordering::SeqCst);
    });
    std::thread::sleep(Duration::from_millis(400));
    assert!(!got.load(Ordering::SeqCst));
    drop(held);
    t.join().unwrap();
    assert!(got.load(Ordering::SeqCst));
}

#[test]
fn a_crashed_holder_releases_the_lock_by_itself() {
    let Some(url) = url() else { return };
    let name = unique("lock");
    let app = unique("aglock");
    let crashing = PostgresAdvisoryLock::connect_as(&url, &name, &app).unwrap();
    let other = PostgresAdvisoryLock::connect(&url, &name).unwrap();
    let held = crashing.exclusive().unwrap();
    // the holder's process dies: the server drops its session…
    kill_sessions_of(&url, &app);
    // …and the lock is free without any cleanup
    let started = Instant::now();
    drop(other.shared().unwrap());
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(held); // releasing on the dead session is harmless
}

#[test]
fn a_lock_whose_connection_died_between_uses_reconnects() {
    let Some(url) = url() else { return };
    let name = unique("lock");
    let app = unique("aglock");
    let lock = PostgresAdvisoryLock::connect_as(&url, &name, &app).unwrap();
    drop(lock.shared().unwrap());
    kill_sessions_of(&url, &app);
    std::thread::sleep(Duration::from_millis(200));
    drop(lock.shared().unwrap());
    drop(lock.exclusive().unwrap());
}

#[test]
fn locks_with_different_names_are_independent() {
    let Some(url) = url() else { return };
    let a = PostgresAdvisoryLock::connect(&url, &unique("lock")).unwrap();
    let b = PostgresAdvisoryLock::connect(&url, &unique("lock")).unwrap();
    let _held = a.exclusive().unwrap();
    let started = Instant::now();
    drop(b.exclusive().unwrap());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn an_unlock_the_server_refuses_does_not_leave_the_lock_held() {
    let Some(url) = url() else { return };
    let name = unique("lock");
    let holder = PostgresAdvisoryLock::connect(&url, &name).unwrap();
    let other = PostgresAdvisoryLock::connect(&url, &name).unwrap();
    let guard = holder.exclusive().unwrap();

    // Put the lock's session into an aborted transaction: the connection stays up, but the server now answers
    // every further statement (including the unlock) with an error.
    holder.batch_on_session_for_test("BEGIN").unwrap();
    holder.batch_on_session_for_test("SELECT 1/0").expect_err("division by zero aborts the transaction");
    drop(guard); // the unlock is refused

    // Before the fix the session kept the advisory lock for as long as this process lived, so nobody else could
    // ever take it. Now the failed unlock ends the session, which releases it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(other.exclusive().unwrap());
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(10)).expect("another process can take the lock after a refused unlock");

    // And the holder itself recovers: its next acquisition connects afresh.
    drop(holder.shared().unwrap());
    drop(holder.exclusive().unwrap());
}
