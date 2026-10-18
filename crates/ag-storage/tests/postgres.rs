//! PostgreSQL-specific behaviour: transaction semantics, concurrency across
//! connections ("processes"), failure handling, TLS.
//!
//! These need a server: run them with `scripts/pg-test.sh cargo test -p ag-storage --test postgres`.
//! Without `AG_TEST_POSTGRES_URL` they print a notice and pass, unless
//! `AG_REQUIRE_BACKENDS=1`, which makes a missing server a failure.
#![allow(missing_docs)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use ag_storage::meta::{Meta, MetaError, Param, PgOptions, StreamState};
use ag_storage::testing::{TempSchema, postgres_url, unique};
use ag_storage::{Clock, ManualClock, SystemClock};
use ag_types::{Limits, StreamName, Timestamp};

fn url() -> Option<String> {
    let u = postgres_url();
    if u.is_none() {
        eprintln!("skipped: AG_TEST_POSTGRES_URL is not set (use scripts/pg-test.sh)");
    }
    u
}

fn open(url: &str, schema: &str) -> Meta {
    Meta::open_postgres(url, schema, Arc::new(SystemClock), Limits::default()).expect("open")
}

fn open_with(url: &str, schema: &str, opts: &PgOptions) -> Meta {
    Meta::open_postgres_with(url, schema, Arc::new(SystemClock), Limits::default(), opts).expect("open")
}

fn backend_pid(tx: &ag_storage::meta::Tx<'_>) -> i64 {
    tx.raw_query("SELECT pg_backend_pid()", &[]).unwrap()[0].i64(0).unwrap()
}

fn terminate(m: &Meta, pid: i64) {
    m.read(|tx| tx.raw_query("SELECT pg_terminate_backend(?1)", &[Param::Int(pid)]).map(|_| ())).unwrap();
}

#[test]
fn opening_twice_at_once_migrates_exactly_once() {
    let Some(url) = url() else { return };
    for _round in 0..3 {
        let schema = TempSchema::new(&url);
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (url, name, barrier) = (url.clone(), schema.name().to_string(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    open(&url, &name)
                })
            })
            .collect();
        let metas: Vec<Meta> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let m = &metas[0];
        assert_eq!(m.read(|tx| tx.meta_get("schema")).unwrap().as_deref(), Some("2"));
        let tables = m
            .read(|tx| {
                tx.raw_query(
                    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = ?1",
                    &[Param::from(schema.name())],
                )
            })
            .unwrap()[0]
            .i64(0)
            .unwrap();
        assert!(tables >= 20, "all tables exist once: {tables}");
        // every opener ended up on a usable, identical schema
        for other in &metas {
            other.write(|tx| tx.next_counter("race")).unwrap();
        }
        assert_eq!(m.write(|tx| tx.next_counter("race")).unwrap(), 9);
    }
}

#[test]
fn a_version_1_schema_migrates_forward_and_a_newer_one_is_refused() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    // schema 1, exactly as it would have been created by release 0.1 on PostgreSQL
    let m = open(&url, schema.name());
    m.write(|tx| {
        tx.raw_execute("DROP TABLE jobs", &[])?;
        tx.raw_execute("UPDATE meta SET value = '1' WHERE key = 'schema'", &[])
    })
    .unwrap();
    m.write(|tx| tx.create_stream(&StreamName::new("main").unwrap(), "user:t")).unwrap();
    drop(m);

    let m = open(&url, schema.name());
    assert_eq!(m.read(|tx| tx.meta_get("schema")).unwrap().as_deref(), Some("2"));
    assert!(m.read(|tx| tx.stream("main")).unwrap().is_some(), "data survives the migration");
    let job = m.write(|tx| tx.job_enqueue("k", &serde_json::json!({}), None, Timestamp::from_millis(0))).unwrap();
    assert_eq!(job.attempts, 0, "the jobs table exists again");

    m.write(|tx| tx.raw_execute("UPDATE meta SET value = '99' WHERE key = 'schema'", &[])).unwrap();
    let err = Meta::open_postgres(&url, schema.name(), Arc::new(SystemClock), Limits::default()).err().unwrap();
    assert!(matches!(err, MetaError::UnsupportedSchema { found: 99, supported: 2 }), "{err}");
}

