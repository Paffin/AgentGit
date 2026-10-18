//! `ag-server admin` honours the server's configuration: a repository created offline gets the storage template
//! (PostgreSQL schema, S3 prefix) of `[repos.storage]`, like one created through the API. Before this was fixed,
//! `admin create-repo` silently made a local SQLite repository, so the Compose and Kubernetes stacks that follow
//! docs/operations/deployment.md ended up with data on the container's volume instead of in PostgreSQL and S3.
#![allow(missing_docs)]

use std::path::Path;
use std::process::{Command, Output};

fn admin(config_env: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ag-server"));
    cmd.env_remove("AG_SERVER_CONFIG")
        .env_remove("AG_DATA_DIR")
        .env("AWS_ACCESS_KEY_ID", "test")
        .env("AWS_SECRET_ACCESS_KEY", "test-secret-0123456789");
    if let Some(c) = config_env {
        cmd.env("AG_SERVER_CONFIG", c);
    }
    cmd.arg("admin").args(args).output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn write_config(dir: &Path, data: &Path, storage: &str) -> std::path::PathBuf {
    let path = dir.join("ag-server.toml");
    std::fs::write(&path, format!("[server]\ndata_dir = {:?}\n\n{storage}", data.display().to_string())).unwrap();
    path
}

#[test]
fn the_data_directory_comes_from_the_configuration_and_a_flag_overrides_it() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("from-config");
    let cfg = write_config(tmp.path(), &data, "");

    let o = admin(Some(&cfg), &["create-repo", "acme/web"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(data.join("repos/acme/web").is_dir(), "the repository lives under the configured data directory");
    let o = admin(Some(&cfg), &["repos"]);
    assert!(text(&o).contains("acme/web"), "{}", text(&o));

    // --config on the command line works the same as the environment variable.
    let o = Command::new(env!("CARGO_BIN_EXE_ag-server"))
        .env_remove("AG_SERVER_CONFIG")
        .args(["admin", "--config"])
        .arg(&cfg)
        .arg("repos")
        .output()
        .unwrap();
    assert!(text(&o).contains("acme/web"), "{}", text(&o));

    // An explicit --data-dir wins.
    let other = tmp.path().join("explicit");
    let o = admin(Some(&cfg), &["--data-dir", other.to_str().unwrap(), "create-repo", "acme/api"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(other.join("repos/acme/api").is_dir());
    assert!(!data.join("repos/acme/api").exists());
}

#[test]
fn a_storage_template_applies_to_repositories_created_offline() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    // An S3 bucket nobody listens at: creating a repository against it must fail, which proves that the template
    // was used. (Without the template the same command succeeds, below.)
    let cfg = write_config(
        tmp.path(),
        &data,
        "[repos.storage.objects]\nkind = \"s3\"\nendpoint = \"http://127.0.0.1:1\"\nbucket = \"nowhere\"\npath_style = true\n",
    );
    let o = admin(Some(&cfg), &["create-repo", "acme/web"]);
    assert!(!o.status.success(), "creating a repository on an unreachable S3 bucket must fail:\n{}", text(&o));
    let o = admin(Some(&cfg), &["repos"]);
    assert!(!text(&o).contains("acme/web"), "a failed create must not leave a registered repository:\n{}", text(&o));

    // The same data directory without the configuration: a plain local repository, as before.
    let o = admin(None, &["--data-dir", data.to_str().unwrap(), "create-repo", "acme/web"]);
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn a_missing_configuration_file_is_an_error_not_a_silent_default() {
    let tmp = tempfile::tempdir().unwrap();
    let o = admin(Some(&tmp.path().join("nope.toml")), &["repos"]);
    assert!(!o.status.success());
}

#[test]
fn offline_administration_applies_the_configured_quota_defaults_stream_and_credential_lifetimes() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let cfg = write_config(
        tmp.path(),
        &data,
        "[repos]\ndefault_stream = \"trunk\"\n\n[limits]\nmax_total_object_bytes = 123456\nrepo_max_streams = 4\n\
         max_open_proposals = 9\n\n[auth]\nmax_ttl_secs = 600\ndefault_ttl_secs = 300\nmax_agent_ttl_secs = 60\ndefault_agent_ttl_secs = 30\n",
    );

    // A repository created here is the one the API would create: same default stream, same starting quota.
    let o = admin(Some(&cfg), &["create-repo", "acme/web"]);
    assert!(o.status.success(), "{}", text(&o));
    let db = rusqlite::Connection::open(data.join("control.db")).unwrap();
    let (stream, quota): (String, String) = db
        .query_row("SELECT default_stream, quota_json FROM repos WHERE name = 'acme/web'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(stream, "trunk", "repos.default_stream, not a hardcoded main");
    let quota: serde_json::Value = serde_json::from_str(&quota).unwrap();
    assert_eq!(
        (quota["max_total_object_bytes"].as_u64(), quota["max_streams"].as_u64(), quota["max_open_proposals"].as_u64()),
        (Some(123456), Some(4), Some(9)),
        "{quota}"
    );
    // An explicit flag still wins.
    let o = admin(Some(&cfg), &["create-repo", "acme/api", "--default-stream", "dev"]);
    assert!(o.status.success(), "{}", text(&o));

    // A minted credential gets the configured default lifetime and cannot pass the configured cap.
    let o = admin(Some(&cfg), &["mint", "--subject", "user:alice"]);
    assert!(o.status.success() && text(&o).contains("expires in 300s"), "{}", text(&o));
    let o = admin(Some(&cfg), &["mint", "--subject", "agent:bot"]);
    assert!(o.status.success() && text(&o).contains("expires in 30s"), "{}", text(&o));
    let o = admin(Some(&cfg), &["mint", "--subject", "user:alice", "--ttl", "601"]);
    assert!(!o.status.success() && text(&o).contains("at most 600"), "{}", text(&o));
    let o = admin(Some(&cfg), &["mint", "--subject", "agent:bot", "--ttl", "61"]);
    assert!(!o.status.success() && text(&o).contains("at most 60"), "{}", text(&o));
    let o = admin(Some(&cfg), &["mint", "--subject", "user:alice", "--ttl", "120"]);
    assert!(o.status.success() && text(&o).contains("expires in 120s"), "{}", text(&o));

    // A role is granted on patterns that compile, like through the API.
    let o = admin(Some(&cfg), &["grant", "--subject", "user:alice", "--repo", "acme/[", "--role", "writer"]);
    assert!(!o.status.success() && text(&o).contains("bad pattern"), "{}", text(&o));
    let o = admin(Some(&cfg), &["grant", "--subject", "user:alice", "--repo", "acme/*", "--role", "writer"]);
    assert!(o.status.success(), "{}", text(&o));
}
