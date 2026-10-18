//! The commands that list things, past the size of one page, and the failures that must be loud: every page of a
//! server listing arrives, a live feed is one JSON object per line, a bad deadline is refused, a credential that
//! cannot be deleted is reported, and a damaged metadata database is not mistaken for an empty repository.
#![allow(missing_docs)]

mod common;

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use ag_server::authz::Role;
use ag_storage::meta::NewProposal;
use common::{Ag, Run, Server};

/// A server with the repository `acme/demo`, and a clone of it that pushed one proposal.
struct Fixture {
    srv: Server,
    token: String,
    a: Ag,
    proposal: String,
}

impl Fixture {
    fn new() -> Self {
        let srv = Server::start();
        srv.create_repo("acme/demo");
        let token = srv.user("user:alice", "acme/demo", Role::Writer);
        let a = Ag::bare();
        let url = format!("{}/acme/demo", srv.url);
        let run = |args: &[&str]| a.run_env("user:alice", args, &[("AG_TOKEN", token.as_str())]);
        run(&["init", "--namespace", "acme/demo"]).ok();
        run(&["remote", "add", "origin", &url]).ok();
        a.write("README.md", "# demo\n");
        run(&["snapshot", "-m", "first"]).ok();
        let proposal =
            run(&["--json", "push", "-t", "first"]).ok().json()["proposal"]["id"].as_str().unwrap().to_string();
        Self { srv, token, a, proposal }
    }

    fn ag(&self, args: &[&str]) -> Run {
        self.ag_env(args, &[])
    }

    fn ag_env(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut all = vec![("AG_TOKEN", self.token.as_str())];
        all.extend_from_slice(env);
        self.a.run_env("user:alice", args, &all)
    }
}

#[test]
fn a_listing_of_more_proposals_than_one_page_holds_is_complete() {
    let fx = Fixture::new();
    let repo = fx.srv.app.repos.handle("acme/demo").unwrap();
    let head = repo.meta().read(|tx| tx.proposal(&fx.proposal)).unwrap().unwrap().head;
    // More than a page: the server never returns more than MAX_PAGE_SIZE proposals in one answer.
    let extra = ag_protocol::limits::MAX_PAGE_SIZE as usize + 150;
    repo.meta()
        .write(|tx| {
            for i in 0..extra {
                tx.proposal_create(&NewProposal {
                    id: &format!("prop_bulk{i:05}"),
                    stream: "main",
                    head: &head,
                    title: "bulk",
                    description: "",
                    author: "user:alice",
                    task_id: None,
                })?;
            }
            Ok(())
        })
        .unwrap();

    let all = fx.ag(&["--json", "remote", "proposals"]).ok().json();
    let ids: Vec<&str> = all["proposals"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), extra + 1, "every proposal, not the first page of them");
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "and none twice");
    assert!(ids.contains(&fx.proposal.as_str()), "including the oldest");
    // The text form is the same list.
    let text = fx.ag(&["remote", "proposals", "--state", "open"]).ok().stdout();
    assert_eq!(text.lines().count(), extra + 1);
    // And a filter finds nothing where there is nothing.
    let closed = fx.ag(&["--json", "remote", "proposals", "--state", "closed"]).ok().json();
    assert!(closed["proposals"].as_array().unwrap().is_empty());
}

#[test]
fn replaying_the_events_reads_every_page() {
    let fx = Fixture::new();
    let repo = fx.srv.app.repos.handle("acme/demo").unwrap();
    // More audit entries than two replay pages of the size the CLI asks for.
    let extra = ag_protocol::limits::MAX_PAGE_SIZE as usize * 2 + 37;
    repo.meta()
        .write(|tx| {
            for i in 0..extra {
                tx.audit_append("system:test", "bulk.event", &format!("t{i}"), &serde_json::json!({"i": i}))?;
            }
            Ok(())
        })
        .unwrap();

    let bulk = fx.ag(&["--json", "remote", "events", "--topics", "bulk"]).ok().json();
    let events = bulk["events"].as_array().unwrap();
    assert_eq!(events.len(), extra, "all of the matching events");
    let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "oldest first, each once");

    // From a later point on: only what follows it.
    let after = seqs[extra - 11];
    let tail = fx.ag(&["--json", "remote", "events", "--topics", "bulk", "--after", &after.to_string()]).ok().json();
    assert_eq!(tail["events"].as_array().unwrap().len(), 10);
    // Without a topic filter, the repository's own events (the push) come first and are all there too.
    let every = fx.ag(&["--json", "remote", "events"]).ok().json();
    let n = every["events"].as_array().unwrap().len();
    assert!(n > extra, "{n} events");
    assert!(every["latest_seq"].as_u64().unwrap() >= *seqs.last().unwrap());
}

