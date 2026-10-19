//! The measurement tools run end to end on tiny inputs: the engine benchmark against the real engine, the load
//! generator against a real `ag-server` process. These are not performance tests (debug builds, a second of load);
//! they prove the tools measure what they say, and the load run doubles as a black-box test of the server's rate
//! limit, load shedding and event fan-out.
#![allow(missing_docs)]

#[cfg(unix)]
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn bench() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ag-bench"))
}

#[cfg(unix)]
/// The `ag-server` binary of the test profile, brought up to date now.
fn server_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("AG_SERVER_BIN") {
        return p.into();
    }
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_ag-bench"));
    let bin = exe.with_file_name(if cfg!(windows) { "ag-server.exe" } else { "ag-server" });
    // Always ask cargo: a binary left over from an older build must not be what is tested (a no-op when fresh).
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "agentgit-server"])
        .status()
        .expect("running cargo to build ag-server");
    assert!(status.success(), "building ag-server failed");
    assert!(bin.is_file(), "{} does not exist after building ag-server", bin.display());
    bin
}

fn run_json(cmd: &mut Command, out: &std::path::Path) -> Value {
    let o = cmd.output().expect("running ag-bench");
    assert!(
        o.status.success(),
        "ag-bench failed:\n{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&std::fs::read(out).expect("the JSON report")).expect("valid JSON")
}

#[test]
fn the_engine_benchmark_measures_every_hot_path() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("engine.json");
    let md = dir.path().join("engine.md");
    let rep = run_json(
        bench().args(["engine", "--sizes", "1000", "--samples", "3", "--json"]).arg(&out).arg("--markdown").arg(&md),
        &out,
    );
    let rows = rep["rows"].as_array().unwrap();
    let ops: Vec<&str> = rows.iter().map(|r| r["op"].as_str().unwrap()).collect();
    for want in [
        "publish: the whole import",
        "integrate: fast-forward",
        "integrate: three-way merge (1 file vs 1 file)",
        "integrate: three-way merge, rename vs edit",
        "merge_trees: 1 file each side",
        "detect_renames",
        "tree diff: 1 changed file",
        "manifest: Tree::decode",
        "blame: one file",
    ] {
        assert!(ops.iter().any(|o| o.starts_with(want)), "no row for '{want}' in {ops:?}");
    }
    for r in rows {
        let s = &r["summary"];
        assert!(s["n"].as_u64().unwrap() >= 1, "{r}");
        assert!(s["p50_ms"].as_f64().unwrap() > 0.0, "a zero-time sample means nothing ran: {r}");
        assert!(s["min_ms"].as_f64().unwrap() <= s["p50_ms"].as_f64().unwrap());
        assert!(s["p50_ms"].as_f64().unwrap() <= s["max_ms"].as_f64().unwrap());
    }
    let table = std::fs::read_to_string(md).unwrap();
    assert!(table.contains("| operation | 1000 files |"), "{table}");
    assert!(rep["environment"]["cpus"].as_u64().unwrap() >= 1);
}

// The generator starts `ag-server` as a process and reads /proc; it is a Unix measurement tool and has not been
// run on Windows.
#[cfg(unix)]
#[test]
fn the_load_generator_drives_a_real_server_over_both_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("load.json");
    let rep = run_json(
        bench()
            .args([
                "load",
                "--profile",
                "smoke",
                "--duration-secs",
                "2",
                "--workers",
                "3",
                "--subscribers",
                "4",
                "--scenarios",
                "streams,contention,conflict,sse,ratelimit,shedding",
                "--server-bin",
            ])
            .arg(server_bin())
            .arg("--json")
            .arg(&out),
        &out,
    );
    let results = rep["results"].as_array().unwrap();
    let find = |name: &str, proto: &str| {
        results
            .iter()
            .find(|r| r["name"] == name && r["protocol"] == proto)
            .unwrap_or_else(|| panic!("no result for {name}/{proto}"))
    };
    let count = |r: &Value, prefix: &str| -> u64 {
        r["outcomes"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(_, v)| v.as_u64().unwrap())
            .sum()
    };

    for proto in ["http", "grpc"] {
        // Different streams never conflict and never need a merge.
        let r = find("streams", proto);
        assert!(r["completed"].as_u64().unwrap() > 0, "{r}");
        assert_eq!(count(r, "integrated (fast_forward)"), r["completed"].as_u64().unwrap(), "{r}");
        assert!(r["errors"].as_object().unwrap().is_empty(), "{r}");

        // One stream: everything that was answered was integrated, and stale bases were merged by the server.
        let r = find("contention", proto);
        assert_eq!(count(r, "integrated"), r["completed"].as_u64().unwrap(), "{r}");
        assert!(count(r, "integrated (merge)") > 0, "three workers on one stream never raced: {r}");

        // One file: someone won, and the losers were told so instead of being dropped or merged silently.
        let r = find("conflict", proto);
        assert!(count(r, "integrated") >= 1, "{r}");
        assert!(r["completed"].as_u64().unwrap() > count(r, "integrated"), "no conflict outcome was reported: {r}");

        // Over the rate limit: rejections are `rate_limited`, the second credential is unaffected.
        let r = find("ratelimit", proto);
        let errors = r["errors"].as_object().unwrap();
        assert!(errors.keys().all(|k| k.contains("rate_limited")), "{r}");
        assert!(!errors.is_empty(), "the hammered credential was never limited: {r}");
        let note = r["notes"]["second credential meanwhile"].as_str().unwrap();
        assert!(note.ends_with("0 rejected"), "{note}");
        assert_eq!(r["notes"]["server alive at the end"], "true");
        let admitted = r["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s[0] == "admitted")
            .map_or(0, |s| s[1]["n"].as_u64().unwrap());
        let allowed = r["allowed_requests"].as_f64().expect("allowed_requests");
        assert!(
            (admitted as f64) <= allowed + 5.0,
            "admitted {admitted} requests, the configuration allows about {allowed}"
        );

        // Beyond the concurrency cap the server answers at once and stays up.
        let r = find("shedding", proto);
        assert_eq!(r["notes"]["server alive at the end"], "true", "{r}");
        assert_eq!(r["notes"]["ready after the run"], "true", "{r}");
    }

    // Every subscriber saw every integration exactly once.
    let r = find("sse", "http");
    let note = r["notes"]["stream.integrate events per subscriber"].as_str().unwrap();
    assert!(note.contains("4 of 4 received all of them"), "{note}");
    assert!(note.contains("0 duplicate"), "{note}");
    assert_eq!(r["notes"]["feeds that failed"], "none");
    assert!(r["steps"].as_array().unwrap().iter().any(|s| s[0] == "event lag, SSE"));
    assert!(r["steps"].as_array().unwrap().iter().any(|s| s[0] == "event lag, gRPC"));
}