#[test]
fn schemas_isolate_repositories() {
    let Some(url) = url() else { return };
    let (s1, s2) = (TempSchema::new(&url), TempSchema::new(&url));
    let (a, b) = (open(&url, s1.name()), open(&url, s2.name()));
    let main = StreamName::new("main").unwrap();
    a.write(|tx| tx.create_stream(&main, "user:a")).unwrap();
    assert!(b.read(|tx| tx.stream("main")).unwrap().is_none());
    b.write(|tx| tx.create_stream(&main, "user:b")).unwrap();
    assert_eq!(a.read(|tx| tx.stream("main")).unwrap().unwrap().created_by, "user:a");
    assert_eq!(b.read(|tx| tx.stream("main")).unwrap().unwrap().created_by, "user:b");
    assert_eq!(a.schema_name(), Some(s1.name()));
}

#[test]
fn writers_of_one_repository_are_serialized_and_see_each_others_commits() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let (a, b) = (open(&url, schema.name()), open(&url, schema.name())); // two "processes"
    let (inside_tx, inside_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let a_thread = std::thread::spawn(move || {
        a.write(|tx| {
            tx.meta_set("first", "written by A")?;
            inside_tx.send(()).unwrap();
            go_rx.recv_timeout(Duration::from_secs(20)).expect("released");
            tx.meta_set("a_done", "1")
        })
        .unwrap();
    });
    inside_rx.recv_timeout(Duration::from_secs(20)).unwrap();

    let b_inside = Arc::new(AtomicBool::new(false));
    let b_flag = b_inside.clone();
    let b_thread = std::thread::spawn(move || {
        b.write(|tx| {
            b_flag.store(true, Ordering::SeqCst);
            // The lock was granted only after A committed, and this transaction must
            // *see* that commit (READ COMMITTED after the lock, not a stale snapshot).
            assert_eq!(tx.meta_get("a_done")?.as_deref(), Some("1"));
            assert_eq!(tx.meta_get("first")?.as_deref(), Some("written by A"));
            Ok(())
        })
        .unwrap();
    });
    std::thread::sleep(Duration::from_millis(400));
    assert!(!b_inside.load(Ordering::SeqCst), "B must wait for A's write transaction to end");
    go_tx.send(()).unwrap();
    a_thread.join().unwrap();
    b_thread.join().unwrap();
    assert!(b_inside.load(Ordering::SeqCst));
}

#[test]
fn readers_are_never_blocked_by_a_writer_and_see_a_snapshot() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let (w, r) = (open(&url, schema.name()), open(&url, schema.name()));
    w.write(|tx| tx.meta_set("k", "old")).unwrap();
    let (inside_tx, inside_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let writer = std::thread::spawn(move || {
        w.write(|tx| {
            tx.meta_set("k", "new")?;
            inside_tx.send(()).unwrap();
            go_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            Ok(())
        })
        .unwrap();
    });
    inside_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let started = Instant::now();
    assert_eq!(r.read(|tx| tx.meta_get("k")).unwrap().as_deref(), Some("old"), "uncommitted data is invisible");
    assert!(started.elapsed() < Duration::from_secs(5), "the read did not wait for the writer");

    // one read transaction sees one snapshot even while others commit
    let (snap_tx, snap_rx) = channel();
    let (resume_tx, resume_rx) = channel::<()>();
    let r2 = open(&url, schema.name());
    let reader = std::thread::spawn(move || {
        r2.read(|tx| {
            let first = tx.meta_get("k")?;
            snap_tx.send(()).unwrap();
            resume_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            Ok((first, tx.meta_get("k")?))
        })
        .unwrap()
    });
    snap_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    go_tx.send(()).unwrap(); // the writer commits "new" now
    writer.join().unwrap();
    resume_tx.send(()).unwrap();
    let (first, second) = reader.join().unwrap();
    assert_eq!(first.as_deref(), Some("old"));
    assert_eq!(second.as_deref(), Some("old"), "repeatable read");
    assert_eq!(r.read(|tx| tx.meta_get("k")).unwrap().as_deref(), Some("new"));
}

