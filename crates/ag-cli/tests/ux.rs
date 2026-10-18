//! How `ag` presents itself: colour, `--quiet`, `--verbose`, the global `-C`, error hints, help topics and
//! `ag config`. The binary is run the way a script or a person would run it.
#![allow(missing_docs)]

mod common;

use std::io::Read;
use std::process::{Command, Stdio};

use common::{Ag, run_in};

const ESC: char = '\u{1b}';

#[test]
fn colour_only_appears_when_asked_for_and_never_in_a_pipe() {
    let a = Ag::new();
    // The test captures output, so nothing here is a terminal.
    let plain = a.ag(&["doctor"]).ok().stdout();
    assert!(!plain.contains(ESC), "piped output must be plain: {plain:?}");
    assert!(a.ag(&["--help"]).ok().stdout().find(ESC).is_none(), "piped --help must be plain");

    let forced = a.ag(&["--color", "always", "doctor"]).ok().stdout();
    assert!(forced.contains("\u{1b}[32mok"), "{forced:?}");
    let forced_eq = a.ag(&["--color=always", "doctor"]).ok().stdout();
    assert!(forced_eq.contains(ESC));

    // NO_COLOR is honoured, and so is an explicit `--color never` against CLICOLOR_FORCE.
    let no_color = a.run_env("user:alice", &["doctor"], &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")]).ok().stdout();
    assert!(!no_color.contains(ESC), "{no_color:?}");
    let never = a.run_env("user:alice", &["--color", "never", "doctor"], &[("CLICOLOR_FORCE", "1")]).ok().stdout();
    assert!(!never.contains(ESC), "{never:?}");
    // CLICOLOR_FORCE alone colours even a pipe (the convention for CI logs).
    let force = a.run_env("user:alice", &["doctor"], &[("CLICOLOR_FORCE", "1")]).ok().stdout();
    assert!(force.contains(ESC));

    // The parser's own help and usage errors follow the flag too.
    let help = a.ag(&["--color", "always", "--help"]).ok().stdout();
    assert!(help.contains(ESC), "help should be coloured with --color always");
    let help = a.run_env("user:alice", &["--color", "never", "--help"], &[("CLICOLOR_FORCE", "1")]).ok().stdout();
    assert!(!help.contains(ESC));

    // JSON is never coloured, whatever is asked.
    let json = a.ag(&["--color", "always", "--json", "doctor"]).ok().stdout();
    assert!(!json.contains(ESC));
    // A user-level setting is a default that a flag overrides.
    a.ag(&["config", "set", "color", "always"]).ok();
    assert!(a.ag(&["doctor"]).ok().stdout().contains(ESC));
    assert!(!a.ag(&["--color", "never", "doctor"]).ok().stdout().contains(ESC));
    // Errors are coloured on stderr by the same rules.
    let err = a.ag(&["integrate", "nope"]).code_is(1).stderr();
    assert!(err.contains("\u{1b}[31merror:"), "{err:?}");
}

#[test]
fn quiet_prints_only_what_was_asked_for() {
    let a = Ag::new();
    a.write("README.md", "# demo\n");
    // A snapshot prints just its id; scripts can capture it.
    let id = a.ag(&["-q", "snapshot", "-m", "first"]).ok().stdout();
    assert_eq!(id.trim().len(), 64, "a bare changeset id: {id:?}");
    assert!(id.trim().chars().all(|c| c.is_ascii_hexdigit()));
    let prop = a.ag(&["--quiet", "propose", "-t", "first"]).ok().stdout();
    assert!(prop.trim().starts_with("prop_") && !prop.trim().contains(' '), "{prop:?}");
    // A successful integration says nothing; the exit code is the answer.
    let r = a.ag(&["-q", "integrate", prop.trim()]).ok();
    assert_eq!(r.stdout(), "");
    assert_eq!(r.stderr(), "");
    // Data the user asked for is still printed.
    assert!(a.ag(&["-q", "log"]).ok().stdout().contains("first"));
    assert!(a.ag(&["-q", "status"]).ok().stdout().contains("workspace 'default'"));
    // JSON is data too.
    assert_eq!(a.ag(&["-q", "--json", "snapshot", "-m", "again"]).ok().json()["changeset"], serde_json::Value::Null);
    // Refusals are never silenced.
    a.write("README.md", "# demo\nmore\n");
    a.ag(&["-q", "snapshot", "-m", "second"]).ok();
    let blocked_prop = a.ag(&["-q", "propose", "-t", "second"]).ok().stdout();
    a.write("policy.json", r#"{"approvals": {"min": 1}}"#);
    a.ag(&["policy", "set", "policy.json"]).ok();
    let blocked = a.ag(&["-q", "integrate", blocked_prop.trim()]).code_is(3);
    assert!(blocked.stdout().contains("Blocked"), "reasons must survive --quiet: {}", blocked.stdout());
    assert!(a.ag(&["-q", "-v", "status"]).code_is(2).stderr().contains("cannot be used with"));
}

/// The tutorials capture these values in shell variables (`KEY=$(ag -q validate keygen ci)`), so `--quiet` must
/// print the bare value and nothing else; without `--quiet` and with `--json` the full output is unchanged.
#[test]
fn quiet_prints_the_bare_key_identity_and_fencing_token() {
    let a = Ag::new();
    let key = a.ag(&["-q", "validate", "keygen", "ci"]).ok().stdout();
    assert_eq!(key.trim().len(), 64, "a bare public key: {key:?}");
    assert!(key.trim().chars().all(|c| c.is_ascii_hexdigit()));
    assert!(a.ag(&["validate", "keygen", "ci2"]).ok().stdout().contains("Add this to your policy"));
    assert_eq!(a.ag(&["--json", "validate", "keygen", "ci3"]).ok().json()["identity"], "service:ci3");

    a.ag(&["agent", "register", "bot"]).ok();
    let who = a.ag(&["-q", "agent", "start", "bot"]).ok().stdout();
    assert!(who.trim().starts_with("agent:bot#run_") && !who.trim().contains(' '), "{who:?}");
    assert!(a.ag(&["agent", "start", "bot"]).ok().stdout().contains("Act as:"));

    let token = a.ag(&["-q", "lease", "acquire", "src/**", "--exclusive", "--ttl", "30m"]).ok().stdout();
    assert!(token.trim().parse::<u64>().is_ok(), "a bare fencing token: {token:?}");
    let full = a.ag(&["lease", "acquire", "docs/**", "--ttl", "30m"]).ok().stdout();
    assert!(full.contains("fencing token:"), "{full}");
}

#[test]
fn verbose_explains_itself_on_stderr_only() {
    let a = Ag::new();
    let r = a.ag(&["-v", "status"]).ok();
    assert!(r.stderr().contains("[ag] repository"), "{}", r.stderr());
    assert!(r.stderr().contains("acting as user:alice (from AG_IDENTITY)"), "{}", r.stderr());
    assert!(r.stderr().contains("exit code 0"), "{}", r.stderr());
    assert!(!r.stdout().contains("[ag]"), "diagnostics must not pollute stdout");
    // JSON stays parsable with --verbose.
    let j = a.ag(&["-v", "--json", "status"]).ok();
    assert!(j.json()["workspace"] == "default");
}

#[test]
fn the_global_repo_option_works_from_anywhere() {
    let a = Ag::new();
    a.land_first_change();
    let elsewhere = tempfile::tempdir().unwrap();
    let root = a.root();
    let r = run_in(elsewhere.path(), a.cfg.path(), "user:alice", &["-C", root.to_str().unwrap(), "log"], &[]).ok();
    assert!(r.stdout().contains("initial import"));
    let r = run_in(elsewhere.path(), a.cfg.path(), "user:alice", &["log", "--repo", root.to_str().unwrap()], &[]).ok();
    assert!(r.stdout().contains("initial import"));
    let r = run_in(elsewhere.path(), a.cfg.path(), "user:alice", &["status"], &[]).code_is(1);
    assert!(r.stderr().contains("not an AgentGit repository"), "{}", r.stderr());
    // `-C` also names where `init` creates a repository.
    let fresh = elsewhere.path().join("fresh");
    run_in(
        elsewhere.path(),
        a.cfg.path(),
        "user:alice",
        &["-C", fresh.to_str().unwrap(), "init", "--namespace", "x/y"],
        &[],
    )
    .ok();
    assert!(fresh.join(".ag/config.json").is_file());
}

#[test]
fn errors_say_what_to_do_next_and_machine_readers_get_json() {
    let a = Ag::new();
    for (args, needle) in [
        (&["integrate", "nope"][..], "ag proposal list"),
        (&["proposal", "show", "nope"], "ag proposal list"),
        (&["stream", "show", "nope"], "ag stream list"),
        (&["lease", "release", "nope"], "ag lease list"),
        (&["op", "show", "nope"], "ag op list"),
        (&["unpin", "nope"], "ag pins"),
        (&["cat", "README.md"], "@:PATH"),
        (&["checkout", "main", "/tmp/never-created-by-this-test"], "ag snapshot"),
        (&["--as", "bogus", "log"], "ag help identities"),
        (&["workspace", "remove", "nope"], "ag workspace list"),
    ] {
        let r = a.ag(args).code_is(1);
        let err = r.stderr();
        assert!(err.starts_with("error: "), "{args:?}: {err}");
        assert!(err.contains("hint: ") && err.contains(needle), "{args:?} should hint at {needle:?}: {err}");
        // The same error for a machine: one JSON line on stderr, nothing on stdout.
        let mut with_json = vec!["--json"];
        with_json.extend_from_slice(args);
        let j = a.ag(&with_json).code_is(1);
        assert_eq!(j.stdout(), "", "{args:?}");
        let v: serde_json::Value =
            serde_json::from_str(j.stderr().trim()).unwrap_or_else(|e| panic!("{args:?}: {e}: {}", j.stderr()));
        assert_eq!(v["error"]["exit_code"], 1);
        assert!(v["error"]["message"].as_str().unwrap().len() > 3);
        assert!(v["error"]["hint"].as_str().unwrap().contains(needle), "{v}");
    }
    // An error that already says what to do is not given a second hint.
    let outside = tempfile::tempdir().unwrap();
    let r = run_in(outside.path(), a.cfg.path(), "user:alice", &["log"], &[]).code_is(1);
    assert!(r.stderr().contains("run `ag init`") && !r.stderr().contains("hint:"), "{}", r.stderr());
}

#[test]
fn a_cause_that_the_message_already_contains_is_printed_once() {
    let a = Ag::new();
    a.write("bad.json", r#"{"bogus": 1}"#);
    let err = a.ag(&["policy", "set", "bad.json"]).code_is(1).stderr();
    assert_eq!(err.matches("unknown field `bogus`").count(), 1, "{err}");
    assert!(err.contains("ag policy example"), "{err}");
}

#[test]
fn empty_listings_explain_themselves_on_stderr_without_touching_stdout() {
    let a = Ag::new();
    for (args, needle) in [
        (&["proposal", "list"][..], "ag propose"),
        (&["agent", "list"], "ag agent register"),
        (&["task", "list"], "ag task create"),
        (&["lease", "list"], "ag lease acquire"),
        (&["pins"], "ag pin"),
        (&["queue", "list"], "ag queue add"),
    ] {
        let r = a.ag(args).ok();
        assert_eq!(r.stdout(), "", "{args:?}");
        assert!(r.stderr().contains(needle), "{args:?}: {}", r.stderr());
        // ... and not with --json or --quiet.
        assert_eq!(a.ag(&[&["--json"][..], args].concat()).ok().stderr(), "", "{args:?}");
        assert_eq!(a.ag(&[&["-q"][..], args].concat()).ok().stderr(), "", "{args:?}");
    }
}

#[test]
fn closing_stdout_early_is_not_an_error() {
    let a = Ag::new();
    let big = "x".repeat(8 * 1024 * 1024);
    a.write("big.txt", &big);
    a.ag(&["snapshot", "-m", "big"]).ok();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ag"))
        .current_dir(a.dir.path())
        .env("AG_IDENTITY", "user:alice")
        .env("AG_CONFIG", a.cfg.path().join("config.json"))
        .args(["cat", "@:big.txt"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = [0u8; 16];
    child.stdout.as_mut().unwrap().read_exact(&mut first).unwrap();
    drop(child.stdout.take()); // like `ag cat … | head -c 16`
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "", "a closed pipe must not print a panic");
}

#[test]
fn help_topics_are_pages_and_commands_still_have_their_help() {
    let a = Ag::new();
    for topic in ["workflow", "policy", "leases", "identities", "revisions", "exit-codes", "json", "config"] {
        let r = a.ag(&["help", topic]).ok();
        assert!(r.stdout().lines().count() > 10, "{topic}: {}", r.stdout());
        // The first line is the title and there is no ANSI when piped.
        assert!(!r.stdout().contains(ESC));
    }
    // `help policy` is the concept page, `policy --help` the command.
    assert!(a.ag(&["help", "policy"]).ok().stdout().contains("Policy: what must be true"));
    assert!(a.ag(&["policy", "--help"]).ok().stdout().contains("Show or change stream policy"));
    // Commands: any depth.
    assert!(a.ag(&["help", "integrate"]).ok().stdout().contains("Usage: ag integrate"));
    assert!(a.ag(&["help", "stream", "create"]).ok().stdout().contains("Usage: ag stream create"));
    // No arguments: the overview and the topic list.
    let overview = a.ag(&["help"]).ok().stdout();
    assert!(overview.contains("Common workflow:") && overview.contains("Help topics (ag help TOPIC):"), "{overview}");
    assert!(overview.contains("exit-codes"));
    // Unknown things are an error that lists the topics.
    let e = a.ag(&["help", "frobnicate"]).code_is(1).stderr();
    assert!(e.contains("workflow") && e.contains("leases"), "{e}");
}

#[test]
fn config_shows_sets_and_explains_where_values_come_from() {
    let a = Ag::new();
    let list = a.ag(&["--json", "config", "list"]).ok().json();
    let get = |k: &str| list.as_array().unwrap().iter().find(|e| e["key"] == k).cloned().unwrap_or_default();
    assert_eq!(get("identity")["value"], "user:alice");
    assert_eq!(get("identity")["source"], "AG_IDENTITY");
    assert_eq!(get("repo.namespace")["value"], "acme/demo");
    assert_eq!(get("color")["value"], "auto");

    // Set, get, precedence: --as > AG_IDENTITY > config > default.
    a.ag(&["config", "set", "identity", "user:carol"]).ok();
    let file = std::fs::read_to_string(a.cfg.path().join("config.json")).unwrap();
    assert!(file.contains("user:carol"));
    let no_env = |args: &[&str]| {
        // Removing AG_IDENTITY needs a custom command.
        let mut c = Command::new(env!("CARGO_BIN_EXE_ag"));
        c.current_dir(a.dir.path())
            .env_remove("AG_IDENTITY")
            .env("AG_CONFIG", a.cfg.path().join("config.json"))
            .args(args);
        common::Run { out: c.output().unwrap() }
    };
    assert_eq!(no_env(&["config", "get", "identity"]).ok().stdout().trim(), "user:carol");
    assert!(
        no_env(&["--json", "config", "get", "identity"]).ok().json()["source"]
            .as_str()
            .unwrap()
            .contains("user configuration file")
    );
    assert_eq!(no_env(&["--as", "user:dave", "config", "get", "identity"]).ok().stdout().trim(), "user:dave");
    assert_eq!(
        a.ag(&["config", "get", "identity"]).ok().stdout().trim(),
        "user:alice",
        "the environment beats the file"
    );
    a.ag(&["config", "unset", "identity"]).ok();
    assert!(!std::fs::read_to_string(a.cfg.path().join("config.json")).unwrap().contains("carol"));

    // Validation and unknown keys.
    assert!(a.ag(&["config", "set", "identity", "carol"]).code_is(1).stderr().contains("invalid identity"));
    assert!(a.ag(&["config", "set", "color", "purple"]).code_is(1).stderr().contains("auto, always or never"));
    assert!(a.ag(&["config", "set", "editor", "vim"]).code_is(1).stderr().contains("identity, color"));
    assert!(a.ag(&["config", "get", "nonsense"]).code_is(1).stderr().contains("ag config list"));
    // `path` names the files.
    let p = a.ag(&["--json", "config", "path"]).ok().json();
    assert!(p["user"]["path"].as_str().unwrap().ends_with("config.json"));
    // Compared as a path, not as text: Windows prints the separator as a backslash.
    assert!(std::path::Path::new(p["repository"]["config"].as_str().unwrap()).ends_with(".ag/config.json"));

    // A damaged user file is an error that names the file, not silently ignored.
    std::fs::write(a.cfg.path().join("config.json"), "{ not json").unwrap();
    let e = no_env(&["status"]).code_is(1).stderr();
    assert!(e.contains("config.json") && e.contains("not valid JSON"), "{e}");
    // ... but an identity from the environment does not need the file.
    a.ag(&["status"]).ok();
}

#[test]
fn credentials_are_reported_by_source_never_by_value() {
    let a = Ag::new();
    a.ag(&["remote", "add", "origin", "http://127.0.0.1:9/acme/demo"]).ok();
    let secret = "agt_super_secret_value_1234567890";
    let r = a.run_env("user:alice", &["config", "list"], &[("AG_TOKEN", secret)]).ok();
    assert!(r.stdout().contains("environment variable AG_TOKEN"), "{}", r.stdout());
    assert!(!r.stdout().contains(secret) && !r.stderr().contains(secret));
    let r = a.ag(&["config", "list"]).ok();
    assert!(
        r.stdout().contains("remote.origin.credential") && r.stdout().contains("no credential for remote"),
        "{}",
        r.stdout()
    );
}

#[test]
fn version_command_reports_the_schema_and_protocol() {
    let a = Ag::bare();
    let v = a.ag(&["--json", "version"]).ok().json();
    assert_eq!(v["json_schema"], 1);
    assert_eq!(v["protocol"], "agwp.v1");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert!(a.ag(&["--version"]).ok().stdout().starts_with("ag "));
}
