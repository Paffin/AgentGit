//! `ag login` against a real in-process `ag-server`: a credential saved once is found by `clone`, `fetch` and
//! `remote`; a wrong one is never saved; an environment variable still wins; the file is private.
#![allow(missing_docs)]

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use ag_server::authz::Role;
use common::{Run, Server, run_in};

/// Runs `ag` with `input` on standard input.
fn run_with_stdin(cwd: &Path, cfg: &Path, args: &[&str], input: &str, env: &[(&str, &str)]) -> Run {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ag"));
    c.current_dir(cwd)
        .env_remove("AG_TOKEN")
        .env_remove("NO_COLOR")
        .env("AG_IDENTITY", "user:alice")
        .env("AG_CONFIG", cfg.join("config.json"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    // Invalid arguments can make the child exit before it reads stdin. Its status and stderr
    // are still asserted by the caller; a closed input pipe must not race those assertions.
    if let Err(error) = child.stdin.take().unwrap().write_all(input.as_bytes()) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "{error}");
    }
    Run { out: child.wait_with_output().unwrap() }
}

struct Fixture {
    server: Server,
    token: String,
    work: tempfile::TempDir,
    cfg: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let server = Server::start();
    server.create_repo("acme/web");
    let token = server.user("user:alice", "acme/web", Role::Admin);
    Fixture { server, token, work: tempfile::tempdir().unwrap(), cfg: tempfile::tempdir().unwrap() }
}

impl Fixture {
    fn ag(&self, args: &[&str]) -> Run {
        run_in(self.work.path(), self.cfg.path(), "user:alice", args, &[])
    }
    fn ag_env(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        run_in(self.work.path(), self.cfg.path(), "user:alice", args, env)
    }
    fn url(&self) -> String {
        self.server.url.clone()
    }
    fn login(&self) {
        run_with_stdin(
            self.work.path(),
            self.cfg.path(),
            &["login", &self.url(), "--token-stdin"],
            &format!("{}\n", self.token),
            &[],
        )
        .ok();
    }
}

#[test]
fn a_saved_login_is_all_clone_and_remote_need() {
    let f = fixture();
    let url = f.url();
    // Without a login there is no credential, and the error says how to get one.
    let r = f.ag(&["clone", &format!("{url}/acme/web"), "web"]).code_is(1);
    assert!(r.stderr().contains(&format!("ag login {url}")), "{}", r.stderr());

    let out = run_with_stdin(
        f.work.path(),
        f.cfg.path(),
        &["--json", "login", &url, "--token-stdin"],
        &format!("{}\n", f.token),
        &[],
    )
    .ok();
    let j = out.json();
    assert_eq!(j["server"], url.as_str());
    assert_eq!(j["identity"], "user:alice");
    assert!(!out.stdout().contains(&f.token) && !out.stderr().contains(&f.token), "the credential is never printed");

    f.ag(&["clone", &format!("{url}/acme/web"), "web"]).ok();
    let clone = f.work.path().join("web");
    let who = run_in(&clone, f.cfg.path(), "user:alice", &["--json", "remote", "whoami"], &[]).ok().json();
    assert_eq!(who["identity"], "user:alice");
    run_in(&clone, f.cfg.path(), "user:alice", &["fetch"], &[]).ok();
    // The clone records no secret: the credential stays in the user's file.
    let remotes = std::fs::read_to_string(clone.join(".ag/remotes.json")).unwrap();
    assert!(!remotes.contains(&f.token), "{remotes}");
    assert!(!clone.join(".ag/credentials").exists());
    // `doctor` says where the credential came from (never the credential).
    let doctor = run_in(&clone, f.cfg.path(), "user:alice", &["doctor"], &[]);
    assert!(doctor.stdout().contains("saved login") && !doctor.stdout().contains(&f.token), "{}", doctor.stdout());
}

#[test]
fn the_file_is_private_and_holds_the_login_by_server() {
    let f = fixture();
    f.login();
    let file = f.cfg.path().join("credentials.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["servers"][f.url()]["identity"], "user:alice");
    // A loosened file is refused rather than used.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let r = f.ag(&["clone", &format!("{}/acme/web", f.url()), "web"]).code_is(1);
        assert!(r.stderr().contains("readable by other users"), "{}", r.stderr());
    }
}

#[test]
fn a_wrong_credential_is_refused_and_nothing_is_saved() {
    let f = fixture();
    let r = run_with_stdin(
        f.work.path(),
        f.cfg.path(),
        &["login", &f.url(), "--token-stdin"],
        "agt_not_a_real_token_0123456789\n",
        &[],
    )
    .code_is(1);
    assert!(r.stderr().contains("did not accept the credential"), "{}", r.stderr());
    assert!(!f.cfg.path().join("credentials.json").exists());
    // No source at all (a script has no terminal): an error that names the ways to give one.
    let r = run_with_stdin(f.work.path(), f.cfg.path(), &["login", &f.url()], "", &[]).code_is(1);
    assert!(r.stderr().contains("--token-stdin") && r.stderr().contains("AG_TOKEN"), "{}", r.stderr());
    run_with_stdin(f.work.path(), f.cfg.path(), &["login", "not-a-url", "--token-stdin"], "x\n", &[]).code_is(1);
}