#[test]
fn a_live_feed_is_one_json_object_per_line() {
    let fx = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ag"))
        .current_dir(fx.a.dir.path())
        .env("AG_IDENTITY", "user:alice")
        .env("AG_TOKEN", &fx.token)
        .env("AG_CONFIG", fx.a.cfg.path().join("config.json"))
        .env_remove("AG_KEY_DIR")
        .args(["--json", "remote", "events", "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut seen = Vec::new();
    while seen.len() < 2 {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(line)) => seen.push(line),
            other => {
                let _ = child.kill();
                panic!("the feed stopped after {seen:?}: {other:?}");
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let events: Vec<serde_json::Value> = seen
        .iter()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("a line of the feed is not JSON ({e}): {l:?}")))
        .collect();
    assert!(events[0]["seq"].as_u64().unwrap() < events[1]["seq"].as_u64().unwrap());
    assert!(events.iter().all(|e| e["topic"].is_string()), "{events:?}");
    // the same shape as an event of `ag --json remote events`
    let replayed = fx.ag(&["--json", "remote", "events"]).ok().json();
    let keys = |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys(&events[0]), keys(&replayed["events"][0]));
}

#[test]
fn a_deadline_that_is_not_a_positive_number_of_seconds_is_refused() {
    let fx = Fixture::new();
    for bad in ["abc", "0", "-5", "1.5", "10s"] {
        let r = fx.ag_env(&["remote", "whoami"], &[("AG_TIMEOUT", bad)]);
        assert_ne!(r.code(), 0, "AG_TIMEOUT={bad}");
        assert!(r.stderr().contains("AG_TIMEOUT"), "AG_TIMEOUT={bad}: {}", r.stderr());
    }
    // A good one is used, an empty one is the same as none.
    fx.ag_env(&["remote", "whoami"], &[("AG_TIMEOUT", "30")]).ok();
    fx.ag_env(&["remote", "whoami"], &[("AG_TIMEOUT", "")]).ok();
    // The same variable governs a clone.
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("w");
    let url = format!("{}/acme/demo", fx.srv.url);
    let r = common::run_in(
        parent.path(),
        fx.a.cfg.path(),
        "user:alice",
        &["clone", &url, dest.to_str().unwrap()],
        &[("AG_TOKEN", fx.token.as_str()), ("AG_TIMEOUT", "never")],
    );
    assert_ne!(r.code(), 0);
    assert!(r.stderr().contains("AG_TIMEOUT"), "{}", r.stderr());
}

#[test]
fn removing_a_remote_whose_credential_cannot_be_deleted_says_so_and_keeps_the_remote() {
    let fx = Fixture::new();
    // A stored credential that cannot be deleted as a file: a directory with something in it stands in.
    let stuck = fx.a.root().join(".ag/credentials/origin");
    std::fs::create_dir_all(&stuck).unwrap();
    std::fs::write(stuck.join("keep"), "x").unwrap();
    let r = fx.ag(&["remote", "remove", "origin"]);
    assert_ne!(r.code(), 0, "{}", r.stdout());
    assert!(r.stderr().contains("credential") && r.stderr().contains("origin"), "{}", r.stderr());
    assert!(fx.ag(&["remote", "list"]).ok().stdout().contains("origin"), "the remote is still there");
    // With the obstacle gone, the same command works and a missing credential is not an error.
    std::fs::remove_dir_all(&stuck).unwrap();
    fx.ag(&["remote", "remove", "origin"]).ok();
    assert!(!fx.ag(&["remote", "list"]).ok().stdout().contains("origin"));
    fx.ag(&["remote", "add", "origin", &format!("{}/acme/demo", fx.srv.url)]).ok();
    fx.ag(&["remote", "remove", "origin"]).ok();
}

#[test]
fn listings_stop_at_a_cap_that_is_said_out_loud() {
    let a = Ag::new();
    for i in 0..3 {
        a.write(&format!("f{i}.txt"), "x\n");
        a.propose_as("user:alice", &format!("p{i}"));
    }
    let r = a.ag(&["--json", "proposal", "list", "-n", "2"]).ok();
    assert_eq!(r.json().as_array().unwrap().len(), 2);
    assert!(r.stderr().contains("showing the newest 2 proposals"), "{}", r.stderr());
    // Everything, with no hint, when the cap is lifted or not reached.
    for args in [&["--json", "proposal", "list", "-n", "0"][..], &["--json", "proposal", "list"][..]] {
        let r = a.ag(args).ok();
        assert_eq!(r.json().as_array().unwrap().len(), 3);
        assert!(!r.stderr().contains("showing the newest"), "{}", r.stderr());
    }
    a.ag(&["--json", "agent", "runs", "-n", "0"]).ok();
    // A cap on the active leases would mean nothing: only the history takes one.
    a.ag(&["lease", "list", "-n", "1"]).code_is(2);
    a.ag(&["--json", "lease", "list", "--all", "-n", "1"]).ok();
}

#[test]
fn a_metadata_database_that_cannot_be_read_is_not_an_empty_history() {
    let a = Ag::new();
    // A repository without snapshots has no history, and says that.
    let r = a.ag(&["log"]);
    assert_ne!(r.code(), 0);
    assert!(r.stderr().contains("no history yet"), "{}", r.stderr());
    // Damage the table the lookup reads. The failure is the database's, not "nothing there".
    let db = rusqlite::Connection::open(a.root().join(".ag/meta.db")).unwrap();
    db.execute_batch("DROP TABLE workspaces").unwrap();
    drop(db);
    let r = a.ag(&["log"]);
    assert_ne!(r.code(), 0);
    assert!(!r.stderr().contains("no history yet"), "{}", r.stderr());
    assert!(r.stderr().contains("workspaces"), "the real cause is shown: {}", r.stderr());
}
