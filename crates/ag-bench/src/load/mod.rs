//! Load generator for `ag-server`.
//!
//! Each scenario starts its own real `ag-server` process (fresh data directory, SQLite control database and
//! repositories, filesystem objects — the default deployment), creates users with real credentials through the
//! admin API, and drives it with real clients over HTTP/JSON and gRPC from threads of this process. The
//! changesets are built the way a client builds them, uploaded as objects, proposed and integrated. The generator and the server share the machine, so the numbers include the
//! generator's own CPU use; they are a statement about that machine, not about a deployment.

mod client;
mod scenarios;
mod server;

pub use server::random_hex;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Args as ClapArgs;
use serde::Serialize;

pub use client::Proto;
use scenarios::ScenarioResult;

use crate::report::{self, Environment};
use crate::stats::Summary;

/// How big a run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Profile {
    /// Seconds per scenario and a handful of clients: proves the harness works end to end (CI).
    Smoke,
    /// The sizes the published numbers use.
    Full,
}

/// `ag-bench load`
#[derive(ClapArgs)]
pub struct Args {
    /// The `ag-server` executable to start (default: `ag-server` next to this executable).
    #[arg(long)]
    server_bin: Option<PathBuf>,
    /// Sizes and durations (individual flags override).
    #[arg(long, value_enum, default_value = "full")]
    profile: Profile,
    /// Scenarios to run, comma separated: baseline, streams, contention, conflict, sse, ratelimit, shedding.
    #[arg(long, value_delimiter = ',', default_value = "baseline,streams,contention,conflict,sse,ratelimit,shedding")]
    scenarios: Vec<String>,
    /// Protocols for the scenarios that can use either.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "http,grpc")]
    protocols: Vec<Proto>,
    /// Seconds each scenario drives load.
    #[arg(long)]
    duration_secs: Option<u64>,
    /// Concurrent writers (one thread and one credential each).
    #[arg(long)]
    workers: Option<usize>,
    /// Concurrent event subscribers in the `sse` scenario (half HTTP/SSE, half gRPC).
    #[arg(long)]
    subscribers: Option<usize>,
    /// Directory under which the servers keep their data (default: the system temporary directory). Point it at a
    /// memory-backed file system to see how much of the write latency is the disk.
    #[arg(long)]
    data_parent: Option<PathBuf>,
    /// Write the raw results here as JSON.
    #[arg(long)]
    json: Option<PathBuf>,
    /// Write the Markdown tables here.
    #[arg(long)]
    markdown: Option<PathBuf>,
}

/// Resolved settings of a run.
#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub duration: Duration,
    pub workers: usize,
    pub subscribers: usize,
    pub rate_limit: u32,
    pub rate_burst: u32,
    pub shed_limit: usize,
    pub data_parent: Option<PathBuf>,
}

#[derive(Serialize)]
struct Report<'a> {
    environment: Environment,
    plan: &'a Plan,
    results: &'a [ScenarioResult],
}

