//! Read and remove captured execution output through the same contract as the Web UI.
use ag_protocol::v1;
use ag_storage::meta::MetaError;
use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use serde_json::json;

use crate::ctx::Ctx;

/// Options for inspecting one run's captured output.
#[derive(Args, Debug)]
pub struct OutputArgs {
    /// Run identifier.
    run: String,
    /// Use a server instead of this repository (without a NAME: origin).
    #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "origin")]
    remote: Option<String>,
    /// Print one stream. Text output escapes terminal controls; --json preserves exact text.
    #[arg(long, value_enum, conflicts_with_all = ["delete", "json"])]
    stream: Option<OutputStream>,
    /// Permanently remove captured text, retaining the run and audit history. Remote use requires agent.operate.
    #[arg(long)]
    delete: bool,
}

/// A captured process stream.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputStream {
    Stdout,
    Stderr,
}

/// Executes output inspection or explicit deletion.
pub fn run(ctx: &Ctx, args: &OutputArgs) -> Result<()> {
    match (ctx.json, args.stream) {
        (true, Some(_)) => bail!("choose --json or --stream, not both"),
        _ => {}
    }
    if args.delete {
        return delete_output(ctx, args);
    }
    let output = read_output(ctx, args)?;
    if !output.available && args.stream.is_some() {
        bail!("no output saved for run {}; enable capture on its executor", args.run);
    }
    ctx.out(&output, || display_output(&output, args.stream))
}

fn read_output(ctx: &Ctx, args: &OutputArgs) -> Result<v1::RunOutput> {
    match &args.remote {
        Some(remote) => super::remote::run_output(ctx, remote, &args.run, false),
        None => local_output(ctx, &args.run),
    }
}

fn delete_output(ctx: &Ctx, args: &OutputArgs) -> Result<()> {
    if let Some(remote) = &args.remote {
        super::remote::run_output(ctx, remote, &args.run, true)?;
    } else {
        ag_agent::output::delete(ctx.repo.meta(), &args.run, ctx.me.as_str())?;
    }
    ctx.done(&json!({"run_id":args.run,"deleted":true}), || format!("Deleted captured output for {}", args.run))
}

fn local_output(ctx: &Ctx, run: &str) -> Result<v1::RunOutput> {
    let saved = ctx.repo.meta().read(|tx| {
        tx.run(run)?.ok_or_else(|| MetaError::NotFound(format!("run '{run}'")))?;
        ag_agent::output::read(tx, run)
    })?;
    let Some(saved) = saved else { return Ok(v1::RunOutput { run_id: run.into(), ..Default::default() }) };
    Ok(v1::RunOutput {
        run_id: run.into(),
        available: true,
        sequence: saved.sequence,
        stdout: saved.stdout,
        stderr: saved.stderr,
        stdout_bytes: saved.stdout_bytes,
        stderr_bytes: saved.stderr_bytes,
        captured_at_ms: saved.captured_at_ms,
    })
}

fn display_output(output: &v1::RunOutput, stream: Option<OutputStream>) -> String {
    if !output.available {
        return format!("No output saved for run {}. Enable capture on its executor.", output.run_id);
    }
    match stream {
        Some(OutputStream::Stdout) => output.stdout.clone(),
        Some(OutputStream::Stderr) => output.stderr.clone(),
        None => format!(
            "Run {}: {} stdout bytes, {} stderr bytes (captured tails)\n\nStandard output:\n{}\n\nStandard error:\n{}",
            output.run_id, output.stdout_bytes, output.stderr_bytes, output.stdout, output.stderr
        ),
    }
}