#[test]
fn writers_of_different_repositories_do_not_wait_for_each_other() {
    let Some(url) = url() else { return };
    let (s1, s2) = (TempSchema::new(&url), TempSchema::new(&url));
    let (a, b) = (open(&url, s1.name()), open(&url, s2.name()));
    let (inside_tx, inside_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let a_thread = std::thread::spawn(move || {
        a.write(|_| {
            inside_tx.send(()).unwrap();
            go_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            Ok(())
        })
        .unwrap();
    });
    inside_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let started = Instant::now();
    b.write(|tx| tx.meta_set("k", "v")).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "no cross-repository lock");
    go_tx.send(()).unwrap();
    a_thread.join().unwrap();
}

#[test]
fn waiting_too_long_for_the_write_lock_is_a_retriable_error() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let impatient = PgOptions { lock_timeout: Duration::from_millis(500), ..PgOptions::default() };
    let (holder, waiter) = (open(&url, schema.name()), open_with(&url, schema.name(), &impatient));
    let (inside_tx, inside_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let t = std::thread::spawn(move || {
        holder
            .write(|_| {
                inside_tx.send(()).unwrap();
                go_rx.recv_timeout(Duration::from_secs(20)).unwrap();
                Ok(())
            })
            .unwrap();
    });
    inside_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let started = Instant::now();
    let err = waiter.write(|tx| tx.meta_set("k", "v")).unwrap_err();
    assert!(matches!(err, MetaError::Retriable(_)), "{err}");
    assert!(err.is_retriable());
    assert!(started.elapsed() >= Duration::from_millis(400) && started.elapsed() < Duration::from_secs(10));
    go_tx.send(()).unwrap();
    t.join().unwrap();
    // and the very same handle works once the lock is free
    waiter.write(|tx| tx.meta_set("k", "v")).unwrap();
}

#[test]
fn a_connection_killed_mid_transaction_is_an_error_and_nothing_is_applied() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let (m, killer) = (open(&url, schema.name()), open(&url, schema.name()));
    let err = m
        .write(|tx| {
            tx.meta_set("half", "done")?;
            terminate(&killer, backend_pid(tx));
            tx.meta_set("never", "reached") // the connection is gone
        })
        .unwrap_err();
    assert!(matches!(err, MetaError::ConnectionLost { during_commit: false, .. }), "{err}");
    assert!(err.is_retriable(), "nothing was committed: running it again is safe");
    assert!(killer.read(|tx| tx.meta_get("half")).unwrap().is_none(), "the partial transaction was rolled back");

    // the handle recovers: the dead connection is discarded, a new one is opened
    m.write(|tx| tx.meta_set("after", "ok")).unwrap();
    assert_eq!(killer.read(|tx| tx.meta_get("after")).unwrap().as_deref(), Some("ok"));
    // the killed session's advisory lock is gone with it (a second writer is not stuck)
    killer.write(|tx| tx.meta_set("again", "ok")).unwrap();
}

#[test]
fn a_connection_lost_before_commit_is_reported_as_unknown_outcome() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let (m, killer) = (open(&url, schema.name()), open(&url, schema.name()));
    // The closure is happy, but the session dies before the COMMIT can be sent.
    let err = m
        .write(|tx| {
            tx.meta_set("k", "v")?;
            terminate(&killer, backend_pid(tx));
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(err, MetaError::ConnectionLost { during_commit: true, .. }), "{err}");
    assert!(!err.is_retriable(), "a commit with an unknown outcome must not be blindly repeated");
    assert!(killer.read(|tx| tx.meta_get("k")).unwrap().is_none(), "(here the kill came first, so it was not applied)");
}