#[test]
fn a_token_file_works_and_a_world_readable_one_is_refused() {
    let f = fixture();
    let file = f.work.path().join("token");
    std::fs::write(&file, format!("{}\n", f.token)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let r = f.ag(&["login", &f.url(), "--token-file", file.to_str().unwrap()]).code_is(1);
        assert!(r.stderr().contains("readable by other users"), "{}", r.stderr());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    f.ag(&["login", &f.url(), "--token-file", file.to_str().unwrap()]).ok();
    f.ag(&["clone", &format!("{}/acme/web", f.url()), "web"]).ok();
}

#[test]
fn an_environment_credential_still_wins_over_the_saved_one() {
    let f = fixture();
    f.login();
    let url = format!("{}/acme/web", f.url());
    // A bad AG_TOKEN is used (and fails) instead of silently falling back to the saved login.
    let r = f.ag_env(&["clone", &url, "bad"], &[("AG_TOKEN", "agt_wrong_wrong_wrong_0123456789")]).code_is(1);
    assert!(!f.work.path().join("bad").exists(), "{}", r.stderr());
    // A good one works with no login at all.
    let g = fixture();
    g.ag_env(&["clone", &format!("{}/acme/web", g.url()), "ok"], &[("AG_TOKEN", g.token.as_str())]).ok();
}

#[test]
fn logout_forgets_and_an_expired_login_says_so() {
    let f = fixture();
    f.login();
    let url = f.url();
    f.ag(&["logout", &format!("{url}/acme/web")]).ok();
    assert!(f.ag(&["clone", &format!("{url}/acme/web"), "web"]).code_is(1).stderr().contains("ag login"));
    f.ag(&["logout", &url]).code_is(1); // nothing left to forget

    // A login whose credential has run out is reported as that, not as a server error.
    let file = f.cfg.path().join("credentials.json");
    std::fs::write(
        &file,
        serde_json::json!({"servers": {url.clone(): {"token": f.token, "identity": "user:alice", "expires_at_ms": 1, "saved_at_ms": 0}}}).to_string(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let r = f.ag(&["clone", &format!("{url}/acme/web"), "web"]).code_is(1);
    assert!(r.stderr().contains("expired") && r.stderr().contains(&format!("ag login {url}")), "{}", r.stderr());

    f.login();
    f.ag(&["logout", "--all"]).ok();
    assert!(!std::fs::read_to_string(&file).unwrap().contains(&f.token));
}

#[test]
fn auth_status_is_useful_offline_and_never_prints_the_saved_secret() {
    let f = fixture();
    let empty = f.ag(&["--json", "auth", "status"]).ok().json();
    assert_eq!(empty["connections"], serde_json::json!([]));
    f.login();
    let cached = f.ag(&["--json", "auth", "status"]).ok();
    assert!(!cached.stdout().contains(&f.token));
    assert_eq!(cached.json()["connections"][0]["state"], "saved");
    assert_eq!(cached.json()["connections"][0]["identity"], "user:alice");
    let checked = f.ag(&["--json", "auth", "status", &f.url(), "--check"]).ok();
    assert!(!checked.stdout().contains(&f.token));
    assert_eq!(checked.json()["connections"][0]["state"], "verified");
    f.ag(&["auth", "status", "https://missing.example"]).code_is(1);
    assert!(f.ag(&["auth", "status"]).ok().stdout().contains("--check"));
}

#[test]
fn auth_status_distinguishes_expired_rejected_and_unreachable_without_changing_credentials() {
    let f = fixture();
    f.login();
    let file = f.cfg.path().join("credentials.json");
    let mut saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let url = f.url();
    saved["servers"][&url]["expires_at_ms"] = 1.into();
    std::fs::write(&file, saved.to_string()).unwrap();
    let expired = f.ag(&["--json", "auth", "status"]).code_is(1);
    assert_eq!(expired.json()["connections"][0]["state"], "expired");
    saved["servers"][&url]["expires_at_ms"] = 0.into();
    saved["servers"][&url]["token"] = "agt_invalid_invalid_0123456789".into();
    std::fs::write(&file, saved.to_string()).unwrap();
    let rejected = f.ag(&["--json", "auth", "status", "--check"]).code_is(1);
    assert_eq!(rejected.json()["connections"][0]["state"], "rejected");
    let stalled = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let offline = format!("http://{}", stalled.local_addr().unwrap());
    saved["servers"][&offline] = saved["servers"][&url].clone();
    std::fs::write(&file, saved.to_string()).unwrap();
    let before = std::fs::read(&file).unwrap();
    let started = std::time::Instant::now();
    let checked = f.ag(&["--json", "auth", "status", &offline, "--check", "--timeout", "1"]).code_is(1);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(checked.json()["connections"][0]["state"], "unreachable");
    assert_eq!(std::fs::read(&file).unwrap(), before, "status must never rewrite saved credentials");
    assert!(!checked.stdout().contains("agt_invalid"));
    f.ag(&["auth", "status", "--check", "--timeout", "0"]).code_is(2);
}
