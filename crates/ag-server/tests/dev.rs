//! `ag-server dev` as a real process: it makes its own data directory, administrator, demo repository and policy,
//! listens on the loopback interface only, and a second start reuses the data and replaces the credential.
#![allow(missing_docs)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use serde_json::Value;

struct Dev {
    child: Child,
    base: String,
    banner: String,
    token: String,
}

impl Dev {
    fn start(data: &Path, extra: &[&str]) -> Dev {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ag-server"))
            .env_remove("AG_BOOTSTRAP_TOKEN")
            .env_remove("RUST_LOG")
            .env("NO_COLOR", "1")
            .arg("dev")
            .arg("--data-dir")
            .arg(data)
            .args(["--port", "0"])
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start ag-server dev");
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut banner = String::new();
        for line in lines.by_ref() {
            let line = line.expect("banner line");
            banner.push_str(&line);
            banner.push('\n');
            if line.starts_with("Credential expires") {
                break;
            }
        }
        // Keep reading so the child never blocks on a full pipe.
        std::thread::spawn(move || for _ in lines {});
        let base = banner
            .lines()
            .find_map(|l| l.trim().strip_prefix("ag clone "))
            .and_then(|c| c.strip_suffix("/demo/welcome"))
            .unwrap_or_else(|| panic!("no clone command in the banner:\n{banner}"))
            .to_string();
        let token = std::fs::read_to_string(data.join("dev/token")).expect("token file").trim().to_string();
        Dev { child, base, banner, token }
    }

    fn get(&self, token: &str, path: &str) -> (u16, Value) {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let mut r = agent
            .get(&format!("{}{path}", self.base))
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .unwrap();
        let status = r.status().as_u16();
        (status, r.body_mut().read_json().unwrap_or(Value::Null))
    }

    fn stop(mut self) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            Command::new("kill").args(["-INT", &self.child.id().to_string()]).status().unwrap();
        }
        #[cfg(not(unix))]
        {
            self.child.kill().unwrap();
        }
        self.child.wait().unwrap()
    }
}

#[test]
fn one_command_makes_everything_and_the_banner_says_what_it_is() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("fresh/data"); // neither it nor its parent exists
    let dev = Dev::start(&data, &[]);

    // The banner: what this mode is, how it differs from production, the link and the exact commands.
    let b = &dev.banner;
    assert!(b.contains("not for real work") && b.contains("loopback only") && b.contains("plain HTTP"), "{b}");
    assert!(
        b.contains("lives 8 hours") && b.contains("replaced at every start") && b.contains("`agents` policy preset"),
        "{b}"
    );
    assert!(
        b.contains(&format!(
            "ag login {} --token-file {}",
            dev.base,
            data.canonicalize().unwrap().join("dev/token").display()
        )),
        "{b}"
    );
    assert!(b.contains(&format!("ag clone {}/demo/welcome", dev.base)), "{b}");
    assert!(dev.base.starts_with("http://127.0.0.1:"), "{}", dev.base);
    // This build may or may not embed the Web UI; when it does the link carries the credential in the fragment.
    if b.contains("Open in a browser") {
        assert!(b.contains(&format!("{}/repos/demo/welcome#token={}", dev.base, dev.token)), "{b}");
    } else {
        assert!(b.contains("no Web UI embedded"), "{b}");
    }

    // The credential is a real one, for an administrator, in a file only its owner can read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(data.join("dev/token")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(data.join("dev")).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(
            std::fs::metadata(&data).unwrap().permissions().mode() & 0o777,
            0o700,
            "a directory it made is private"
        );
    }
    let (status, who) = dev.get(&dev.token, "/v1/whoami");
    assert_eq!(status, 200);
    assert_eq!(who["identity"], "user:dev");
    assert_eq!(who["roles"], serde_json::json!(["*:admin"]));
    let ttl_ms = who["expires_at_ms"].as_i64().unwrap() - chrono_free_now_ms();
    assert!(
        (7 * 3600 * 1000..=8 * 3600 * 1000 + 60_000).contains(&ttl_ms),
        "the credential lives about eight hours, not {ttl_ms} ms"
    );
    assert_eq!(dev.get("agt_wrong_wrong_wrong_0123456789", "/v1/whoami").0, 401);

    // The demo repository has sample files and the preset's policy.
    let (status, tree) = dev.get(&dev.token, "/v1/repos/demo/welcome/tree/list?rev=main");
    assert_eq!(status, 200);
    let names: Vec<&str> = tree["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["README.md", "SECURITY.md", "docs", "src"]);
    let (_, policy) = dev.get(&dev.token, "/v1/repos/demo/welcome/policy?stream=main");
    let stored = ag_policy::Policy::parse(policy["document"].as_str().unwrap()).unwrap();
    let preset = ag_policy::presets::Preset::Agents
        .build(&ag_policy::presets::PresetOptions { admins: vec!["user:dev".into()], ci: None })
        .unwrap();
    assert_eq!(stored, preset);
    assert_eq!(policy["version"], 1);

    // Ctrl-C is a clean stop.
    assert!(dev.stop().success());
}