#[test]
fn idle_pooled_connections_that_died_are_replaced_transparently() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let app = unique("agpool");
    let opts = PgOptions { application_name: app.clone(), pool_size: 3, ..PgOptions::default() };
    let m = open_with(&url, schema.name(), &opts);
    let killer = open(&url, schema.name());
    for _ in 0..3 {
        m.write(|tx| tx.next_counter("c")).unwrap();
    }
    // the server (or a firewall) drops every idle connection
    killer
        .write(|tx| {
            tx.raw_query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = ?1",
                &[Param::from(app.as_str())],
            )
            .map(|_| ())
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(m.write(|tx| tx.next_counter("c")).unwrap(), 4, "the first use after the kill just works");
    assert_eq!(m.read(|tx| tx.meta_get("schema")).unwrap().as_deref(), Some("2"));
}

#[test]
fn a_statement_failure_rolls_back_and_leaves_the_connection_usable() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let m = open(&url, schema.name());
    m.write(|tx| tx.create_stream(&StreamName::new("main").unwrap(), "user:t")).unwrap();
    let dup = m.write(|tx| {
        tx.meta_set("before", "1")?;
        tx.raw_execute(
            "INSERT INTO streams (name, head, version, created_at, created_by) VALUES ('main', NULL, 0, 0, 'x')",
            &[],
        )
    });
    assert!(matches!(dup, Err(MetaError::Postgres(m)) if m.contains("23505")), "unique violation carries its SQLSTATE");
    assert!(m.read(|tx| tx.meta_get("before")).unwrap().is_none());
    assert_eq!(m.open_connections(), 1, "a database error does not cost the connection");
    m.write(|tx| tx.meta_set("fine", "yes")).unwrap();
}

#[test]
fn a_panic_inside_a_transaction_does_not_leak_the_lock_or_the_connection() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let opts = PgOptions { pool_size: 1, ..PgOptions::default() };
    let m = Arc::new(open_with(&url, schema.name(), &opts));
    let m2 = m.clone();
    let r = std::thread::spawn(move || {
        let _ = m2.write(|tx| {
            tx.meta_set("k", "v")?;
            if true {
                panic!("boom");
            }
            Ok(())
        });
    })
    .join();
    assert!(r.is_err(), "the panic propagated");
    // The only pooled connection was mid-transaction: it must have been discarded,
    // otherwise this would reuse a transaction that holds the write lock.
    m.write(|tx| tx.meta_set("k2", "v")).unwrap();
    assert!(m.read(|tx| tx.meta_get("k")).unwrap().is_none(), "the panicked transaction never committed");
}

#[test]
fn the_pool_is_bounded_and_connections_are_reused() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let opts = PgOptions { pool_size: 2, ..PgOptions::default() };
    let m = Arc::new(open_with(&url, schema.name(), &opts));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let m = m.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    m.write(|tx| tx.next_counter("n")).unwrap();
                    m.read(|tx| tx.meta_get("schema")).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert!(m.open_connections() <= 2, "{}", m.open_connections());
    assert_eq!(m.write(|tx| tx.next_counter("n")).unwrap(), 81);
}

