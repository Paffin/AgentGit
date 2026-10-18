//! Shared command context: the open repository, who is acting, output mode.

use std::path::{Path, PathBuf};

use ag_integrate::workdir::{Platform, adaptations};
use ag_integrate::{Layout, Repository};
use ag_tree::Tree;
use ag_types::Identity;
use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::Cli;
use crate::ui;

/// Everything a command needs.
pub struct Ctx {
    /// The open repository.
    pub repo: Repository,
    /// The acting identity.
    pub me: Identity,
    /// Emit JSON instead of text.
    pub json: bool,
    /// `--quiet`: confirmations and hints are not printed.
    pub quiet: bool,
    /// Repository root (the working directory of the default workspace).
    pub root: PathBuf,
}

/// Where the acting identity came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySource {
    /// `--as`.
    Flag,
    /// `AG_IDENTITY`.
    Env,
    /// `identity` in the user-level configuration file.
    Config,
    /// `user:$USER` (nothing was configured).
    Default,
}

impl IdentitySource {
    /// A phrase for messages.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Flag => "--as",
            Self::Env => "AG_IDENTITY",
            Self::Config => "the user configuration file",
            Self::Default => "default: user:$USER",
        }
    }
}

/// Resolves the acting identity and says where it came from: `--as`, `AG_IDENTITY`, the `identity` setting of
/// `ag config`, then `user:$USER`.
pub fn identity_with_source(cli: &Cli) -> Result<(Identity, IdentitySource)> {
    let (raw, source) = if let Some(i) = cli.identity.clone() {
        (i, IdentitySource::Flag)
    } else if let Some(i) = std::env::var("AG_IDENTITY").ok().filter(|s| !s.is_empty()) {
        (i, IdentitySource::Env)
    } else if let Some(i) = crate::userconf::load()?.get("identity").map(str::to_string) {
        (i, IdentitySource::Config)
    } else {
        let login =
            ["USER", "LOGNAME", "USERNAME"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()));
        (default_identity(login.as_deref()), IdentitySource::Default)
    };
    let id = Identity::new(raw.clone())
        .map_err(|e| anyhow!("invalid identity '{raw}' (from {}): {e}", source.describe()))?;
    Ok((id, source))
}

/// Resolves the acting identity (see [`identity_with_source`]).
pub fn identity(cli: &Cli) -> Result<Identity> {
    identity_with_source(cli).map(|(i, _)| i)
}

/// `user:<login>` with every character an identity cannot hold (a space in `John Smith`, non-ASCII letters,
/// a `DOMAIN\` prefix) replaced by `_`; `user:unknown` without a login name.
fn default_identity(login: Option<&str>) -> String {
    let user = login.unwrap_or("unknown");
    let clean: String =
        user.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("user:{clean}")
}

/// Tells the user, on stderr, what writing `tree` to this machine's file system adapts: symlinks that became
/// plain files, executable flags that cannot be applied. Silent where nothing is adapted (Linux, macOS).
pub fn note_adaptations(tree: &Tree) {
    for line in adaptations(tree, &Platform::host()).describe() {
        eprintln!("note: {line}");
    }
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
        let (me, source) = identity_with_source(cli)?;
        ui::verbose(format!("repository {} (namespace {})", root.display(), repo.config().namespace));
        ui::verbose(format!("acting as {me} (from {})", source.describe()));
        Ok(Self { repo, me, json: cli.json, quiet: cli.quiet, root })
    }

    /// Prints data the user asked for: JSON, or the human text. Never suppressed by `--quiet`.
    pub fn out<T: Serialize>(&self, value: &T, human: impl FnOnce() -> String) -> Result<()> {
        emit(self.json, value, human)
    }

    /// Prints a confirmation ("Snapshot abc in workspace 'default'"). In text mode `--quiet` suppresses it, since
    /// the exit code already says it worked; JSON mode always prints, because JSON is data.
    pub fn done<T: Serialize>(&self, value: &T, human: impl FnOnce() -> String) -> Result<()> {
        if self.quiet && !self.json {
            return Ok(());
        }
        emit(self.json, value, human)
    }

    /// Prints the result of creating something (`id` is its identifier): a confirmation normally, only the bare
    /// identifier with `--quiet` in text mode, so that `id=$(ag -q propose -t x)` works.
    pub fn created<T: Serialize>(&self, id: &str, value: &T, human: impl FnOnce() -> String) -> Result<()> {
        if self.quiet && !self.json {
            ui::print_line(id);
            return Ok(());
        }
        emit(self.json, value, human)
    }

    /// A note for a person on standard error, such as what to do about an empty listing. Not printed in JSON
    /// mode or with `--quiet`, and never part of standard output, so pipelines are unaffected.
    pub fn note(&self, msg: impl AsRef<str>) {
        if !self.json {
            ui::hint(msg);
        }
    }

    /// The row cap for a listing command: `0` means everything. (An unlimited query is a very large limit rather
    /// than `usize::MAX`, which some SQL back ends read as a negative number.)
    pub fn cap(limit: usize) -> usize {
        if limit == 0 { i64::MAX as usize } else { limit }
    }

    /// Tells the reader that a listing stopped at its cap. It goes to standard error even with `--json`, where a
    /// script would otherwise take the newest `limit` rows for all of them.
    pub fn note_cap(&self, shown: usize, limit: usize, what: &str) {
        if limit != 0 && shown >= limit {
            ui::hint(format!("showing the newest {limit} {what}; use -n 0 for all of them"));
        }
    }

    /// The default stream name.
    pub fn default_stream(&self) -> String {
        self.repo.default_stream().to_string()
    }
}

/// Prints JSON, or sanitized and (if allowed) coloured human text, on standard output.
pub fn emit<T: Serialize>(json: bool, value: &T, human: impl FnOnce() -> String) -> Result<()> {
    if json {
        ui::print_line(&serde_json::to_string_pretty(value)?);
    } else {
        let text = human();
        if !text.is_empty() {
            ui::print_line(&ui::finish(&sanitize_for_terminal(text.trim_end()), ui::settings().color_out));
        }
    }
    Ok(())
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
    fn default_identity_uses_the_login_name_and_cleans_it() {
        assert_eq!(default_identity(Some("alice")), "user:alice");
        assert_eq!(default_identity(Some("John Smith")), "user:John_Smith");
        assert_eq!(default_identity(Some("CORP\\jdoe")), "user:CORP_jdoe");
        assert_eq!(default_identity(Some("Юрий")), "user:____");
        assert_eq!(default_identity(None), "user:unknown");
        for login in ["alice", "John Smith", "CORP\\jdoe", "Юрий", "a.b-c_d"] {
            assert!(Identity::new(default_identity(Some(login))).is_ok(), "{login}");
        }
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