#[test]
fn a_second_start_keeps_the_data_and_replaces_the_credential() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let first = Dev::start(&data, &[]);
    let (_, repo1) = first.get(&first.token, "/v1/repos/demo/welcome");
    // Someone changes the policy; the next start must not put the preset back.
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let doc = serde_json::json!({"approvals": {"min": 3}}).to_string();
    let r = agent
        .put(&format!("{}/v1/repos/demo/welcome/policy", first.base))
        .header("Authorization", &format!("Bearer {}", first.token))
        .send_json(serde_json::json!({"stream": "main", "document": doc}))
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let old_token = first.token.clone();
    assert!(first.stop().success());

    let second = Dev::start(&data, &[]);
    assert_ne!(second.token, old_token, "the credential is replaced at every start");
    assert_eq!(second.get(&old_token, "/v1/whoami").0, 401, "and the old one is revoked");
    assert_eq!(second.get(&second.token, "/v1/whoami").0, 200);
    let (_, repo2) = second.get(&second.token, "/v1/repos/demo/welcome");
    assert_eq!(repo2["changeset_count"], repo1["changeset_count"], "the sample content is not landed twice");
    assert_eq!(repo2["created_at_ms"], repo1["created_at_ms"]);
    let (_, policy) = second.get(&second.token, "/v1/repos/demo/welcome/policy?stream=main");
    assert_eq!(policy["version"], 2, "your policy change survived");
    assert_eq!(ag_policy::Policy::parse(policy["document"].as_str().unwrap()).unwrap().approvals.min, 3);
    assert!(second.stop().success());
}

#[test]
fn it_refuses_a_public_address_unless_that_is_asked_for_and_says_what_to_do_about_a_busy_port() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ag-server"))
            .arg("dev")
            .arg("--data-dir")
            .arg(&data)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    for host in ["0.0.0.0", "192.0.2.1", "::"] {
        let o = run(&["--host", host, "--port", "0"]);
        assert!(!o.status.success(), "{host} must be refused");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("loopback") && err.contains("--allow-non-loopback"), "{host}: {err}");
    }
    assert!(!data.exists(), "a refused start leaves nothing behind");

    // A port that is taken.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let o = run(&["--port", &port]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains(&format!("127.0.0.1:{port}")) && err.contains("--port"), "{err}");
}

#[test]
fn a_non_loopback_address_is_possible_when_asked_for_explicitly() {
    let tmp = tempfile::tempdir().unwrap();
    let dev = Dev::start(&tmp.path().join("data"), &["--host", "0.0.0.0", "--allow-non-loopback"]);
    assert!(dev.base.starts_with("http://0.0.0.0:"), "{}", dev.base);
    let port = dev.base.rsplit(':').next().unwrap();
    let (status, _) = Dev { base: format!("http://127.0.0.1:{port}"), ..dev }.get_with_self();
    assert_eq!(status, 200);
}

fn chrono_free_now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

impl Dev {
    fn get_with_self(self) -> (u16, Value) {
        let r = self.get(&self.token, "/v1/whoami");
        self.stop();
        r
    }
}