#[test]
fn many_connections_racing_the_stream_cas_never_lose_an_update() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(1_000)));
    let setup = Meta::open_postgres(&url, schema.name(), clock.clone(), Limits::default()).unwrap();
    // a chain of 6 published changesets the workers race to the end of
    let chain = build_chain(&setup, 6);
    setup.write(|tx| tx.create_stream(&StreamName::new("main").unwrap(), "user:t")).unwrap();

    const WORKERS: usize = 16;
    let wins = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(WORKERS));
    let handles: Vec<_> = (0..WORKERS)
        .map(|w| {
            let (m, wins, barrier, chain) = (open(&url, schema.name()), wins.clone(), barrier.clone(), chain.clone());
            std::thread::spawn(move || {
                barrier.wait();
                // Optimistic loop: read the state, pick the next changeset in the chain, CAS.
                // Exactly one worker can win each step; the others see CasMismatch and re-read.
                for _ in 0..500 {
                    let st: StreamState = m.read(|tx| tx.stream("main")).unwrap().unwrap().state;
                    let step = st.version as usize;
                    if step >= chain.len() {
                        return;
                    }
                    match m.write(|tx| tx.cas_stream("main", &st, &chain[step], None, &format!("user:w{w}"), "race")) {
                        Ok(_) => {
                            wins.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(MetaError::CasMismatch { .. }) => {}
                        Err(e) if e.is_retriable() => {}
                        Err(e) => panic!("worker {w}: {e}"),
                    }
                }
                panic!("worker {w} starved");
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(wins.load(Ordering::SeqCst), chain.len() as u64, "each step was won exactly once");
    let st = setup.read(|tx| tx.stream("main")).unwrap().unwrap().state;
    assert_eq!((st.version, st.head), (chain.len() as u64, chain.last().copied()));
    let log = setup.read(|tx| tx.reflog("main", 100)).unwrap();
    assert_eq!(log.len(), chain.len());
    let mut prev = None;
    for e in log.into_iter().rev() {
        assert_eq!(e.old_head, prev, "the reflog is one contiguous chain");
        prev = e.new_head;
    }
}

fn build_chain(m: &Meta, n: usize) -> Vec<ag_types::ChangesetId> {
    use ag_changeset::{Changeset, Op};
    use ag_object::hash_blob;
    use ag_types::{FileType, Identity, Namespace, RepoPath};
    let mut parent = Vec::new();
    let mut out = Vec::new();
    for i in 0..n {
        let name = format!("f{i}");
        let c = Changeset::build(
            Namespace::new("t/t").unwrap(),
            parent.clone(),
            Identity::new("user:t").unwrap(),
            Timestamp::from_millis(0),
            format!("add {name}"),
            vec![Op::Put {
                path: RepoPath::new(&name).unwrap(),
                content: hash_blob(name.as_bytes()),
                file_type: FileType::Regular,
                executable: false,
            }],
            &Limits::default(),
        )
        .unwrap();
        let id = c.id();
        m.write(|tx| tx.register_changeset(&id, &c)).unwrap();
        parent = vec![id];
        out.push(id);
    }
    out
}

#[test]
fn backup_is_refused_with_a_pointer_to_pg_dump() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let m = open(&url, schema.name());
    let dest = std::env::temp_dir().join(unique("agtest-backup"));
    let err = m.backup_to(&dest).unwrap_err().to_string();
    assert!(err.contains("pg_dump") && err.contains("ag verify"), "{err}");
    assert!(!dest.exists());
}

#[test]
fn bad_connection_settings_fail_early_and_never_echo_the_password() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    for bad_schema in ["", "Mixed", "pg_catalog", "a-b", "x;drop schema public"] {
        let r = Meta::open_postgres(&url, bad_schema, clock.clone(), Limits::default());
        assert!(matches!(r, Err(MetaError::Invalid(_))), "{bad_schema:?}");
    }
    let r = Meta::open_postgres(
        "postgres://u:s3cret@127.0.0.1:1/db?sslmode=disable",
        schema.name(),
        clock.clone(),
        Limits::default(),
    );
    let msg = r.expect_err("connection refused").to_string();
    assert!(!msg.contains("s3cret"), "{msg}");
    let r = Meta::open_postgres("not a url", schema.name(), clock, Limits::default());
    assert!(matches!(r, Err(MetaError::Invalid(_))));
}

#[test]
fn a_server_that_refuses_the_login_is_an_unavailable_database_not_an_internal_error() {
    let Some(url) = url() else { return };
    let schema = TempSchema::new(&url);
    let admin = open(&url, schema.name());
    let role = unique("agtest_locked");
    admin.write(|tx| tx.raw_query(&format!("CREATE ROLE {role} LOGIN SUPERUSER"), &[]).map(|_| ())).unwrap();
    let locked_url = url.replacen("postgres@", &format!("{role}@"), 1);
    assert_ne!(locked_url, url, "the test URL names the postgres user");
    // While the role may log in, opening works.
    let other = TempSchema::new(&url);
    open(&locked_url, other.name());
    // Locked out: the server answers, but with SQLSTATE 28000. Callers must see a connection failure (which the
    // API reports as retryable `unavailable`), not an opaque statement error.
    admin.write(|tx| tx.raw_query(&format!("ALTER ROLE {role} NOLOGIN"), &[]).map(|_| ())).unwrap();
    let r = Meta::open_postgres(&locked_url, other.name(), Arc::new(SystemClock), Limits::default());
    let result = match r {
        Err(e @ MetaError::ConnectionLost { during_commit: false, .. }) => {
            assert!(e.to_string().contains("28000"), "the SQLSTATE stays in the message: {e}");
            Ok(())
        }
        Err(e) => Err(format!("expected ConnectionLost, got {e:?}")),
        Ok(_) => Err("a role that may not log in must not connect".to_string()),
    };
    let _ = admin.write(|tx| tx.raw_query(&format!("DROP OWNED BY {role}"), &[]).map(|_| ()));
    let _ = admin.write(|tx| tx.raw_query(&format!("DROP ROLE {role}"), &[]).map(|_| ()));
    result.unwrap();
}

// ---- TLS (needs a server with ssl=on; scripts/pg-test.sh provides one) ---------

#[cfg(feature = "tls")]
fn tls_urls() -> Option<(String, String, String)> {
    let base = url()?;
    let Ok(ca) = std::env::var("AG_TEST_POSTGRES_CA") else {
        eprintln!("skipped: AG_TEST_POSTGRES_CA is not set (use scripts/pg-test.sh)");
        assert!(!ag_storage::testing::backends_required(), "AG_REQUIRE_BACKENDS: no TLS-enabled server");
        return None;
    };
    let host_port = base.split('@').nth(1)?.split('/').next()?.to_string();
    let (_, port) = host_port.rsplit_once(':')?;
    let user = "postgres";
    let tls = |mode: &str| format!("postgres://{user}@localhost:{port}/postgres?sslmode={mode}");
    Some((tls("require"), format!("{}&sslrootcert={ca}", tls("verify-full")), ca))
}

#[cfg(feature = "tls")]
#[test]
fn tls_connections_work_and_certificates_are_verified_when_asked() {
    let Some((require, verify, ca)) = tls_urls() else { return };
    let schema = TempSchema::new(&require);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    // sslmode=require: encrypted
    let m = Meta::open_postgres(&require, schema.name(), clock.clone(), Limits::default()).unwrap();
    m.write(|tx| tx.meta_set("k", "v")).unwrap();
    let ssl = m.read(|tx| tx.raw_query("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()", &[])).unwrap();
    assert_eq!(ssl[0].i64(0).unwrap(), 1, "the session really is encrypted");

    // verify-full with the right CA works
    let m = Meta::open_postgres(&verify, schema.name(), clock.clone(), Limits::default()).unwrap();
    assert_eq!(m.read(|tx| tx.meta_get("k")).unwrap().as_deref(), Some("v"));

    // verify-full without the CA (system/web roots only) must refuse the self-signed server
    let untrusted = verify.split("&sslrootcert=").next().unwrap().to_string();
    let err = Meta::open_postgres(&untrusted, schema.name(), clock.clone(), Limits::default()).expect_err("must fail");
    assert!(
        err.to_string().to_lowercase().contains("certificate") || err.to_string().to_lowercase().contains("unknown"),
        "{err}"
    );

    // verify-full against the wrong host name must fail too
    let wrong_host = verify.replace("@localhost:", "@127.0.0.2:");
    assert!(Meta::open_postgres(&wrong_host, schema.name(), clock.clone(), Limits::default()).is_err());

    // an unreadable CA file is a clear configuration error
    let bad_ca = verify.replace(&ca, "/nonexistent/ca.pem");
    let err = Meta::open_postgres(&bad_ca, schema.name(), clock, Limits::default()).err().unwrap();
    assert!(matches!(err, MetaError::Invalid(_)), "{err}");
}
