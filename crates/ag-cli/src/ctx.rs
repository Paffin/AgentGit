//! Shared command context: the open repository, who is acting, output mode.

use std::path::{Path, PathBuf};

use ag_integrate::{Layout, Repository};
use ag_types::Identity;
use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::Cli;

/// Everything a command needs.
pub struct Ctx {
    /// The open repository.
    pub repo: Repository,
    /// The acting identity.
    pub me: Identity,
    /// Emit JSON instead of text.
    pub json: bool,
    /// Repository root (the working directory of the default workspace).
    pub root: PathBuf,
}

/// Resolves the acting identity: `--as`, `AG_IDENTITY`, then `user:$USER`.
pub fn identity(cli: &Cli) -> Result<Identity> {
    let raw = cli
        .identity
        .clone()
        .or_else(|| std::env::var("AG_IDENTITY").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| {
            let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_else(|_| "unknown".into());
            let clean: String =
                user.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
            format!("user:{clean}")
        });
    Identity::new(raw.clone()).map_err(|e| anyhow!("invalid identity '{raw}': {e}"))
}

impl Ctx {
    /// Opens the repository containing the current directory (or `--repo`).
    pub fn open(cli: &Cli) -> Result<Self> {
        let start = match &cli.repo {
            Some(p) => p.clone(),
            None => std::env::current_dir().context("cannot read the current directory")?,
        };
        let layout = Layout::discover(&start).ok_or_else(|| {
            anyhow!("not an AgentGit repository (or any parent of {}); run `ag init`", start.display())
        })?;
        let root = layout.root().to_path_buf();
        let repo = Repository::open(&root)?;
        Ok(Self { repo, me: identity(cli)?, json: cli.json, root })
    }

    /// Prints either the JSON form or the human text.
    pub fn out<T: Serialize>(&self, value: &T, human: impl FnOnce() -> String) -> Result<()> {
        if self.json {
            println!("{}", serde_json::to_string_pretty(value)?);
        } else {
            let text = human();
            if !text.is_empty() {
                println!("{}", sanitize_for_terminal(text.trim_end()));
            }
        }
        Ok(())
    }

    /// The default stream name.
    pub fn default_stream(&self) -> String {
        self.repo.default_stream().to_string()
    }
}

/// Neutralizes control characters in text that may contain attacker-controlled
/// data (commit messages, author names, provenance fields, file names).
///
/// A hostile commit message could otherwise carry terminal escape sequences that
/// rewrite earlier output, hide lines, change the window title or, on some
/// terminals, inject input. Newlines and tabs are kept; every other control
/// character (including ESC and the C1 range) is shown as `\u{XXXX}`. JSON output
/// is unaffected, since JSON already escapes control characters.
pub fn sanitize_for_terminal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            out.push_str(&format!("\\u{{{:04x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses durations such as `500ms`, `90s`, `30m`, `12h`, `7d`.
pub fn parse_duration_ms(s: &str) -> Result<i64> {
    let s = s.trim();
    let split =
        s.find(|c: char| !c.is_ascii_digit()).ok_or_else(|| anyhow!("duration '{s}' needs a unit (ms, s, m, h, d)"))?;
    let (num, unit) = s.split_at(split);
    let n: i64 = num.parse().map_err(|_| anyhow!("bad duration '{s}'"))?;
    let mult = match unit {
        "ms" => 1,
        "s" => 1000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(anyhow!("unknown duration unit '{unit}' in '{s}' (use ms, s, m, h, d)")),
    };
    n.checked_mul(mult).ok_or_else(|| anyhow!("duration '{s}' is too large"))
}

/// Shortens a path for display relative to the current directory.
pub fn display_path(p: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|c| p.strip_prefix(&c).ok().map(|r| r.display().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_text_is_defanged() {
        assert_eq!(sanitize_for_terminal("plain\ttext\nline2"), "plain\ttext\nline2");
        assert_eq!(sanitize_for_terminal("\u{1b}[2J\u{1b}]0;pwned\u{7}"), "\\u{001b}[2J\\u{001b}]0;pwned\\u{0007}");
        assert_eq!(sanitize_for_terminal("a\u{9b}b\r"), "a\\u{009b}b\\u{000d}");
        assert_eq!(sanitize_for_terminal("юникод ✓"), "юникод ✓");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
        assert_eq!(parse_duration_ms("90s").unwrap(), 90_000);
        assert_eq!(parse_duration_ms("30m").unwrap(), 1_800_000);
        assert_eq!(parse_duration_ms("2h").unwrap(), 7_200_000);
        assert_eq!(parse_duration_ms("7d").unwrap(), 604_800_000);
        assert!(parse_duration_ms("10").is_err());
        assert!(parse_duration_ms("5x").is_err());
        assert!(parse_duration_ms("h").is_err());
        assert!(parse_duration_ms("99999999999999999999d").is_err());
    }
}
