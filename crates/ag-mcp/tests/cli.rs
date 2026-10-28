//! The command line of `ag-mcp`: `--list-tools`, `--check`, `--version` and the flag errors, run as the real binary.
#![allow(missing_docs)]

mod common;

use common::*;
use serde_json::Value;

const REPO: &str = "acme/web";
/// Arguments, environment and the text the complaint must contain.
type Case<'a> = (Vec<&'a str>, &'a [(&'a str, &'a str)], &'a str);

const NOWHERE: &str = "http://127.0.0.1:1";

fn tool_names_of(stdout: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));
    v["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn version_names_the_binary_and_the_crate_version() {
    let (code, out, _) = run_once(&["--version"], &[]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), format!("ag-mcp {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn list_tools_needs_no_server_and_no_credential_and_follows_the_allow_list() {
    // Nothing listens on port 1 and no credential is set: --list-tools must not touch either.
    let (code, out, err) = run_once(&["--server", NOWHERE, "--list-tools"], &[]);
    assert_eq!(code, 0, "{err}");
    let default = tool_names_of(&out);
    assert!(default.contains(&"read_file".to_string()) && default.contains(&"task_claim".to_string()), "{default:?}");
    assert!(!default.contains(&"integrate".to_string()), "integrate is off by default");

    let (code, out, _) = run_once(&["--server", NOWHERE, "--list-tools", "--allow", "read"], &[]);
    assert_eq!(code, 0);
    let read_only = tool_names_of(&out);
    assert!(read_only.len() < default.len(), "{read_only:?} vs {default:?}");
    assert!(!read_only.contains(&"workspace_write_files".to_string()), "{read_only:?}");
    assert!(read_only.contains(&"read_file".to_string()));

    let (code, out, _) = run_once(&["--server", NOWHERE, "--list-tools", "--enable-integrate"], &[]);
    assert_eq!(code, 0);
    assert!(tool_names_of(&out).contains(&"integrate".to_string()));

    // The listing is what a client sees: the same tools in the same order as `tools/list` of a running process.
    let (_, out, _) = run_once(&["--server", NOWHERE, "--list-tools"], &[]);
    let srv = TestServer::start();
    let mut m = Mcp::start(&srv, "agt_unused", &[]);
    assert_eq!(m.tool_names(), tool_names_of(&out));
}

#[test]
fn check_reports_the_identity_the_scope_and_what_would_be_offered() {
    let srv = TestServer::start();
    srv.repo(REPO);
    let tok = srv.credential("agent:coder", REPO, "writer", 3600, &[]);
    srv.register_agent(&tok, REPO, "coder");
    let (code, out, err) = run_once(&["--server", &srv.url, "--check", "--repo", REPO], &[("AG_MCP_TOKEN", &tok)]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("identity  agent:coder"), "{out}");
    assert!(out.contains(REPO), "the scope names the repository: {out}");
    assert!(out.contains("groups    read,workspace,propose,coordinate,discuss"), "{out}");
    assert!(out.contains("expires   in "), "{out}");
    assert!(!out.contains(&tok) && !err.contains(&tok), "the credential is never printed");
}

#[test]
fn check_fails_with_a_reason_for_a_bad_credential_and_an_unreachable_server() {
    let srv = TestServer::start();
    srv.repo(REPO);
    let (code, _, err) = run_once(&["--server", &srv.url, "--check"], &[("AG_MCP_TOKEN", "agt_not-a-real-credential")]);
    assert_eq!(code, 1);
    assert!(err.contains("rejected the credential"), "{err}");

    let (code, _, err) = run_once(&["--server", NOWHERE, "--check"], &[("AG_MCP_TOKEN", "agt_x")]);
    assert_eq!(code, 1);
    assert!(err.contains("cannot reach the server"), "{err}");
}

#[test]
fn flag_errors_name_the_flag_and_exit_nonzero_without_writing_to_the_protocol_channel() {
    let t = [("AG_MCP_TOKEN", "x")];
    let cases: Vec<Case> = vec![
        (vec!["--server", NOWHERE, "--allow", "admin"], &t, "admin"),
        (vec!["--server", NOWHERE, "--allow", "bogus"], &t, "bogus"),
        (vec!["--server", "not a url"], &t, "--server"),
        (vec!["--server", "ftp://example.com"], &t, "http(s)"),
        (vec!["--server", "http://user:pw@127.0.0.1:1"], &t, "credentials"),
        (vec!["--server", "http://git.example.com"], &t, "plain http"),
        (vec!["--server", NOWHERE, "--max-message-bytes", "10"], &t, "--max-message-bytes"),
        (vec!["--server", NOWHERE, "--max-output-bytes", "10"], &t, "--max-output-bytes"),
        (vec!["--server", NOWHERE, "--max-concurrent", "0"], &t, "--max-concurrent"),
        (vec!["--server", NOWHERE, "--allow", "propose", "--enable-integrate"], &t, "read group"),
        (vec!["--server", NOWHERE, "--ca-file", "/nonexistent/ca.pem"], &t, "--ca-file"),
        (vec!["--server", NOWHERE], &[], "AG_MCP_TOKEN"),
        (vec!["--server", NOWHERE, "--token-file", "/nonexistent/token"], &[], "token"),
    ];
    for (args, envs, needle) in cases {
        let (code, out, err) = run_once(&args, envs);
        assert_ne!(code, 0, "{args:?} should fail");
        assert!(out.is_empty(), "{args:?}: nothing may reach stdout (it is the protocol channel): {out}");
        assert!(err.contains(needle), "{args:?}: stderr should mention `{needle}`: {err}");
    }
    // Without --server (and no AG_MCP_SERVER) clap itself refuses.
    let (code, _, err) = run_once(&["--list-tools"], &[]);
    assert_ne!(code, 0);
    assert!(err.contains("--server"), "{err}");
}

#[test]
fn a_token_file_is_used_and_conflicts_with_a_token_env_flag() {
    let srv = TestServer::start();
    srv.repo(REPO);
    let tok = srv.credential("agent:filer", REPO, "writer", 3600, &[]);
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("token");
    write_file(&f, &format!("{tok}\n"));
    let f = f.to_str().unwrap();
    let (code, out, err) = run_once(&["--server", &srv.url, "--check", "--token-file", f], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("identity  agent:filer"), "{out}");
    let (code, _, err) = run_once(&["--server", &srv.url, "--check", "--token-file", f, "--token-env", "X"], &[]);
    assert_ne!(code, 0);
    assert!(err.contains("cannot be used with"), "{err}");
}

#[test]
fn ca_file_trusts_a_private_ca_and_a_bundle_with_other_certificates_in_it() {
    let local = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    let other = rcgen::generate_simple_self_signed(vec!["elsewhere.example".to_string()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    write_file(&cert, &local.cert.pem());
    write_file(&key, &local.signing_key.serialize_pem());
    let srv = TestServer::start_tls(&cert, &key);
    assert!(srv.url.starts_with("https://"), "{}", srv.url);
    let envs = [("AG_MCP_TOKEN", ADMIN)];

    // The built-in roots do not know this certificate: the connection is refused, not silently accepted.
    let (code, _, err) = run_once(&["--server", &srv.url, "--check"], &envs);
    assert_eq!(code, 1);
    assert!(err.contains("cannot reach the server"), "{err}");

    // With the CA file, it works.
    let ca = cert.to_str().unwrap();
    let (code, out, err) = run_once(&["--server", &srv.url, "--check", "--ca-file", ca], &envs);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("AGWP"), "{out}");

    // --ca-file replaces the built-in roots (a limit documented in the guide); to keep the system roots as well,
    // the operator concatenates them with the private CA into one file. Every certificate of the file is used.
    let bundle = dir.path().join("bundle.pem");
    write_file(&bundle, &format!("{}\n{}", other.cert.pem(), local.cert.pem()));
    let (code, _, err) = run_once(&["--server", &srv.url, "--check", "--ca-file", bundle.to_str().unwrap()], &envs);
    assert_eq!(code, 0, "{err}");

    // A file with the wrong certificate does not authenticate the server.
    let wrong = dir.path().join("wrong.pem");
    write_file(&wrong, &other.cert.pem());
    let (code, _, err) = run_once(&["--server", &srv.url, "--check", "--ca-file", wrong.to_str().unwrap()], &envs);
    assert_eq!(code, 1);
    assert!(err.contains("cannot reach the server"), "{err}");

    // A file without any certificate is a configuration error, named as such.
    let empty = dir.path().join("empty.pem");
    write_file(&empty, "not a certificate\n");
    let (code, _, err) = run_once(&["--server", &srv.url, "--check", "--ca-file", empty.to_str().unwrap()], &envs);
    assert_eq!(code, 1);
    assert!(err.contains("contains no certificate"), "{err}");
}
