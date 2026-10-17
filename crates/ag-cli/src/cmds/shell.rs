//! Shell completions, man pages and `ag version`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, CommandFactory};
use clap_complete::Shell;
use serde_json::json;

use crate::exit;
use crate::{Cli, JSON_SCHEMA_VERSION, ui};

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// The shell to generate the script for.
    #[arg(value_enum)]
    pub shell: Shell,
}

/// The completion script for `shell`, exactly as `ag completions` prints it.
pub fn completion_script(shell: Shell) -> Vec<u8> {
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, "ag", &mut buf);
    buf
}

pub fn completions(a: &CompletionsArgs) -> Result<u8> {
    ui::print_bytes(&completion_script(a.shell));
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct ManArgs {
    /// Write `ag.1` and one page per subcommand (`ag-stream-list.1`, …) into this directory instead of
    /// printing the top-level page.
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

pub fn man(a: &ManArgs) -> Result<u8> {
    let cmd = Cli::command();
    match &a.dir {
        None => {
            let mut buf = Vec::new();
            clap_mangen::Man::new(cmd).render(&mut buf).context("cannot render the man page")?;
            ui::print_bytes(&buf);
        }
        Some(dir) => {
            std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
            clap_mangen::generate_to(cmd, dir)
                .with_context(|| format!("cannot write man pages to {}", dir.display()))?;
            let n = std::fs::read_dir(dir)?
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().ends_with(".1"))
                .count();
            ui::eprint_line(&format!("Wrote {n} man page(s) to {}", dir.display()));
        }
    }
    Ok(exit::OK)
}

pub fn version(cli: &Cli) -> Result<u8> {
    let v = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": ag_protocol::VERSION,
        "json_schema": JSON_SCHEMA_VERSION,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    });
    crate::ctx::emit(cli.json, &v, || {
        format!(
            "ag {}\nprotocol    {}\njson schema {}\nplatform    {} {}",
            env!("CARGO_PKG_VERSION"),
            ag_protocol::VERSION,
            JSON_SCHEMA_VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    Ok(exit::OK)
}
