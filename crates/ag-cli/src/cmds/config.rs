//! `ag config`: where settings come from, and the few that can be stored.
//!
//! Settings are scattered by nature (flags, environment, a user file, the repository's `.ag/config.json`,
//! `.ag/remotes.json`). This command shows the *effective* value of each with its origin, and edits the user
//! file. It never prints a secret: credentials are reported by where they are read from, not by value.

use std::path::PathBuf;

use ag_integrate::{Layout, RepoConfig};
use anyhow::{Result, anyhow, bail};
use clap::Subcommand;
use serde::Serialize;
use serde_json::json;

use crate::cmds::remote;
use crate::ctx::{self, identity_with_source};
use crate::ui::{self, ColorMode};
use crate::{Cli, exit, userconf};

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Show every setting with its effective value and where it comes from.
    List,
    /// Print one setting's effective value (see `list` for the keys).
    Get {
        /// Setting name, for example `identity`.
        key: String,
    },
    /// Store a user-level setting: `identity` or `color`.
    Set {
        /// Setting name.
        key: String,
        /// New value.
        value: String,
    },
    /// Remove a stored user-level setting.
    Unset {
        /// Setting name.
        key: String,
    },
    /// Print the locations of the configuration files.
    Path,
}

/// One effective setting.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Setting name.
    pub key: String,
    /// Effective value (`null` when unset).
    pub value: Option<String>,
    /// Where the value comes from.
    pub source: String,
    /// Can `ag config set` change it?
    pub settable: bool,
}

fn entry(key: &str, value: Option<String>, source: impl Into<String>, settable: bool) -> Entry {
    Entry { key: key.into(), value, source: source.into(), settable }
}

fn repo_root(cli: &Cli) -> Option<PathBuf> {
    let start = cli.repo.clone().or_else(|| std::env::current_dir().ok())?;
    Layout::discover(&start).map(|l| l.root().to_path_buf())
}

/// Collects all effective settings.
pub fn entries(cli: &Cli) -> Result<Vec<Entry>> {
    let user = userconf::load()?;
    let mut v = Vec::new();

    match identity_with_source(cli) {
        Ok((id, src)) => v.push(entry("identity", Some(id.to_string()), src.describe(), true)),
        Err(e) => v.push(entry("identity", None, format!("invalid: {e}"), true)),
    }

    let (color, color_src) = match (cli.color, user.get("color").and_then(ColorMode::parse)) {
        (Some(c), _) => (c.name().to_string(), "--color".to_string()),
        (None, Some(c)) => (c.name().to_string(), "the user configuration file".to_string()),
        (None, None) => {
            let env = ui::ColorEnv::from_process();
            let why = if env.no_color { "default (NO_COLOR is set, so auto means off)" } else { "default" };
            ("auto".to_string(), why.to_string())
        }
    };
    v.push(entry("color", Some(color), color_src, true));
    v.push(entry(
        "user_config",
        userconf::path().map(|p| p.display().to_string()),
        if std::env::var_os("AG_CONFIG").is_some_and(|p| !p.is_empty()) { "AG_CONFIG" } else { "default location" },
        false,
    ));

    if let Some(root) = repo_root(cli) {
        let layout = Layout::new(&root);
        v.push(entry(
            "repo.root",
            Some(root.display().to_string()),
            if cli.repo.is_some() { "--repo" } else { "discovered" },
            false,
        ));
        match std::fs::read(layout.config_path())
            .map_err(anyhow::Error::from)
            .and_then(|b| Ok(serde_json::from_slice::<RepoConfig>(&b)?))
        {
            Ok(cfg) => {
                let src = layout.config_path().display().to_string();
                v.push(entry("repo.namespace", Some(cfg.namespace.to_string()), &src, false));
                v.push(entry("repo.default_stream", Some(cfg.default_stream.to_string()), &src, false));
                let st = serde_json::to_value(cfg.storage())?;
                let kind = |k: &str| st[k]["kind"].as_str().unwrap_or("?").to_string();
                v.push(entry("repo.storage.meta", Some(kind("meta")), &src, false));
                v.push(entry("repo.storage.objects", Some(kind("objects")), &src, false));
            }
            Err(e) => v.push(entry(
                "repo.config",
                None,
                format!("{} is unreadable: {e}", layout.config_path().display()),
                false,
            )),
        }
        let keys_src = if std::env::var_os("AG_KEY_DIR").is_some_and(|d| !d.is_empty()) {
            "AG_KEY_DIR"
        } else {
            "default location"
        };
        v.push(entry("keys.dir", Some(layout.keys_dir().display().to_string()), keys_src, false));
        match remote::load_remotes_if_any(&root) {
            Ok(rs) => {
                for (name, r) in rs {
                    let src = layout.ag_dir().join("remotes.json").display().to_string();
                    v.push(entry(&format!("remote.{name}.url"), Some(r.url.clone()), &src, false));
                    v.push(entry(&format!("remote.{name}.repo"), Some(r.repo.clone()), &src, false));
                    let cred = match remote::credential_source(&root, &name, &r) {
                        Ok((_, from)) => entry(&format!("remote.{name}.credential"), Some(from), "found", false),
                        Err(e) => entry(&format!("remote.{name}.credential"), None, format!("{e}"), false),
                    };
                    v.push(cred);
                }
            }
            Err(e) => v.push(entry("remotes", None, format!("remotes.json is unreadable: {e}"), false)),
        }
    } else {
        v.push(entry("repo.root", None, "not inside an AgentGit repository", false));
        if let Some(d) = std::env::var_os("AG_KEY_DIR").filter(|d| !d.is_empty()) {
            v.push(entry("keys.dir", Some(PathBuf::from(d).display().to_string()), "AG_KEY_DIR", false));
        }
    }
    if let Some(run) = std::env::var("AG_RUN").ok().filter(|s| !s.is_empty()) {
        v.push(entry("run", Some(run), "AG_RUN", false));
    }
    Ok(v)
}

