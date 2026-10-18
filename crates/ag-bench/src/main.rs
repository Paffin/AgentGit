//! `ag-bench` — measurements for AgentGit.
//!
//! * `ag-bench engine`: the engine's hot paths (publish, integrate, three-way merge with rename detection, tree
//!   diff, blame, manifest decode) at 1k / 10k / 100k files. See `docs/architecture/performance.md`.
//! * `ag-bench load`: a load generator that starts a real `ag-server` process and drives it with real HTTP/JSON
//!   and gRPC clients. See `docs/operations/performance.md`.
//!
//! Both print what they measured with the conditions of the run and can write the same as JSON. Neither asserts a
//! target: the numbers are reported, not judged.

mod engine;
mod load;
mod report;
mod stats;
mod tmp;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "ag-bench", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Benchmark the engine in-process.
    Engine {
        /// Repository sizes in files, comma separated.
        #[arg(long, value_delimiter = ',', default_value = "1000,10000,100000")]
        sizes: Vec<usize>,
        /// Samples per repeated operation.
        #[arg(long, default_value_t = 7)]
        samples: usize,
        /// Stop sampling an operation after this many seconds (never below 3 samples).
        #[arg(long, default_value_t = 60)]
        budget_secs: u64,
        /// Storage under test.
        #[arg(long, value_enum, default_value = "memory")]
        backend: engine::Backend,
        /// Only operations whose name contains this text (the setup always runs).
        #[arg(long, default_value = "")]
        filter: String,
        /// Write the raw results here as JSON.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Write the results table here as Markdown.
        #[arg(long)]
        markdown: Option<PathBuf>,
    },
    /// Load-test a real ag-server process.
    Load(load::Args),
}

#[derive(Serialize)]
struct EngineReport<'a> {
    environment: report::Environment,
    backend: engine::Backend,
    samples: usize,
    rows: &'a [engine::Row],
}

fn engine_markdown(
    env: &report::Environment,
    backend: engine::Backend,
    sizes: &[usize],
    rows: &[engine::Row],
) -> String {
    let mut ops: Vec<&str> = Vec::new();
    for r in rows {
        if !ops.contains(&r.op.as_str()) {
            ops.push(&r.op);
        }
    }
    let mut header = vec!["operation".to_string()];
    header.extend(sizes.iter().map(|n| format!("{n} files")));
    let header: Vec<&str> = header.iter().map(String::as_str).collect();
    let body: Vec<Vec<String>> = ops
        .iter()
        .map(|op| {
            let mut line = vec![(*op).to_string()];
            for n in sizes {
                line.push(match rows.iter().find(|r| r.op == *op && r.files == *n) {
                    Some(r) if r.summary.n > 1 => format!(
                        "{} ({}–{})",
                        report::ms(r.summary.p50_ms),
                        report::ms(r.summary.min_ms),
                        report::ms(r.summary.max_ms)
                    ),
                    Some(r) => report::ms(r.summary.p50_ms),
                    None => "–".into(),
                });
            }
            line
        })
        .collect();
    format!(
        "{}\n\nBackend: {backend:?}. Milliseconds: median (min–max) of the samples; a single number is one sample.\n\n{}",
        env.markdown(),
        report::table(&header, &body)
    )
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Engine { sizes, samples, budget_secs, backend, filter, json, markdown } => {
            let env = report::Environment::capture();
            if !env.release_build {
                eprintln!(
                    "warning: a debug build; the numbers are not comparable. Use `cargo run --release -p ag-bench`."
                );
            }
            let opts = engine::Options {
                sizes: sizes.clone(),
                samples,
                budget: Duration::from_secs(budget_secs),
                backend,
                filter,
            };
            let rows = engine::run(&opts)?;
            let md = engine_markdown(&env, backend, &sizes, &rows);
            println!("\n{md}");
            if let Some(p) = markdown {
                std::fs::write(&p, &md).with_context(|| format!("writing {}", p.display()))?;
            }
            if let Some(p) = json {
                let rep = EngineReport { environment: env, backend, samples, rows: &rows };
                std::fs::write(&p, serde_json::to_vec_pretty(&rep)?)
                    .with_context(|| format!("writing {}", p.display()))?;
            }
            Ok(())
        }
        Command::Load(args) => load::run(args),
    }
}
