//! The user's saved logins (`ag login`): one credential per server, in a private file next to the user
//! configuration (`credentials.json`, mode 0600; on Windows restricted to the current user).
//!
//! It is the last place `ag` looks for a credential. An environment variable (`AG_TOKEN`, `AG_TOKEN_<REMOTE>`) or a
//! credential stored with a remote (`--token-stdin` of `ag clone` / `ag remote add`) takes precedence, so a script that
//! says which credential to use is never overruled by what a person saved earlier.
//!
//! ```json
//! { "servers": { "http://127.0.0.1:8080": { "token": "agt_...", "identity": "user:dev", "saved_at_ms": 1760000000000 } } }
//! ```

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// What is kept for one server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Login {
    /// The credential (a secret).
    pub token: String,
    /// Who the server said it belongs to when it was saved.
    #[serde(default)]
    pub identity: String,
    /// When the server said it expires (milliseconds since the epoch; 0 = unknown).
    #[serde(default)]
    pub expires_at_ms: i64,
    /// When it was saved.
    #[serde(default)]
    pub saved_at_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    servers: BTreeMap<String, Login>,
}

/// Where the file lives: beside the user configuration (`ag config path`).
pub fn path() -> Option<PathBuf> {
    crate::userconf::path().map(|p| p.with_file_name("credentials.json"))
}

/// The key of a server: `scheme://host[:port]`, lower case, without a path or a trailing slash. Anything after the
/// authority (a repository path such as `/acme/web`) is ignored, so a remote's URL and the server URL given to
/// `ag login` find the same entry.
pub fn server_key(url: &str) -> Result<String> {
    let (scheme, rest) =
        url.trim().split_once("://").ok_or_else(|| anyhow!("'{url}' is not a URL like http://host:8080"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        bail!("unsupported scheme '{scheme}' (use http or https)");
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase();
    if authority.is_empty() || authority.contains('@') {
        bail!("'{url}' needs a host (and no user name: the credential is a token)");
    }
    Ok(format!("{scheme}://{authority}"))
}

fn load() -> Result<File> {
    let Some(p) = path() else { return Ok(File::default()) };
    match std::fs::read(&p) {
        Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(File::default()),
        Ok(b) => {
            match ag_crypto::private_file::check_owner_only(&p) {
                Ok(()) => {}
                Err(ag_crypto::private_file::Insecure::Accessible) => {
                    bail!("{} is readable by other users; run `chmod 600 {}`", p.display(), p.display())
                }
                Err(ag_crypto::private_file::Insecure::Io(e)) => {
                    return Err(e).with_context(|| format!("cannot read {}", p.display()));
                }
            }
            serde_json::from_slice(&b)
                .with_context(|| format!("{} is damaged; delete it and log in again", p.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", p.display())),
    }
}

fn store(f: &File) -> Result<PathBuf> {
    let p = path()
        .ok_or_else(|| anyhow!("cannot locate a configuration directory: set HOME, XDG_CONFIG_HOME or AG_CONFIG"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let tmp = p.with_extension("json.tmp");
    let mut file = ag_crypto::private_file::create(&tmp, false)?;
    file.write_all(&serde_json::to_vec_pretty(f)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, &p).with_context(|| format!("cannot replace {}", p.display()))?;
    Ok(p)
}

/// The saved login for the server a URL points at, if any.
pub fn find(url: &str) -> Result<Option<Login>> {
    let key = server_key(url)?;
    Ok(load()?.servers.remove(&key))
}

/// Saves (or replaces) the login for a server; returns the file written.
pub fn save(url: &str, login: Login) -> Result<PathBuf> {
    let mut f = load()?;
    f.servers.insert(server_key(url)?, login);
    store(&f)
}

/// Forgets the login for a server; was there one?
pub fn forget(url: &str) -> Result<bool> {
    let mut f = load()?;
    let had = f.servers.remove(&server_key(url)?).is_some();
    if had {
        store(&f)?;
    }
    Ok(had)
}

/// The servers with a saved login, with who they are (never the token).
pub fn list() -> Result<Vec<(String, Login)>> {
    Ok(load()?.servers.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_and_a_repository_url_have_the_same_key() {
        let k = "http://127.0.0.1:8080";
        for u in [
            "http://127.0.0.1:8080",
            "http://127.0.0.1:8080/",
            "HTTP://127.0.0.1:8080/acme/web",
            "http://127.0.0.1:8080/a?b#c",
        ] {
            assert_eq!(server_key(u).unwrap(), k, "{u}");
        }
        assert_ne!(server_key("https://127.0.0.1:8080").unwrap(), k);
        assert_ne!(server_key("http://127.0.0.1:8081").unwrap(), k);
        for bad in ["127.0.0.1:8080", "ftp://x", "http://", "http://user:pw@host"] {
            assert!(server_key(bad).is_err(), "{bad}");
        }
    }
}