pub fn run(args: Args) -> Result<()> {
    let bin = match args.server_bin {
        Some(p) => p,
        None => {
            let me = std::env::current_exe()?;
            me.with_file_name(if cfg!(windows) { "ag-server.exe" } else { "ag-server" })
        }
    };
    if !bin.is_file() {
        bail!(
            "{} not found; build it (`cargo build --release -p agentgit-server`) or pass --server-bin",
            bin.display()
        );
    }
    let (d, w, s) = match args.profile {
        Profile::Smoke => (3, 4, 8),
        Profile::Full => (20, 16, 100),
    };
    let plan = Plan {
        duration: Duration::from_secs(args.duration_secs.unwrap_or(d)),
        workers: args.workers.unwrap_or(w).max(1),
        subscribers: args.subscribers.unwrap_or(s),
        rate_limit: 50,
        rate_burst: 100,
        shed_limit: 4,
        data_parent: args.data_parent,
    };
    let env = Environment::capture();
    if !env.release_build {
        eprintln!("warning: a debug build of the generator; the numbers are not comparable.");
    }
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    let mut results = Vec::new();
    for name in &args.scenarios {
        let one = |proto: Proto| scenarios::run_one(name, proto, &bin, &plan, rt.handle());
        match name.as_str() {
            "baseline" | "streams" | "contention" | "conflict" | "ratelimit" | "shedding" => {
                for p in &args.protocols {
                    results.push(one(*p).with_context(|| format!("scenario {name} over {p:?}"))?);
                }
            }
            "sse" => results.push(one(Proto::Http).with_context(|| "scenario sse")?),
            other => bail!("unknown scenario '{other}'"),
        }
    }
    let md = markdown(&env, &plan, &results);
    println!("\n{md}");
    if let Some(p) = args.markdown {
        std::fs::write(&p, &md).with_context(|| format!("writing {}", p.display()))?;
    }
    if let Some(p) = args.json {
        let rep = Report { environment: env, plan: &plan, results: &results };
        std::fs::write(&p, serde_json::to_vec_pretty(&rep)?).with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(())
}

fn row(name: &str, s: &Summary) -> Vec<String> {
    vec![
        name.into(),
        s.n.to_string(),
        report::ms(s.p50_ms),
        report::ms(s.p90_ms),
        report::ms(s.p99_ms),
        report::ms(s.max_ms),
    ]
}

fn counts(m: &BTreeMap<String, u64>) -> String {
    if m.is_empty() {
        "none".into()
    } else {
        m.iter().map(|(k, v)| format!("`{k}` {v}")).collect::<Vec<_>>().join(", ")
    }
}

fn markdown(env: &Environment, plan: &Plan, results: &[ScenarioResult]) -> String {
    let mut out = format!(
        "{}\n\nPlan: {} s per scenario, {} writers, {} subscribers (sse), rate-limit scenario {} req/s burst {}, shedding scenario \
         `max_concurrent_requests = {}`. Servers keep their data under {}. Generator and server share the machine.\n",
        env.markdown(),
        plan.duration.as_secs(),
        plan.workers,
        plan.subscribers,
        plan.rate_limit,
        plan.rate_burst,
        plan.shed_limit,
        plan.data_parent
            .as_deref()
            .map_or("the system temporary directory".to_string(), |p| format!("`{}`", p.display()))
    );
    for r in results {
        out.push_str(&format!("\n### {} over {:?}\n\n{}\n\n", r.name, r.protocol, r.description));
        out.push_str(&format!(
            "{} clients for {:.1} s: **{} completed ({:.1}/s)**; server CPU {:.1} s ({:.0}% of one core), peak RSS {:.0} MiB.\n\n",
            r.clients,
            r.elapsed_secs,
            r.completed,
            r.per_second,
            r.server.as_ref().map_or(0.0, |s| s.cpu_secs),
            r.server.as_ref().map_or(0.0, |s| s.cpu_secs) / r.elapsed_secs.max(0.001) * 100.0,
            r.server.as_ref().map_or(0.0, |s| s.peak_rss_mib),
        ));
        let rows: Vec<Vec<String>> = r.steps.iter().map(|(k, v)| row(k, v)).collect();
        if !rows.is_empty() {
            out.push_str(&report::table(&["step (ms)", "n", "p50", "p90", "p99", "max"], &rows));
            out.push('\n');
        }
        out.push_str(&format!("Outcomes: {}.\n\nFailures: {}.\n", counts(&r.outcomes), counts(&r.errors)));
        for (k, v) in &r.notes {
            out.push_str(&format!("\n- {k}: {v}"));
        }
        if !r.notes.is_empty() {
            out.push('\n');
        }
        if !r.sample_errors.is_empty() {
            out.push_str("\nFirst failures: ");
            out.push_str(&r.sample_errors.iter().map(|e| format!("`{e}`")).collect::<Vec<_>>().join("; "));
            out.push('\n');
        }
    }
    out
}
