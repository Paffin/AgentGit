//! The operator hardening page (docs/operations/hardening.md) is held to the server it describes:
//!
//! * every setting in its tables exists, and the default the page states is the default the server has;
//! * every test and file the page cites as evidence exists, so renaming a test or moving a file fails here instead
//!   of leaving the page pointing at nothing;
//! * the one control the page leans on that no other test reached - the bearer token on `/metrics` and the start-up
//!   warning when there is none - is exercised on the real binary.
#![allow(missing_docs)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use ag_server::config::ServerConfig;
use common::Http;
use serde_json::Value;

const PAGE: &str = "docs/operations/hardening.md";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn page() -> String {
    std::fs::read_to_string(repo_root().join(PAGE)).unwrap_or_else(|e| panic!("{PAGE}: {e}"))
}

/// The table rows of the page whose first cell is a backticked setting (`section.key`): (key, default cell).
fn settings(page: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in page.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(str::trim).collect();
        let key = cells[0].trim_matches('`');
        if key.contains('.') && !key.contains('/') && cells.len() >= 3 {
            out.push((key.to_string(), cells[1].trim_matches('`').to_string()));
        }
    }
    out
}

#[test]
fn every_setting_on_the_page_exists_and_has_the_default_the_page_states() {
    let defaults = serde_json::to_value(ServerConfig::default()).unwrap();
    let rows = settings(&page());
    for (key, stated) in &rows {
        let (section, name) = key.split_once('.').unwrap();
        let actual = defaults
            .get(section)
            .and_then(|s| s.get(name))
            .unwrap_or_else(|| panic!("{PAGE}: `{key}` is not a setting of the server"));
        let stated: Value = serde_json::from_str(stated)
            .unwrap_or_else(|e| panic!("{PAGE}: the default of `{key}` ({stated}) is not JSON: {e}"));
        assert_eq!(&stated, actual, "{PAGE}: the default of `{key}`");
    }
    assert!(rows.len() >= 28, "the page lists {} settings: the table parser found too few", rows.len());
}

fn rust_and_python_sources(dir: &Path, out: &mut String) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | "node_modules" | ".git") {
                rust_and_python_sources(&path, out);
            }
        } else if name.ends_with(".rs") || name.ends_with(".py") {
            out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
}

#[test]
fn every_test_and_file_the_page_cites_exists() {
    let page = page();
    let mut code = String::new();
    rust_and_python_sources(&repo_root().join("crates"), &mut code);
    rust_and_python_sources(&repo_root().join("scripts"), &mut code);

    // Where evidence is cited: the last cell of a table row, and prose outside the tables. The other cells hold
    // settings, defaults and advice, whose backticked words (`allow_insecure_remote`) are not tests.
    let mut cited: Vec<(&str, usize)> = Vec::new(); // (text, minimum underscores for a word to count as a test name)
    for line in page.lines() {
        if let Some(row) = line.strip_prefix("| ") {
            if let Some(evidence) = row.trim_end().trim_end_matches('|').rsplit('|').next() {
                cited.push((evidence, 2));
            }
        } else {
            cited.push((line, 3));
        }
    }

    let (mut tests, mut files) = (0, 0);
    for (text, min_underscores) in cited {
        for token in text.split('`').skip(1).step_by(2) {
            let is_name = token.len() >= 20
                && token.matches('_').count() >= min_underscores
                && token.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
            let is_path = !token.contains(char::is_whitespace)
                && token.contains('/')
                && !token.starts_with(['/', '{'])
                && token.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b));
            if is_name {
                let defined = code.contains(&format!("fn {token}(")) || code.contains(&format!("def {token}("));
                assert!(defined, "{PAGE} cites the test `{token}`, which does not exist");
                tests += 1;
            } else if is_path {
                assert!(repo_root().join(token).exists(), "{PAGE} cites the file `{token}`, which does not exist");
                files += 1;
            }
        }
    }
    assert!(tests >= 20 && files >= 6, "the page cites {tests} tests and {files} files: the scan found too few");
}

#[cfg(unix)]
struct Process {
    child: std::process::Child,
    port: u16,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

#[cfg(unix)]
impl Process {
    /// A real `ag-server serve`, with the metrics token variable set or removed.
    async fn start(listen_ip: &str, insecure_remote: bool, metrics_token: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let config = dir.path().join("ag.toml");
        std::fs::write(
            &config,
            format!(
                "[server]\nlisten = \"{listen_ip}:{port}\"\ndata_dir = {:?}\nallow_insecure_remote = {insecure_remote}\n",
                dir.path().join("data")
            ),
        )
        .unwrap();
        let log = dir.path().join("server.log");
        let out = std::fs::File::create(&log).unwrap();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ag-server"));
        command
            .args(["serve", "--config"])
            .arg(&config)
            .env("AG_BOOTSTRAP_TOKEN", common::ADMIN_TOKEN)
            .env_remove("AG_METRICS_TOKEN")
            .env_remove("RUST_LOG") // this test asserts the server's default startup warning
            .stdout(out.try_clone().unwrap())
            .stderr(out);
        if let Some(t) = metrics_token {
            command.env("AG_METRICS_TOKEN", t);
        }
        let mut child = command.spawn().unwrap();
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Self { child, port, log, _dir: dir };
            }
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("the server did not start; its log:\n{}", std::fs::read_to_string(&log).unwrap_or_default());
    }

    fn client(&self, token: &str) -> Http {
        Http { base: format!("http://127.0.0.1:{}", self.port), token: token.into() }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn stop(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.log()
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_need_the_bearer_token_when_one_is_set_and_the_start_up_warns_when_it_is_not() {
    // With a token: no token, a wrong token and the right one on loopback, where no warning is due.
    let secured = Process::start("127.0.0.1", false, Some("metrics-secret-0123456789")).await;
    assert_eq!(secured.client("").get("/metrics").await.status, 401);
    assert_eq!(secured.client("wrong-secret-0123456789").get("/metrics").await.status, 401);
    let ok = secured.client("metrics-secret-0123456789").get("/metrics").await;
    assert_eq!(ok.status, 200);
    assert!(String::from_utf8_lossy(&ok.body).contains("agwp_"), "a Prometheus page");
    // the API token is not the metrics token
    assert_eq!(secured.client(common::ADMIN_TOKEN).get("/metrics").await.status, 401);
    let log = secured.stop();
    assert!(!log.contains("without a token"), "{log}");
    assert!(!log.contains("metrics-secret"), "the token must never be logged: {log}");

    // Without one, listening beyond loopback (behind a TLS terminator): open, and the server says so at start-up.
    let open = Process::start("0.0.0.0", true, None).await;
    assert_eq!(open.client("").get("/metrics").await.status, 200);
    let log = open.stop();
    assert!(log.contains("/metrics is served on 0.0.0.0:") && log.contains("without a token"), "{log}");
}