pub fn config(cli: &Cli, cmd: &ConfigCmd) -> Result<u8> {
    match cmd {
        ConfigCmd::List => {
            let es = entries(cli)?;
            ctx::emit(cli.json, &es, || {
                let w = es.iter().map(|e| e.key.len()).max().unwrap_or(0);
                es.iter()
                    .map(|e| {
                        let value = e.value.clone().unwrap_or_else(|| "(unset)".into());
                        format!("{:<w$}  {}  {}", e.key, value, ui::tone(ui::Tone::Dim, format!("[{}]", e.source)))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;
        }
        ConfigCmd::Get { key } => {
            let es = entries(cli)?;
            let e = es
                .iter()
                .find(|e| &e.key == key)
                .ok_or_else(|| anyhow!("unknown setting '{key}'; `ag config list` shows the available ones"))?;
            match &e.value {
                Some(v) => ctx::emit(cli.json, e, || v.clone())?,
                None => bail!("'{key}' is not set ({})", e.source),
            }
        }
        ConfigCmd::Set { key, value } => {
            let mut c = userconf::load()?;
            c.set(key, value)?;
            let path = userconf::save(&c)?;
            if !(cli.quiet && !cli.json) {
                ctx::emit(cli.json, &json!({"key": key, "value": value, "file": path}), || {
                    format!("Set {key} = {value} in {}", path.display())
                })?;
            }
        }
        ConfigCmd::Unset { key } => {
            userconf::check_key(key)?;
            let mut c = userconf::load()?;
            let had = c.unset(key);
            let path = userconf::save(&c)?;
            if !(cli.quiet && !cli.json) {
                ctx::emit(cli.json, &json!({"key": key, "was_set": had, "file": path}), || {
                    if had {
                        format!("Removed {key} from {}", path.display())
                    } else {
                        format!("{key} was not set in {}", path.display())
                    }
                })?;
            }
        }
        ConfigCmd::Path => {
            let user = userconf::path();
            let root = repo_root(cli);
            let value = json!({
                "user": user.as_ref().map(|p| json!({"path": p, "exists": p.is_file()})),
                "repository": root.as_ref().map(|r| {
                    let l = Layout::new(r);
                    json!({"config": l.config_path(), "remotes": l.ag_dir().join("remotes.json"), "keys": l.keys_dir()})
                }),
            });
            ctx::emit(cli.json, &value, || {
                let mut s = match &user {
                    Some(p) => format!(
                        "user        {}{}",
                        p.display(),
                        if p.is_file() { "" } else { "  (does not exist yet)" }
                    ),
                    None => "user        (no location: set HOME, XDG_CONFIG_HOME or AG_CONFIG)".to_string(),
                };
                if let Some(r) = &root {
                    let l = Layout::new(r);
                    s.push_str(&format!(
                        "\nrepository  {}\nremotes     {}\nkeys        {}",
                        l.config_path().display(),
                        l.ag_dir().join("remotes.json").display(),
                        l.keys_dir().display()
                    ));
                }
                s
            })?;
        }
    }
    Ok(exit::OK)
}
