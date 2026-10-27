//! Inspect saved connections without exposing credentials or needing a local repository.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ag_client::Client;
use ag_protocol::v1;
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::json;

use crate::cmds::remote::{Remote, options_for, rt};
use crate::credstore::{self, Login};
use crate::{Cli, exit};

#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// List saved connections; --check asks each server whether its saved credential still works.
    Status(StatusArgs),
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Inspect just this server (a repository URL works too).
    server: Option<String>,
    /// Verify saved credentials online. Without this, status is local and works offline.
    #[arg(long)]
    check: bool,
    /// Timeout for each online check, in seconds.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=60), requires = "check")]
    timeout: u64,
    /// Extra CA certificate (PEM) for online checks.
    #[arg(long, requires = "check")]
    ca_file: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Saved,
    Verified,
    Expired,
    Rejected,
    Unreachable,
}

#[derive(Serialize)]
struct Connection {
    server: String,
    identity: String,
    state: State,
    expires_at: Option<String>,
}

fn expiration(expires_at_ms: i64) -> Option<String> {
    (expires_at_ms > 0).then(|| ag_types::Timestamp::from_millis(expires_at_ms).to_string())
}

fn saved_connection(server: String, login: &Login, now: i64) -> Connection {
    let expired = login.expires_at_ms > 0 && login.expires_at_ms <= now;
    Connection {
        server,
        identity: login.identity.clone(),
        state: if expired { State::Expired } else { State::Saved },
        expires_at: expiration(login.expires_at_ms),
    }
}

async fn check_connection(connection: &mut Connection, login: &Login, args: &StatusArgs) {
    let remote = Remote {
        url: connection.server.clone(),
        repo: String::new(),
        token_env: None,
        token_file: None,
        ca_file: args.ca_file.clone(),
    };
    let timeout = Duration::from_secs(args.timeout);
    let result = tokio::time::timeout(timeout, async {
        let client = Client::connect(options_for(&remote, login.token.clone(), timeout)).await.ok()?;
        Some(client.protocol().whoami(client.req(v1::WhoamiRequest {})).await)
    })
    .await;
    match result {
        Ok(Some(Ok(reply))) => {
            let who = reply.into_inner();
            connection.identity = who.identity;
            connection.expires_at = expiration(who.expires_at_ms);
            connection.state = State::Verified;
        }
        Ok(Some(Err(status)))
            if matches!(status.code(), tonic::Code::Unauthenticated | tonic::Code::PermissionDenied) =>
        {
            connection.state = State::Rejected;
        }
        _ => connection.state = State::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_expiration_includes_the_boundary_and_keeps_unknown_expiry_unknown() {
        let mut login = Login { token: String::new(), identity: "user:test".into(), expires_at_ms: 10, saved_at_ms: 0 };
        assert_eq!(saved_connection("server".into(), &login, 10).state, State::Expired);
        assert_eq!(saved_connection("server".into(), &login, 9).state, State::Saved);
        login.expires_at_ms = 0;
        let unknown = saved_connection("server".into(), &login, 10);
        assert_eq!(unknown.state, State::Saved);
        assert_eq!(unknown.expires_at, None);
        assert!(expiration(1).is_some());
    }
}

fn describe(connection: &Connection) -> String {
    let state = match connection.state {
        State::Saved => "saved locally; not checked online",
        State::Verified => "verified with the server",
        State::Expired => "expired; sign in again with ag login",
        State::Rejected => "rejected by the server; sign in again with ag login",
        State::Unreachable => "could not verify; check the server address, network and CA certificate",
    };
    let expiry = connection.expires_at.as_deref().unwrap_or("not specified");
    format!("{}\n  {} — {state}\n  Expires: {expiry}", connection.server, connection.identity)
}

fn selected_logins(server: Option<&str>) -> Result<Vec<(String, Login)>> {
    let mut logins = credstore::list()?;
    if let Some(server) = server {
        let key = credstore::server_key(server)?;
        logins.retain(|(server, _)| server == &key);
        if logins.is_empty() {
            bail!("no saved login for {key}; sign in with `ag login {key}`");
        }
    }
    Ok(logins)
}

fn connections(args: &StatusArgs) -> Result<Vec<Connection>> {
    let logins = selected_logins(args.server.as_deref())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    let runtime = args.check.then(rt).transpose()?;
    let mut connections = Vec::new();
    for (server, login) in logins {
        let mut connection = saved_connection(server, &login, now);
        if let Some(runtime) = &runtime {
            runtime.block_on(check_connection(&mut connection, &login, args));
        }
        connections.push(connection);
    }
    Ok(connections)
}

fn render(connections: &[Connection]) -> String {
    if connections.is_empty() {
        return "No saved connections. Sign in with `ag login SERVER`.".into();
    }
    format!(
        "{}\n\nSaved logins only. Use `ag auth status --check` to verify them online. Environment and remote-specific credentials are not listed.",
        connections.iter().map(describe).collect::<Vec<_>>().join("\n\n")
    )
}

pub fn auth(cli: &Cli, command: &AuthCmd) -> Result<u8> {
    let AuthCmd::Status(args) = command;
    let connections = connections(args)?;
    let failed = connections.iter().any(|c| matches!(c.state, State::Expired | State::Rejected | State::Unreachable));
    crate::ctx::emit(cli.json, &json!({ "connections": connections, "checked_online": args.check }), || {
        render(&connections)
    })?;
    Ok(if failed { exit::ERROR } else { exit::OK })
}
