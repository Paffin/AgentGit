//! The user-level configuration file (`ag config`).
//!
//! One small JSON file per user, holding personal defaults that are not part of any repository:
//!
//! ```json
//! { "identity": "user:alice", "color": "auto" }
//! ```
//!
//! Location: `$AG_CONFIG` (a file), else `$XDG_CONFIG_HOME/agentgit/config.json`, else
//! `~/.config/agentgit/config.json` (`%APPDATA%\agentgit\config.json` where `HOME` is unset).
//! Repository settings live in `.ag/config.json` and are not edited here.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

/// The settings `ag config set` can store, with what they accept.
pub const KEYS: &[(&str, &str)] = &[
    ("identity", "who you act as when neither --as nor AG_IDENTITY is given, e.g. user:alice"),
    ("color", "auto, always or never (--color and NO_COLOR are described in `ag help config`)"),
];

/// Where the user-level file lives (it need not exist).
pub fn path() -> Option<PathBuf> {
    let nonempty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if let Some(p) = nonempty("AG_CONFIG") {
        return Some(PathBuf::from(p));
    }
    if let Some(x) = nonempty("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(x).join("agentgit").join("config.json"));
    }
    if let Some(h) = nonempty("HOME") {
        return Some(PathBuf::from(h).join(".config").join("agentgit").join("config.json"));
    }
    nonempty("APPDATA").map(|a| PathBuf::from(a).join("agentgit").join("config.json"))
}

/// The parsed file.
#[derive(Debug, Default, Clone)]
pub struct UserConfig {
    map: Map<String, Value>,
}

impl UserConfig {
    /// A stored string setting.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).and_then(Value::as_str)
    }

    /// Validates and stores a setting.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        validate(key, value)?;
        self.map.insert(key.to_string(), Value::String(value.to_string()));
        Ok(())
    }

    /// Removes a setting; was it present?
    pub fn unset(&mut self, key: &str) -> bool {
        self.map.remove(key).is_some()
    }
}

/// Checks that `key` is a setting `ag config set` can store.
pub fn check_key(key: &str) -> Result<()> {
    if KEYS.iter().any(|(k, _)| *k == key) {
        return Ok(());
    }
    bail!(
        "unknown setting '{key}'; settable keys: {} (see `ag config list`)",
        KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
    )
}

/// Checks that `key` is a known setting and `value` is acceptable for it.
pub fn validate(key: &str, value: &str) -> Result<()> {
    check_key(key)?;
    match key {
        "identity" => {
            ag_types::Identity::new(value.to_string()).map_err(|e| anyhow!("invalid identity '{value}': {e}"))?;
        }
        "color" => {
            if crate::ui::ColorMode::parse(value).is_none() {
                bail!("invalid color '{value}': use auto, always or never");
            }
        }
        _ => unreachable!("check_key accepts only the keys above"),
    }
    Ok(())
}

/// Reads the file; a missing file is an empty configuration, a damaged one is an error that names it.
pub fn load() -> Result<UserConfig> {
    let Some(p) = path() else { return Ok(UserConfig::default()) };
    match std::fs::read(&p) {
        Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(UserConfig::default()),
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => Ok(UserConfig { map }),
            Ok(_) => bail!(
                "{} must hold a JSON object; fix or delete it (`ag config path` prints the location)",
                p.display()
            ),
            Err(e) => bail!(
                "{} is not valid JSON ({e}); fix or delete it (`ag config path` prints the location)",
                p.display()
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserConfig::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", p.display())),
    }
}

/// Like [`load`], but a damaged or unreadable file reads as empty (used where failing would only hide the
/// real message, such as colouring an error).
pub fn load_lenient() -> UserConfig {
    load().unwrap_or_default()
}

/// Writes the file, creating its directory.
pub fn save(cfg: &UserConfig) -> Result<PathBuf> {
    let p = path()
        .ok_or_else(|| anyhow!("cannot locate a configuration directory: set HOME, XDG_CONFIG_HOME or AG_CONFIG"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&Value::Object(cfg.map.clone()))?)
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("cannot replace {}", p.display()))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_keys_with_valid_values_are_accepted() {
        let mut c = UserConfig::default();
        c.set("identity", "user:alice").unwrap();
        c.set("color", "never").unwrap();
        assert_eq!(c.get("identity"), Some("user:alice"));
        assert!(c.set("identity", "alice").is_err());
        assert!(c.set("color", "purple").is_err());
        let e = c.set("editor", "vim").unwrap_err().to_string();
        assert!(e.contains("identity, color"), "{e}");
        assert!(c.unset("color") && !c.unset("color"));
    }
}
