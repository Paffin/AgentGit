//! `ag login` and `ag logout`: save a server credential once, and every `ag clone`, `ag fetch`, `ag push` and
//! `ag remote ...` to that server finds it (see `crate::credstore`).

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use ag_client::Client;
use ag_protocol::v1;
use anyhow::{Result, anyhow, bail};
use clap::Args;
use serde_json::json;

use crate::cmds::remote::{Remote, call_timeout, options_for, rt};
use crate::credstore::{self, Login};
use crate::{Cli, exit};

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// The server, e.g. `http://127.0.0.1:8080` (a repository URL works too; only the server part is used).
    pub server: String,
    /// Read the credential from standard input (`echo "$TOKEN" | ag login URL --token-stdin`).
    #[arg(long, conflicts_with = "token_file")]
    pub token_stdin: bool,
    /// Read the credential from a file (`ag-server dev` writes one and prints the command).
    #[arg(long, value_name = "FILE")]
    pub token_file: Option<PathBuf>,
    /// Extra CA certificate (PEM) to trust when checking the credential.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct LogoutArgs {
    /// The server whose saved login to forget.
    pub server: Option<String>,
    /// Forget every saved login.
    #[arg(long, conflicts_with = "server")]
    pub all: bool,
}

/// Where the credential comes from: stdin, a file, `AG_TOKEN`, or (at a terminal) a prompt.
fn read_credential(a: &LoginArgs) -> Result<String> {
    let raw = if a.token_stdin {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else if let Some(f) = &a.token_file {
        ag_crypto::private_file::check_owner_only(f).map_err(|e| match e {
            ag_crypto::private_file::Insecure::Accessible => {
                anyhow!("{} is readable by other users; run `chmod 600 {}`", f.display(), f.display())
            }
            ag_crypto::private_file::Insecure::Io(e) => {
                anyhow::Error::new(e).context(format!("cannot read {}", f.display()))
            }
        })?;
        std::fs::read_to_string(f)?
    } else if let Some(t) = std::env::var("AG_TOKEN").ok().filter(|t| !t.trim().is_empty()) {
        t
    } else if std::io::stdin().is_terminal() {
        eprint!("Paste the credential for {} (input is visible; Ctrl-C to cancel): ", a.server);
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        s
    } else {
        bail!(
            "no credential given: pass --token-stdin or --token-file FILE, or set AG_TOKEN (a script has no terminal to ask on)"
        );
    };
    let token = raw.trim().strip_prefix("Bearer ").unwrap_or(raw.trim()).trim().to_string();
    if token.is_empty() || token.contains(char::is_whitespace) {
        bail!("the credential is empty or contains spaces; paste the whole token and nothing else");
    }
    Ok(token)
}

pub fn login(cli: &Cli, a: &LoginArgs) -> Result<u8> {
    let key = credstore::server_key(&a.server)?;
    let token = read_credential(a)?;
    // Ask the server before saving anything: a mistyped token must not be stored as if it worked.
    let remote =
        Remote { url: key.clone(), repo: String::new(), token_env: None, token_file: None, ca_file: a.ca_file.clone() };
    let who = rt()?.block_on(async {
        let client =
            Client::connect(options_for(&remote, token.clone(), call_timeout()?)).await.map_err(|e| anyhow!("{e}"))?;
        client
            .protocol()
            .whoami(client.req(v1::WhoamiRequest {}))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow!("{key} did not accept the credential: {}", s.message()))
    })?;
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let file = credstore::save(
        &key,
        Login { token, identity: who.identity.clone(), expires_at_ms: who.expires_at_ms, saved_at_ms: now },
    )?;
    let expires = (who.expires_at_ms > 0).then(|| ag_types::Timestamp::from_millis(who.expires_at_ms).to_string());
    crate::ctx::emit(
        cli.json,
        &json!({"server": key, "identity": who.identity, "expires_at": expires, "file": file}),
        || {
            format!(
                "Logged in to {key} as {}{}.\nSaved in {} (readable by you only); `ag clone`, `ag fetch`, `ag push` and `ag remote` to this server use it.",
                who.identity,
                expires.as_deref().map(|e| format!(" (the credential expires {e})")).unwrap_or_default(),
                file.display()
            )
        },
    )?;
    Ok(exit::OK)
}

pub fn logout(cli: &Cli, a: &LogoutArgs) -> Result<u8> {
    let forgotten: Vec<String> = if a.all {
        let all: Vec<String> = credstore::list()?.into_iter().map(|(k, _)| k).collect();
        for k in &all {
            credstore::forget(k)?;
        }
        all
    } else {
        let server = a
            .server
            .as_deref()
            .ok_or_else(|| anyhow!("name the server (`ag logout http://127.0.0.1:8080`) or use --all"))?;
        if !credstore::forget(server)? {
            bail!("there is no saved login for {}", credstore::server_key(server)?);
        }
        vec![credstore::server_key(server)?]
    };
    crate::ctx::emit(cli.json, &json!({"forgotten": forgotten}), || {
        if forgotten.is_empty() {
            "There were no saved logins.".to_string()
        } else {
            format!(
                "Forgot the saved login for {}. (The credential itself stays valid until it expires; revoke it on the server to end it.)",
                forgotten.join(", ")
            )
        }
    })?;
    Ok(exit::OK)
}
