//! Working with an `ag-server`: clone, fetch, pull, push and remote administration.
//!
//! A local clone is an ordinary embedded AgentGit repository whose default
//! stream *mirrors* the remote stream: `pull` only ever fast-forwards it, and
//! changes travel back as **proposals** (`push`), which the server integrates
//! under its policy. Nothing here can move a remote stream head directly.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ag_changeset::Changeset;
use ag_client::{Client, ClientError, Options};
use ag_integrate::explain::Where;
use ag_integrate::{InitOptions, Repository, SyncOutcome};
use ag_object::RawObject;
use ag_protocol::v1;
use ag_types::{ChangesetId, Namespace, ObjectId, StreamName};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::cmds::repo::rev_or_default;
use crate::ctx::{Ctx, identity};
use crate::{Cli, exit};

// ---- configuration -----------------------------------------------------------

/// One configured remote.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Remote {
    /// `http://host:port` or `https://host:port`.
    pub url: String,
    /// Repository on the server, `owner/name`.
    pub repo: String,
    /// Environment variable holding the credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
    /// File holding the credential (mode 0600), relative to `.ag/` unless absolute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<String>,
    /// Extra CA certificate to trust.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Remotes {
    #[serde(default)]
    remotes: std::collections::BTreeMap<String, Remote>,
}

fn remotes_path(root: &Path) -> PathBuf {
    root.join(".ag").join("remotes.json")
}

fn load_remotes(root: &Path) -> Result<Remotes> {
    match std::fs::read(remotes_path(root)) {
        Ok(b) => serde_json::from_slice(&b).context("remotes.json is damaged"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Remotes::default()),
        Err(e) => Err(e.into()),
    }
}

fn save_remotes(root: &Path, r: &Remotes) -> Result<()> {
    std::fs::write(remotes_path(root), serde_json::to_vec_pretty(r)?)?;
    Ok(())
}

fn write_secret(path: &Path, token: &str) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = ag_crypto::private_file::create(path, false)?;
    f.write_all(token.trim().as_bytes())?;
    Ok(())
}

fn read_token(root: &Path, name: &str, r: &Remote) -> Result<String> {
    credential_source(root, name, r).map(|(token, _)| token)
}

/// Finds a remote's credential and says where it came from (never the credential itself in the label).
pub(crate) fn credential_source(root: &Path, name: &str, r: &Remote) -> Result<(String, String)> {
    if let Some(var) = &r.token_env
        && let Ok(v) = std::env::var(var)
        && !v.trim().is_empty()
    {
        return Ok((v.trim().to_string(), format!("environment variable {var}")));
    }
    if let Some(file) = &r.token_file {
        let path = if Path::new(file).is_absolute() { PathBuf::from(file) } else { root.join(".ag").join(file) };
        match ag_crypto::private_file::check_owner_only(&path) {
            Ok(()) => {}
            Err(ag_crypto::private_file::Insecure::Io(e)) => {
                return Err(anyhow::Error::new(e).context(format!("cannot read {}", path.display())));
            }
            Err(ag_crypto::private_file::Insecure::Accessible) => {
                bail!("{} is readable by other users; run `chmod 600 {}`", path.display(), path.display());
            }
        }
        return Ok((std::fs::read_to_string(&path)?.trim().to_string(), format!("file {}", path.display())));
    }
    let upper: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    for var in [format!("AG_TOKEN_{upper}"), "AG_TOKEN".to_string()] {
        if let Ok(v) = std::env::var(&var)
            && !v.trim().is_empty()
        {
            return Ok((v.trim().to_string(), format!("environment variable {var}")));
        }
    }
    if let Some(login) = crate::credstore::find(&r.url)? {
        let key = crate::credstore::server_key(&r.url)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if login.expires_at_ms > 0 && login.expires_at_ms <= now {
            bail!(
                "the saved login for {key} expired on {}; run `ag login {key}` again",
                ag_types::Timestamp::from_millis(login.expires_at_ms)
            );
        }
        return Ok((login.token, format!("saved login for {key} ({})", login.identity)));
    }
    bail!(
        "no credential for remote '{name}': run `ag login {}`, or set AG_TOKEN (or AG_TOKEN_{upper}), or pass --token-stdin to `ag clone` / `ag remote add`",
        crate::credstore::server_key(&r.url).unwrap_or_else(|_| r.url.clone())
    )
}

/// Splits `https://host:8080/acme/web` into the server URL and `acme/web`.
fn split_url(url: &str) -> Result<(String, Option<String>)> {
    let (scheme, rest) =
        url.split_once("://").ok_or_else(|| anyhow!("'{url}' is not a URL like http://host:8080/owner/name"))?;
    if scheme != "http" && scheme != "https" {
        bail!("unsupported scheme '{scheme}' (use http or https)");
    }
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, p.trim_matches('/')),
        None => (rest, ""),
    };
    if authority.is_empty() {
        bail!("'{url}' has no host");
    }
    let repo = (!path.is_empty()).then(|| path.to_string());
    Ok((format!("{scheme}://{authority}"), repo))
}

pub(crate) fn rt() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?)
}

/// The deadline of a call to a server, in seconds, when `AG_TIMEOUT` does not say otherwise.
const DEFAULT_CALL_TIMEOUT_SECS: u64 = 120;

/// The deadline of every call to a server: `AG_TIMEOUT` seconds (a positive whole number), else 120. A push
/// sends its objects in batches that each get the deadline, so it bounds one batch, not the whole transfer.
pub(crate) fn call_timeout() -> Result<std::time::Duration> {
    match std::env::var("AG_TIMEOUT") {
        Ok(v) if !v.trim().is_empty() => {
            let secs = v.trim().parse::<u64>().ok().filter(|s| *s > 0).ok_or_else(|| {
                anyhow!("AG_TIMEOUT must be a whole number of seconds greater than zero, not '{}'", v.trim())
            })?;
            Ok(std::time::Duration::from_secs(secs))
        }
        _ => Ok(std::time::Duration::from_secs(DEFAULT_CALL_TIMEOUT_SECS)),
    }
}

async fn connect(root: &Path, name: &str, r: &Remote) -> Result<Client> {
    connect_with_timeout(root, name, r, call_timeout()?).await
}

/// How `ag` connects to a remote: the one place that sets the agent run (`AG_RUN`), the user agent and the CA.
pub(crate) fn options_for(r: &Remote, token: String, timeout: std::time::Duration) -> Options {
    Options {
        endpoint: r.url.clone(),
        token,
        run: std::env::var("AG_RUN").ok().filter(|s| !s.is_empty()),
        ca_file: r.ca_file.clone(),
        timeout: Some(timeout),
        user_agent: Some(format!("ag/{}", env!("CARGO_PKG_VERSION"))),
        ..Default::default()
    }
}

/// Connects with `timeout` as the deadline of every call (`ag doctor` asks for a short one).
pub(crate) async fn connect_with_timeout(
    root: &Path,
    name: &str,
    r: &Remote,
    timeout: std::time::Duration,
) -> Result<Client> {
    let token = read_token(root, name, r)?;
    crate::ui::verbose(format!("remote {name}: {} (repository {})", r.url, r.repo));
    Client::connect(options_for(r, token, timeout)).await.map_err(|e| anyhow!("{e}"))
}

/// The configured remotes in name order (none when the repository has no `remotes.json`).
pub(crate) fn load_remotes_if_any(root: &Path) -> Result<Vec<(String, Remote)>> {
    Ok(load_remotes(root)?.remotes.into_iter().collect())
}

/// Asks the server for the current head of `stream` (a short, non-retrying call for `ag status --remote`).
pub(crate) fn server_head(root: &Path, name: &str, remote: &Remote, stream: &str) -> Result<String> {
    rt()?.block_on(async {
        let client = connect_with_timeout(root, name, remote, std::time::Duration::from_secs(10)).await?;
        let head = client
            .streams()
            .read_head(client.req(v1::ReadHeadRequest {
                repo: remote.repo.clone(),
                stream: stream.into(),
                ..Default::default()
            }))
            .await
            .map_err(|s| anyhow!("{}", ClientError::from(s)))?
            .into_inner();
        Ok(if head.head.is_empty() { "(empty)".to_string() } else { head.head })
    })
}

/// `ag agent reap --remote NAME`: asks the server to abandon the stale runs of the remote's repository
/// (`AgentService.ReapRuns`; needs `agent.operate`). `stale_after_ms` 0 = the server's own threshold.
pub(crate) fn reap_runs(ctx: &Ctx, remote: &str, stale_after_ms: u64, dry_run: bool) -> Result<v1::ReapRunsResponse> {
    let (name, r) = resolve_remote(&ctx.root, Some(remote))?;
    rt()?.block_on(async {
        let c = connect(&ctx.root, &name, &r).await?;
        c.agents()
            .reap_runs(c.req(v1::ReapRunsRequest { repo: r.repo.clone(), stale_after_ms, dry_run }))
            .await
            .map(tonic::Response::into_inner)
            .map_err(|s| anyhow!("{}", ClientError::from(s)))
    })
}

/// Lists registered identities from the configured remote.
pub(crate) fn list_agents(ctx: &Ctx, remote: &str) -> Result<Vec<v1::Agent>> {
    let (name, config) = resolve_remote(&ctx.root, Some(remote))?;
    rt()?.block_on(async {
        let client = connect(&ctx.root, &name, &config).await?;
        Ok(client
            .agents()
            .list(client.req(v1::ListAgentsRequest { repo: config.repo }))
            .await
            .map_err(ClientError::from)?
            .into_inner()
            .agents)
    })
}

async fn require_agent_controls(client: &Client) -> Result<()> {
    if !client.negotiate().await?.features.iter().any(|f| f == ag_protocol::features::AGENT_STATE) {
        bail!("this server does not support agent state controls; update it before using this command");
    }
    Ok(())
}

/// Changes a remote agent only if its inspected state still matches.
pub(crate) fn set_agent_state(ctx: &Ctx, remote: &str, agent: &str, disabled: bool) -> Result<()> {
    let (name, config) = resolve_remote(&ctx.root, Some(remote))?;
    rt()?.block_on(async {
        let client = connect(&ctx.root, &name, &config).await?;
        require_agent_controls(&client).await?;
        let agents = client
            .agents()
            .list(client.req(v1::ListAgentsRequest { repo: config.repo.clone() }))
            .await
            .map_err(ClientError::from)?
            .into_inner()
            .agents;
        let current =
            agents.iter().find(|row| row.name == agent).ok_or_else(|| anyhow!("agent '{agent}' not found"))?;
        let request = v1::SetAgentStateRequest {
            repo: config.repo,
            name: agent.into(),
            disabled,
            expected_disabled: current.disabled,
            idempotency_key: ag_crypto::random_id("agent-state")?,
        };
        client.retrying(|| async { client.agents().set_state(client.req(request.clone())).await }).await?;
        Ok(())
    })
}

/// Refines a remote task using the same full-content precondition as the browser.
pub(crate) fn task_attempts(ctx: &Ctx, remote: &str, id: &str, update: Option<(u32, u32)>) -> Result<v1::Task> {
    let (name, config) = resolve_remote(&ctx.root, Some(remote))?;
    rt()?.block_on(async {
        let client = connect(&ctx.root, &name, &config).await?;
        if !client.negotiate().await?.features.iter().any(|f| f == ag_protocol::features::TASK_ATTEMPT_LIMITS) {
            bail!("this server does not support task attempt limits; update it before using this command");
        }
        if let Some((max_attempts, expected_max_attempts)) = update {
            let request = v1::SetTaskAttemptLimitRequest {
                repo: config.repo,
                id: id.into(),
                max_attempts,
                expected_max_attempts,
                idempotency_key: ag_crypto::random_id("task-attempts")?,
            };
            Ok(client
                .retrying(|| async { client.agents().set_task_attempt_limit(client.req(request.clone())).await })
                .await?
                .into_inner())
        } else {
            Ok(client
                .agents()
                .get_task(client.req(v1::GetTaskRequest { repo: config.repo, id: id.into() }))
                .await
                .map_err(ClientError::from)?
                .into_inner())
        }
    })
}

pub(crate) fn update_task(ctx: &Ctx, remote: &str, id: &str, changes: &super::agents::TaskEdit) -> Result<v1::Task> {
    let (name, config) = resolve_remote(&ctx.root, Some(remote))?;
    rt()?.block_on(async {
        let client = connect(&ctx.root, &name, &config).await?;
        if !client.negotiate().await?.features.iter().any(|f| f == ag_protocol::features::TASK_EDIT) {
            bail!("this server does not support task editing; update it before using this command");
        }
        let task = client
            .agents()
            .get_task(client.req(v1::GetTaskRequest { repo: config.repo.clone(), id: id.into() }))
            .await
            .map_err(ClientError::from)?
            .into_inner();
        let current =
            ag_agent::tasks::TaskContent { title: task.title, description: task.description, scope: task.scope };
        let content = changes.apply(&current);
        let request = v1::UpdateTaskRequest {
            repo: config.repo,
            id: id.into(),
            content: Some(v1::TaskContent {
                title: content.title,
                description: content.description,
                scope: content.scope,
            }),
            expected: Some(v1::TaskContent {
                title: current.title,
                description: current.description,
                scope: current.scope,
            }),
            idempotency_key: ag_crypto::random_id("task-edit")?,
        };
        Ok(client
            .retrying(|| async { client.agents().update_task(client.req(request.clone())).await })
            .await?
            .into_inner())
    })
}

fn resolve_remote(root: &Path, name: Option<&str>) -> Result<(String, Remote)> {
    let all = load_remotes(root)?;
    let name = name.unwrap_or("origin").to_string();
    let r = all.remotes.get(&name).cloned().ok_or_else(|| {
        anyhow!("no remote named '{name}' (see `ag remote list`; add one with `ag remote add {name} <url>`)")
    })?;
    Ok((name, r))
}

pub(crate) fn tracking_key(remote: &str, stream: &str) -> String {
    format!("remote.{remote}.{stream}")
}

// ---- fetch ----------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct FetchReport {
    pub remote: String,
    pub stream: String,
    pub head: Option<String>,
    pub version: u64,
    pub changesets: usize,
    pub blobs: usize,
}

async fn download(client: &Client, remote_repo: &str, repo: &Repository, id: &ObjectId) -> Result<RawObject> {
    let raw = client.get_object(remote_repo, id).await.map_err(|e| anyhow!("{e}"))?;
    repo.objects().put(&raw)?;
    Ok(raw)
}

async fn fetch_stream(
    client: &Client,
    remote_name: &str,
    remote: &Remote,
    repo: &Repository,
    stream: &str,
) -> Result<FetchReport> {
    let head = client
        .retrying(|| async {
            client
                .streams()
                .read_head(client.req(v1::ReadHeadRequest {
                    repo: remote.repo.clone(),
                    stream: stream.into(),
                    ..Default::default()
                }))
                .await
        })
        .await
        .map_err(|e| anyhow!("{e}"))?
        .into_inner();
    let mut report = FetchReport {
        remote: remote_name.into(),
        stream: stream.into(),
        head: None,
        version: head.version,
        changesets: 0,
        blobs: 0,
    };
    if head.head.is_empty() {
        return Ok(report);
    }
    let want: ChangesetId = head.head.parse().map_err(|_| anyhow!("the server sent a bad head id"))?;
    let key = tracking_key(remote_name, stream);
    let mut have: Vec<ChangesetId> = Vec::new();
    if let Some(prev) = repo.meta().read(|tx| tx.meta_get(&key))?.and_then(|s| s.parse().ok()) {
        have.push(prev);
    }
    // Everything the server has that we do not, parents first.
    let mut ids: Vec<ChangesetId> = Vec::new();
    let mut token = String::new();
    loop {
        let resp = client
            .retrying(|| async {
                client
                    .history()
                    .ancestors(client.req(v1::AncestorsRequest {
                        repo: remote.repo.clone(),
                        want: vec![want.to_string()],
                        have: have.iter().map(|h| h.to_string()).collect(),
                        page_size: 1000,
                        page_token: token.clone(),
                    }))
                    .await
            })
            .await
            .map_err(|e| anyhow!("{e}"))?
            .into_inner();
        for id in &resp.changeset_ids {
            ids.push(id.parse().map_err(|_| anyhow!("the server sent a bad changeset id"))?);
        }
        if resp.next_page_token.is_empty() {
            break;
        }
        token = resp.next_page_token;
    }
    let mut blobs: BTreeSet<ObjectId> = BTreeSet::new();
    for id in &ids {
        let oid = id.object_id();
        let raw = if repo.objects().contains(&oid)? {
            repo.objects().get(&oid)?.ok_or_else(|| anyhow!("object {oid} vanished"))?
        } else {
            report.changesets += 1;
            download(client, &remote.repo, repo, &oid).await?
        };
        let cs = Changeset::from_raw(&raw, repo.limits()).map_err(|e| anyhow!("changeset {id}: {e}"))?;
        blobs.extend(cs.blobs().map(|b| b.object_id()));
    }
    for b in blobs {
        if !repo.objects().contains(&b)? {
            download(client, &remote.repo, repo, &b).await?;
            report.blobs += 1;
        }
    }
    let actor = identity_for_local(repo);
    let published = repo.publish(&want, &actor, None)?;
    if published.state != ag_integrate::PublishState::Published {
        bail!("the fetched history failed local verification: {}", published.reason.unwrap_or_default());
    }
    repo.meta().write(|tx| tx.meta_set(&key, &want.to_hex()))?;
    report.head = Some(want.to_string());
    Ok(report)
}

fn identity_for_local(_repo: &Repository) -> ag_types::Identity {
    ag_types::Identity::new("system:sync").expect("valid")
}

/// Fast-forwards the local stream to the fetched head and updates the workspace.
fn advance_local(ctx: &Ctx, stream: &str, head: &ChangesetId) -> Result<Option<SyncOutcome>> {
    let state = ctx.repo.meta().read(|tx| tx.stream(stream))?;
    let name = StreamName::new(stream.to_string()).map_err(|e| anyhow!("{e}"))?;
    match state {
        None => {
            ctx.repo.create_stream(&name, Some(head), &ctx.me)?;
        }
        Some(row) => {
            if row.state.head == Some(*head) {
                return Ok(None);
            }
            ctx.repo
                .stream_cas(stream, &row.state, head, &ctx.me, "pull", false)
                .map_err(|e| anyhow!("the local stream '{stream}' cannot fast-forward to the remote head ({e}); it has local history the server lacks"))?;
        }
    }
    let outcome = ctx.repo.sync_workspace("default", &ctx.root, &ctx.me)?;
    if !matches!(outcome, SyncOutcome::UpToDate)
        && let Ok(w) = ctx.repo.workspace_row("default")
        && let Ok(tree) = ctx.repo.tree_or_empty(w.head.as_ref())
    {
        crate::ctx::note_adaptations(&tree);
    }
    Ok(Some(outcome))
}

// ---- clone -----------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct CloneArgs {
    /// `http(s)://host:port/owner/name`
    pub url: String,
    /// Directory to create (default: the repository name).
    pub dir: Option<PathBuf>,
    /// Stream to clone (default: the repository's default stream).
    #[arg(long)]
    pub stream: Option<String>,
    /// Name for the remote.
    #[arg(long, default_value = "origin")]
    pub name: String,
    /// Environment variable holding the credential (default: AG_TOKEN).
    #[arg(long)]
    pub token_env: Option<String>,
    /// Read the credential from stdin and store it in `.ag/credentials/<name>` (mode 0600).
    #[arg(long)]
    pub token_stdin: bool,
    /// Extra CA certificate (PEM) to trust.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
}

pub fn clone(cli: &Cli, a: &CloneArgs) -> Result<u8> {
    let (server, repo_name) = split_url(&a.url)?;
    let repo_name = repo_name.ok_or_else(|| anyhow!("the URL must name the repository: {server}/owner/name"))?;
    let dir = a.dir.clone().unwrap_or_else(|| PathBuf::from(repo_name.rsplit('/').next().unwrap_or("repo")));
    if dir.exists() && std::fs::read_dir(&dir)?.next().is_some() {
        bail!("{} already exists and is not empty", dir.display());
    }
    let mut remote = Remote {
        url: server,
        repo: repo_name.clone(),
        token_env: a.token_env.clone(),
        token_file: None,
        ca_file: a.ca_file.clone(),
    };
    let mut stdin_token = None;
    if a.token_stdin {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        remote.token_file = Some(format!("credentials/{}", a.name));
        stdin_token = Some(s);
    }
    let runtime = rt()?;
    // Validate the connection and the credential before creating anything on disk.
    let (client, info) = runtime.block_on(async {
        let token = match &stdin_token {
            Some(t) => t.trim().to_string(),
            None => read_token(Path::new("."), &a.name, &remote)?,
        };
        let client = Client::connect(options_for(&remote, token, call_timeout()?)).await.map_err(|e| anyhow!("{e}"))?;
        let info = client
            .repos()
            .describe(client.req(v1::GetRepoRequest { repo: repo_name.clone() }))
            .await
            .map_err(|s| anyhow!("{}", ClientError::from(s)))?
            .into_inner();
        Ok::<_, anyhow::Error>((client, info))
    })?;
    let stream = a.stream.clone().unwrap_or_else(|| info.default_stream.clone());
    std::fs::create_dir_all(&dir)?;
    let repo = Repository::init(
        &dir,
        &InitOptions::new(
            Namespace::new(info.namespace.clone()).map_err(|e| anyhow!("{e}"))?,
            StreamName::new(stream.clone()).map_err(|e| anyhow!("{e}"))?,
            identity(cli)?,
        ),
    )?;
    if let Some(t) = &stdin_token {
        write_secret(&dir.join(".ag").join("credentials").join(&a.name), t)?;
    }
    let mut all = Remotes::default();
    all.remotes.insert(a.name.clone(), remote.clone());
    save_remotes(&dir, &all)?;
    let ctx = Ctx { repo, me: identity(cli)?, json: cli.json, quiet: cli.quiet, root: dir.canonicalize()? };
    let report = runtime.block_on(fetch_stream(&client, &a.name, &remote, &ctx.repo, &stream))?;
    if let Some(h) = &report.head {
        advance_local(&ctx, &stream, &h.parse().map_err(|_| anyhow!("bad head"))?)?;
    }
    ctx.done(&report, || {
        format!(
            "Cloned {} into {} ({} changeset(s), {} blob(s)); stream '{stream}' at {}.",
            remote.repo,
            dir.display(),
            report.changesets,
            report.blobs,
            report.head.as_deref().map(|h| &h[..12]).unwrap_or("(empty)")
        )
    })?;
    Ok(exit::OK)
}

// ---- fetch / pull / push -------------------------------------------------------------

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Remote name (default: origin).
    #[arg(long)]
    pub remote: Option<String>,
    /// Stream (default: the repository's default stream).
    #[arg(long)]
    pub stream: Option<String>,
}

pub fn fetch(ctx: &Ctx, a: &SyncArgs) -> Result<u8> {
    let (name, remote) = resolve_remote(&ctx.root, a.remote.as_deref())?;
    let stream = a.stream.clone().unwrap_or_else(|| ctx.default_stream());
    let report = rt()?.block_on(async {
        let client = connect(&ctx.root, &name, &remote).await?;
        fetch_stream(&client, &name, &remote, &ctx.repo, &stream).await
    })?;
    ctx.done(&report, || match &report.head {
        Some(h) => format!(
            "Fetched {} changeset(s) and {} blob(s); {name}/{stream} is at {}.",
            report.changesets,
            report.blobs,
            &h[..12]
        ),
        None => format!("{name}/{stream} has no changesets yet."),
    })?;
    Ok(exit::OK)
}

pub fn pull(ctx: &Ctx, a: &SyncArgs) -> Result<u8> {
    let (name, remote) = resolve_remote(&ctx.root, a.remote.as_deref())?;
    let stream = a.stream.clone().unwrap_or_else(|| ctx.default_stream());
    let report = rt()?.block_on(async {
        let client = connect(&ctx.root, &name, &remote).await?;
        fetch_stream(&client, &name, &remote, &ctx.repo, &stream).await
    })?;
    let Some(head) = &report.head else {
        ctx.done(&report, || format!("{name}/{stream} has no changesets yet."))?;
        return Ok(exit::OK);
    };
    let outcome = advance_local(ctx, &stream, &head.parse().map_err(|_| anyhow!("bad head"))?)?;
    let text = match &outcome {
        None => "Already up to date.".to_string(),
        Some(SyncOutcome::UpToDate) => "Already up to date.".to_string(),
        Some(SyncOutcome::FastForwarded { to }) => format!("Fast-forwarded the workspace to {}.", to.short()),
        Some(SyncOutcome::Merged { id }) => format!("Merged remote changes into the workspace ({}).", id.short()),
        Some(SyncOutcome::Conflicted { conflicts }) => {
            format!("Merged with {} conflict(s); resolve the marked files, then `ag snapshot`.", conflicts.len())
        }
    };
    ctx.done(&serde_json::json!({"fetch": report, "workspace": outcome}), || text.clone())?;
    Ok(if matches!(outcome, Some(SyncOutcome::Conflicted { .. })) { exit::REFUSED } else { exit::OK })
}

#[derive(Args, Debug)]
pub struct PushArgs {
    /// Remote name (default: origin).
    #[arg(long)]
    pub remote: Option<String>,
    /// Revision to propose (default: the workspace head).
    pub rev: Option<String>,
    /// Proposal title.
    #[arg(short, long)]
    pub title: String,
    /// Longer description.
    #[arg(short, long, default_value = "")]
    pub description: String,
    /// Target stream (default: the repository's default stream).
    #[arg(long)]
    pub stream: Option<String>,
    /// Task this proposal belongs to.
    #[arg(long)]
    pub task: Option<String>,
}

pub fn push(ctx: &Ctx, a: &PushArgs) -> Result<u8> {
    let (name, remote) = resolve_remote(&ctx.root, a.remote.as_deref())?;
    let stream = a.stream.clone().unwrap_or_else(|| ctx.default_stream());
    let head = rev_or_default(ctx, &a.rev)?;
    let changes = ctx.repo.workspace_status("default", &ctx.root)?;
    if a.rev.is_none() && !changes.is_empty() {
        bail!("the working directory has {} uncommitted change(s); run `ag snapshot -m ...` first", changes.len());
    }
    let key = tracking_key(&name, &stream);
    let known: Vec<ChangesetId> =
        ctx.repo.meta().read(|tx| tx.meta_get(&key))?.and_then(|s| s.parse().ok()).into_iter().collect();
    let ids = ctx.repo.ancestors_between(&[head], &known)?;
    let mut objects: Vec<RawObject> = Vec::new();
    let mut blob_ids: BTreeSet<ObjectId> = BTreeSet::new();
    for id in &ids {
        let raw = ctx
            .repo
            .objects()
            .get(&id.object_id())?
            .ok_or_else(|| anyhow!("changeset {id} is not in the local store"))?;
        let cs = Changeset::from_raw(&raw, ctx.repo.limits()).map_err(|e| anyhow!("changeset {id}: {e}"))?;
        blob_ids.extend(cs.blobs().map(|b| b.object_id()));
        objects.push(raw);
    }
    for b in &blob_ids {
        objects.push(ctx.repo.objects().get(b)?.ok_or_else(|| anyhow!("blob {b} is not in the local store"))?);
    }
    let outcome = rt()?.block_on(async {
        let client = connect(&ctx.root, &name, &remote).await?;
        let all_ids: Vec<ObjectId> = objects.iter().map(|o| o.id()).collect();
        let missing: BTreeSet<ObjectId> =
            client.missing_objects(&remote.repo, &all_ids).await.map_err(|e| anyhow!("{e}"))?.into_iter().collect();
        let to_send: Vec<RawObject> = objects.iter().filter(|o| missing.contains(&o.id())).cloned().collect();
        let sent = client.upload_objects(&remote.repo, &to_send).await.map_err(|e| anyhow!("{e}"))?;
        let req = v1::CreateProposalRequest {
            repo: remote.repo.clone(),
            stream: stream.clone(),
            head: head.to_string(),
            title: a.title.clone(),
            description: a.description.clone(),
            task_id: a.task.clone().unwrap_or_default(),
            idempotency_key: format!("push:{stream}:{head}"),
        };
        let prop = client
            .retrying(|| async { client.proposals().create(client.req(req.clone())).await })
            .await
            .map_err(|e| anyhow!("{e}"))?
            .into_inner();
        Ok::<_, anyhow::Error>((prop, sent.stored.len(), to_send.len()))
    })?;
    let (prop, stored, sent) = outcome;
    // With --quiet: just the proposal id, like `ag propose`, so `P=$(ag -q push -t "...")` works.
    ctx.created(&prop.id, &serde_json::json!({"proposal": prop, "objects_sent": sent, "objects_stored": stored}), || {
        format!(
            "Uploaded {sent} object(s) and opened proposal {} -> '{stream}'.\nNext: `ag remote prepare {}` to see what is missing, `ag remote integrate {}` when it is ready.",
            prop.id, prop.id, prop.id
        )
    })?;
    Ok(exit::OK)
}

// ---- remote administration ------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct RemoteArgs {
    /// Remote to use (default: origin).
    #[arg(long, global = true)]
    pub remote: Option<String>,
    #[command(subcommand)]
    pub cmd: RemoteCmd,
}

#[derive(Subcommand, Debug)]
pub enum RemoteCmd {
    /// Add a remote.
    Add {
        /// Name, e.g. origin.
        name: String,
        /// `http(s)://host:port/owner/name`
        url: String,
        /// Environment variable holding the credential.
        #[arg(long)]
        token_env: Option<String>,
        /// Read the credential from stdin and store it in `.ag/credentials/<name>` (mode 0600).
        #[arg(long)]
        token_stdin: bool,
        /// Extra CA certificate (PEM) to trust.
        #[arg(long)]
        ca_file: Option<PathBuf>,
    },
    /// List remotes.
    List,
    /// Forget a remote.
    Remove {
        /// Name.
        name: String,
    },
    /// Show who the server thinks you are.
    Whoami,
    /// List proposals on the server.
    Proposals {
        /// Filter: open, integrated, closed.
        #[arg(long)]
        state: Option<String>,
        /// Only proposals that request this reviewer, matched before paging.
        #[arg(long, value_name = "IDENTITY")]
        requested_reviewer: Option<String>,
    },
    /// Show a proposal with its reviews.
    Proposal {
        /// Proposal id.
        id: String,
    },
    /// What would integrating do now, and what is missing?
    Prepare {
        /// Proposal id.
        id: String,
        /// Also queue the validation jobs the policy knows how to run.
        #[arg(long)]
        validate: bool,
    },
    /// Explain why a server proposal cannot land, with the next commands. Changes nothing.
    Explain {
        /// Proposal id (any unique prefix).
        id: String,
    },
    /// Inspect, add or remove requested reviewers (no approval or access change).
    Reviewers(crate::cmds::reviewers::ReviewersArgs),
    /// Save or restore your private account review notes without submitting them.
    ReviewDraft {
        #[command(subcommand)]
        cmd: crate::cmds::review_drafts::ReviewDraftCmd,
    },
    /// Review a proposal's current head.
    Review {
        /// Proposal id.
        id: String,
        /// Approve.
        #[arg(long, group = "decision")]
        approve: bool,
        /// Request changes.
        #[arg(long, group = "decision")]
        request_changes: bool,
        /// Comment only.
        #[arg(long, group = "decision")]
        comment: bool,
        /// Review text.
        #[arg(short, long, default_value = "")]
        message: String,
    },
    /// Read and participate in revision-bound line discussions.
    Discussion {
        #[command(subcommand)]
        cmd: crate::cmds::discussions::DiscussionCmd,
    },
    /// Integrate a proposal into its stream.
    Integrate {
        /// Proposal id.
        id: String,
        /// Fencing tokens of exclusive leases the author holds.
        #[arg(long = "fencing-token")]
        fencing_tokens: Vec<u64>,
    },
    /// Close a proposal without integrating it.
    Close {
        /// Proposal id.
        id: String,
    },
    /// List streams.
    Streams,
    /// Show the events of the repository (replay), or follow them live.
    Events {
        /// Replay events after this sequence number.
        #[arg(long, default_value_t = 0)]
        after: u64,
        /// Only these topics (prefix match), e.g. `stream,proposal`.
        #[arg(long, value_delimiter = ',')]
        topics: Vec<String>,
        /// Keep following new events.
        #[arg(long)]
        follow: bool,
    },
    /// Show or set a stream's policy.
    Policy {
        /// Stream.
        stream: String,
        /// Replace the policy with the JSON document in this file.
        #[arg(long, conflicts_with = "preset")]
        set: Option<PathBuf>,
        /// Replace the policy with a ready-made one: solo, team, agents or strict (`ag policy preset list`).
        #[arg(long, value_name = "NAME")]
        preset: Option<String>,
        /// Also say in plain language what the policy allows, requires and refuses.
        #[arg(long)]
        explain: bool,
        #[command(flatten)]
        flags: crate::cmds::policy::PresetFlags,
    },
    /// Show history of a revision on the server.
    Log {
        /// Revision (default: the default stream).
        rev: Option<String>,
        /// Maximum entries.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
    },
    /// Show which changeset introduced each line of a file on the server: `ag remote blame main:src/lib.rs`.
    Blame {
        /// `[REV:]PATH`
        spec: String,
        /// Only these lines, `START,END`.
        #[arg(short = 'L', long, value_name = "START,END")]
        lines: Option<String>,
        /// Look at most this many changesets back (default: the server's limit; the output says when it stopped).
        #[arg(long, value_name = "N", default_value_t = 0)]
        max_commits: u32,
    },
    /// Print a file as of a revision on the server: `ag remote cat main:src/lib.rs`.
    Cat {
        /// `REV:PATH`
        spec: String,
    },
}

fn local_root(cli: &Cli) -> Result<PathBuf> {
    let start = match &cli.repo {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    Ok(ag_integrate::Layout::discover(&start).map(|l| l.root().to_path_buf()).unwrap_or(start))
}

fn print<T: Serialize>(cli: &Cli, value: &T, human: impl FnOnce() -> String) -> Result<()> {
    crate::ctx::emit(cli.json, value, human)
}

/// Like [`print()`] for confirmations, which `--quiet` suppresses.
fn print_done<T: Serialize>(cli: &Cli, value: &T, human: impl FnOnce() -> String) -> Result<()> {
    if cli.quiet && !cli.json {
        return Ok(());
    }
    print(cli, value, human)
}

async fn record_remote_review(client: &Client, repo: &str, id: &str, decision: &str, body: &str) -> Result<v1::Review> {
    if decision == "comment"
        && client.negotiate().await?.features.iter().any(|feature| feature == ag_protocol::features::PROPOSAL_COMMENTS)
    {
        let detail = client
            .proposals()
            .get(client.req(v1::GetProposalRequest { repo: repo.into(), id: id.into() }))
            .await
            .map_err(ClientError::from)?
            .into_inner();
        let proposal = detail.proposal.ok_or_else(|| anyhow!("server returned no proposal"))?;
        let request = v1::CommentProposalRequest {
            repo: repo.into(),
            id: proposal.id,
            body: body.into(),
            expected_head: proposal.head,
            idempotency_key: ag_crypto::random_id("comment")?,
        };
        return Ok(client
            .retrying(|| async { client.proposals().comment(client.req(request.clone())).await })
            .await?
            .into_inner());
    }
    Ok(client
        .proposals()
        .review(client.req(v1::ReviewProposalRequest {
            repo: repo.into(),
            id: id.into(),
            decision: decision.into(),
            body: body.into(),
            idempotency_key: String::new(),
            expected_head: String::new(),
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner())
}

async fn explain_remote(cli: &Cli, client: &Client, repo: &str, id: &str) -> Result<u8> {
    use crate::cmds::explain::{from_wire, render};
    let detail = client
        .proposals()
        .get(client.req(v1::GetProposalRequest { repo: repo.into(), id: id.into() }))
        .await
        .map_err(|s| anyhow!("{}", ClientError::from(s)))?
        .into_inner();
    let proposal = detail.proposal.ok_or_else(|| anyhow!("server returned no proposal"))?;
    if proposal.state != "open" {
        let state = if proposal.state == "integrated" { "already landed" } else { "closed" };
        let summary = format!("{} is {state}; there is nothing to fix.", proposal.id);
        print(
            cli,
            &serde_json::json!({ "proposal": proposal.id, "state": proposal.state,
            "standing": "refused", "explanations": [], "next": [], "summary": summary }),
            || summary,
        )?;
        return Ok(exit::REFUSED);
    }
    let prep = client
        .integration()
        .prepare(client.req(v1::PrepareRequest {
            repo: repo.into(),
            proposal: proposal.id.clone(),
            enqueue_validations: false,
            idempotency_key: String::new(),
        }))
        .await
        .map_err(|s| anyhow!("{}", ClientError::from(s)))?
        .into_inner();
    let mut explanations = from_wire(&prep.blockers, &prep.conflicts, &proposal.id);
    if !prep.problem.is_empty() {
        explanations.push(ag_integrate::explain::Explanation {
            code: "invalid".into(),
            kind: "denied",
            summary: "The candidate cannot be prepared.".into(),
            why: prep.problem.clone(),
            fixes: vec![],
        });
    }
    let (standing, code) = if explanations.is_empty() {
        ("ready", exit::OK)
    } else if explanations.iter().all(|e| e.kind == "blocked") {
        ("blocked", exit::BLOCKED)
    } else {
        ("refused", exit::REFUSED)
    };
    let next: Vec<&str> = explanations.iter().filter_map(|e| e.fixes.first()).map(|f| f.command.as_str()).collect();
    let body = serde_json::json!({ "proposal": proposal.id, "title": proposal.title, "stream": proposal.stream,
        "standing": standing, "policy_version": prep.policy_version, "candidate_tree": prep.candidate_tree,
        "explanations": explanations, "next": next });
    print(cli, &body, || {
        let heading = format!(
            "Proposal {} \"{}\" -> {}: {standing} (policy v{}).",
            proposal.id, proposal.title, proposal.stream, prep.policy_version
        );
        if code == exit::OK {
            format!("{heading}\nNext:\n  ag remote integrate {}", proposal.id)
        } else {
            format!("{heading}\n\n{}", render(&explanations))
        }
    })?;
    Ok(code)
}

pub fn remote(cli: &Cli, args: &RemoteArgs) -> Result<u8> {
    let root = local_root(cli)?;
    match &args.cmd {
        RemoteCmd::Add { name, url, token_env, token_stdin, ca_file } => {
            if !root.join(".ag").is_dir() {
                bail!("not an AgentGit repository; run `ag init` first");
            }
            let (server, repo) = split_url(url)?;
            let repo = repo.ok_or_else(|| anyhow!("the URL must name the repository: {server}/owner/name"))?;
            let mut r =
                Remote { url: server, repo, token_env: token_env.clone(), token_file: None, ca_file: ca_file.clone() };
            if *token_stdin {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                r.token_file = Some(format!("credentials/{name}"));
                write_secret(&root.join(".ag").join("credentials").join(name), &s)?;
            }
            let mut all = load_remotes(&root)?;
            all.remotes.insert(name.clone(), r.clone());
            save_remotes(&root, &all)?;
            print_done(cli, &r, || format!("Added remote '{name}' -> {}/{}", r.url, r.repo))?;
        }
        RemoteCmd::List => {
            let all = load_remotes(&root)?;
            print(cli, &all.remotes, || {
                all.remotes.iter().map(|(n, r)| format!("{n}\t{}/{}", r.url, r.repo)).collect::<Vec<_>>().join("\n")
            })?;
        }
        RemoteCmd::Remove { name } => {
            let mut all = load_remotes(&root)?;
            if !all.remotes.contains_key(name) {
                bail!("no remote named '{name}'");
            }
            // The stored credential goes first: if it cannot be deleted the secret would stay on disk with
            // nothing left that names it, so the remote is kept and the command can be run again.
            let credential = root.join(".ag").join("credentials").join(name);
            match std::fs::remove_file(&credential) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => bail!(
                    "cannot delete the stored credential {} of remote '{name}': {e}; the remote is kept, delete the file by hand and run this again",
                    credential.display()
                ),
            }
            all.remotes.remove(name);
            save_remotes(&root, &all)?;
            print_done(cli, &serde_json::json!({"removed": name}), || format!("Removed remote '{name}'."))?;
        }
        other => return remote_online(cli, &root, args.remote.as_deref(), other),
    }
    Ok(exit::OK)
}

fn remote_online(cli: &Cli, root: &Path, remote_name: Option<&str>, cmd: &RemoteCmd) -> Result<u8> {
    let (name, r) = resolve_remote(root, remote_name)?;
    let repo = r.repo.clone();
    let runtime = rt()?;
    runtime.block_on(async {
        let c = connect(root, &name, &r).await?;
        let e = |e: ClientError| anyhow!("{e}");
        let s = |s: tonic::Status| anyhow!("{}", ClientError::from(s));
        match cmd {
            RemoteCmd::Whoami => {
                let p = c.protocol().whoami(c.req(v1::WhoamiRequest {})).await.map_err(s)?.into_inner();
                print(cli, &p, || {
                    format!("{} ({}, via {})\nroles: {}", p.identity, p.kind, p.method, p.roles.join(", "))
                })?;
            }
            RemoteCmd::Proposals { state, requested_reviewer } => {
                if requested_reviewer.is_some() {
                    crate::cmds::reviewers::require_support(&c).await?;
                }
                let requested_reviewer = requested_reviewer.as_deref()
                    .map(crate::cmds::reviewers::reviewer_identity)
                    .transpose()?
                    .map(|identity| identity.to_string());
                // Every page: a repository with more proposals than one page must not be silently cut short.
                let mut all = Vec::new();
                let mut token = String::new();
                loop {
                    let page = c
                        .proposals()
                        .list(c.req(v1::ListProposalsRequest {
                            repo: repo.clone(),
                            state: state.clone().unwrap_or_default(),
                            requested_reviewer: requested_reviewer.clone().unwrap_or_default(),
                            page_size: ag_protocol::limits::MAX_PAGE_SIZE,
                            page_token: token,
                            ..Default::default()
                        }))
                        .await
                        .map_err(s)?
                        .into_inner();
                    all.extend(page.proposals);
                    token = page.next_page_token;
                    if token.is_empty() {
                        break;
                    }
                }
                let resp = v1::ListProposalsResponse { proposals: all, next_page_token: String::new() };
                print(cli, &resp, || {
                    resp.proposals
                        .iter()
                        .map(|p| {
                            format!(
                                "{}  {:<10} {} -> {}  {}  ({})",
                                p.id,
                                p.state,
                                &p.head[..12],
                                p.stream,
                                p.title,
                                p.author
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })?;
            }
            RemoteCmd::Proposal { id } => {
                let d = c
                    .proposals()
                    .get(c.req(v1::GetProposalRequest { repo, id: id.clone() }))
                    .await
                    .map_err(s)?
                    .into_inner();
                print(cli, &d, || {
                    let p = d.proposal.clone().unwrap_or_default();
                    let mut t = format!(
                        "{}  [{}]  {}\n  {} -> {}  head {}\n  author {}\n",
                        p.id,
                        p.state,
                        p.title,
                        p.author,
                        p.stream,
                        &p.head[..12],
                        p.author
                    );
                    t.push_str(&format!("{}\n", crate::cmds::reviewers::describe(&d.requested_reviewers)));
                    for r in &d.reviews {
                        t.push_str(&format!(
                            "  review: {} by {} on {}  {}\n",
                            r.decision,
                            r.reviewer,
                            &r.head[..12],
                            r.body
                        ));
                    }
                    t
                })?;
            }
            RemoteCmd::Explain { id } => return explain_remote(cli, &c, &repo, id).await,
            RemoteCmd::Prepare { id, validate } => {
                let p = c
                    .integration()
                    .prepare(c.req(v1::PrepareRequest {
                        repo,
                        proposal: id.clone(),
                        enqueue_validations: *validate,
                        idempotency_key: String::new(),
                    }))
                    .await
                    .map_err(s)?
                    .into_inner();
                print(cli, &p, || {
                    let mut t = format!("strategy: {}\ncandidate tree: {}\n", p.strategy, p.candidate_tree);
                    for cf in &p.conflicts {
                        t.push_str(&format!(
                            "conflict: {} ({})\n",
                            cf.path,
                            if cf.message.is_empty() { &cf.kind } else { &cf.message }
                        ));
                    }
                    if !p.problem.is_empty() {
                        t.push_str(&format!("problem: {}\n", p.problem));
                    }
                    for v in &p.validations {
                        t.push_str(&format!("check {:<16} {}\n", v.check, v.state));
                    }
                    for b in &p.blockers {
                        t.push_str(&format!("blocked: {}\n", b.message));
                    }
                    if p.blockers.is_empty() && p.conflicts.is_empty() && p.problem.is_empty() {
                        t.push_str("nothing blocks this integration.\n");
                    } else {
                        let ex = crate::cmds::explain::from_wire(&p.blockers, &p.conflicts, id);
                        t.push_str(&format!("What to do:\n{}\n", crate::cmds::explain::steps(&ex, id, Where::Remote)));
                    }
                    t
                })?;
                if !p.conflicts.is_empty() || !p.problem.is_empty() {
                    return Ok(exit::REFUSED);
                }
                if !p.blockers.is_empty() {
                    return Ok(exit::BLOCKED);
                }
            }
            RemoteCmd::Reviewers(args) => {
                let result = crate::cmds::reviewers::remote(&c, &repo, args).await?;
                if args.changes_state() {
                    print_done(cli, &result, || crate::cmds::reviewers::describe(&result.reviewers))?;
                } else {
                    print(cli, &result, || crate::cmds::reviewers::describe(&result.reviewers))?;
                }
            }
            RemoteCmd::ReviewDraft { cmd } => {
                let result = crate::cmds::review_drafts::run(&c, &repo, cmd).await?;
                if cmd.changes_state() {
                    print_done(cli, &result, || crate::cmds::review_drafts::describe(&result))?;
                } else {
                    print(cli, &result, || crate::cmds::review_drafts::describe(&result))?;
                }
            }
            RemoteCmd::Review { id, approve, request_changes, comment, message } => {
                let decision = match (approve, request_changes, comment) {
                    (true, _, _) => "approve",
                    (_, true, _) => "request_changes",
                    (_, _, true) => "comment",
                    _ => bail!("choose one of --approve, --request-changes, --comment"),
                };
                let rv = record_remote_review(&c, &repo, id, decision, message).await?;
                print_done(cli, &rv, || format!("Recorded '{}' on head {}.", rv.decision, &rv.head[..12]))?;
            }
            RemoteCmd::Discussion { cmd } => {
                let (value, message) = crate::cmds::discussions::run(&c, &repo, cmd).await?;
                if cmd.changes_state() { print_done(cli, &value, || message)?; }
                else { print(cli, &value, || message)?; }
            }
            RemoteCmd::Integrate { id, fencing_tokens } => {
                let res = c
                    .integration()
                    .integrate(c.req(v1::IntegrateRequest {
                        repo,
                        proposal: id.clone(),
                        idempotency_key: String::new(),
                        fencing_tokens: fencing_tokens.clone(),
                    }))
                    .await
                    .map_err(s)?
                    .into_inner();
                let o = res.outcome.clone().unwrap_or_default();
                let human = || match o.status.as_str() {
                    "integrated" => format!(
                        "Integrated ({}): stream is now at {} (version {}), {} file(s) changed.",
                        o.strategy,
                        &o.new_head[..12],
                        o.version,
                        o.changed_files
                    ),
                    "blocked" => format!(
                        "Blocked (policy v{}); the stream was not changed:\n{}\nProvide what is missing, then run `ag remote integrate {id}` again.\n{}",
                        o.policy_version,
                        o.blockers.iter().map(|b| format!("  - {}", b.message)).collect::<Vec<_>>().join("\n"),
                        crate::cmds::explain::hints(&crate::cmds::explain::from_wire(&o.blockers, &[], id), id, Where::Remote)
                    ),
                    "conflict" => format!(
                        "Conflict:\n{}",
                        o.conflicts
                            .iter()
                            .map(|c| format!(
                                "  - {} ({})",
                                c.path,
                                if c.message.is_empty() { &c.kind } else { &c.message }
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    "denied" => format!(
                        "Denied by policy; the stream was not changed:\n{}\nWhat to do:\n{}",
                        o.blockers.iter().map(|b| format!("  - {}", b.message)).collect::<Vec<_>>().join("\n"),
                        crate::cmds::explain::hints(&crate::cmds::explain::from_wire(&o.blockers, &[], id), id, Where::Remote)
                    ),
                    _ => format!("Not integrated: {}", o.reason),
                };
                if o.status == "integrated" {
                    print_done(cli, &res, human)?;
                } else {
                    print(cli, &res, human)?;
                }
                return Ok(match o.status.as_str() {
                    "integrated" => exit::OK,
                    "blocked" => exit::BLOCKED,
                    _ => exit::REFUSED,
                });
            }
            RemoteCmd::Close { id } => {
                let p = c
                    .proposals()
                    .close(c.req(v1::CloseProposalRequest { repo, id: id.clone(), idempotency_key: String::new() }))
                    .await
                    .map_err(s)?
                    .into_inner();
                print_done(cli, &p, || format!("Closed {}.", p.id))?;
            }
            RemoteCmd::Streams => {
                let r = c.streams().list(c.req(v1::ListStreamsRequest { repo })).await.map_err(s)?.into_inner();
                print(cli, &r, || {
                    r.streams
                        .iter()
                        .map(|x| {
                            format!(
                                "{:<20} {}  v{}  policy v{}",
                                x.name,
                                if x.head.is_empty() { "(empty)".into() } else { x.head[..12].to_string() },
                                x.version,
                                x.policy_version
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })?;
            }
            RemoteCmd::Events { after, topics, follow } => {
                if *follow {
                    let mut stream = c
                        .events()
                        .subscribe(tonic::Request::new(v1::SubscribeRequest {
                            repo,
                            topics: topics.clone(),
                            after_seq: *after,
                            heartbeat_ms: 15_000,
                        }))
                        .await
                        .map_err(s)?
                        .into_inner();
                    while let Some(ev) = stream.message().await.map_err(s)? {
                        if ev.topic == "heartbeat" {
                            continue;
                        }
                        if cli.json {
                            // One compact object per line (NDJSON), so that a consumer can read line by line.
                            crate::ui::print_line(&serde_json::to_string(&ev)?);
                        } else {
                            print(cli, &ev, || format!("{:>6}  {}  {}", ev.seq, ev.topic, ev.payload_json))?;
                        }
                    }
                } else {
                    // Every event after `after`, a page at a time: a full page may be followed by more.
                    let page_size = ag_protocol::limits::MAX_PAGE_SIZE;
                    let mut cursor = *after;
                    let mut events = Vec::new();
                    let latest_seq = loop {
                        let page = c
                            .events()
                            .replay(c.req(v1::ReplayRequest {
                                repo: repo.clone(),
                                topics: topics.clone(),
                                after_seq: cursor,
                                limit: page_size,
                            }))
                            .await
                            .map_err(s)?
                            .into_inner();
                        let latest = page.latest_seq;
                        let full = page.events.len() >= page_size as usize;
                        if let Some(last) = page.events.last() {
                            cursor = last.seq;
                        }
                        events.extend(page.events);
                        if !full {
                            break latest;
                        }
                    };
                    let r = v1::ReplayResponse { events, latest_seq };
                    print(cli, &r, || {
                        r.events
                            .iter()
                            .map(|ev| format!("{:>6}  {}  {}", ev.seq, ev.topic, ev.payload_json))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })?;
                }
            }
            RemoteCmd::Policy { stream, set, preset, explain, flags } => {
                let document = match (set, preset) {
                    (Some(file), _) => {
                        Some(std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?)
                    }
                    (None, Some(name)) => {
                        // The signing key of `--ci-key` lives in the clone, if there is one.
                        let local = Repository::open(root).ok();
                        let me = identity(cli)?;
                        Some(crate::cmds::policy::build_preset(name, flags, &me, local.as_ref(), true)?.to_json())
                    }
                    (None, None) => None,
                };
                let v = match document {
                    Some(doc) => c
                        .admin()
                        .set_policy(c.req(v1::SetPolicyRequest {
                            repo,
                            stream: stream.clone(),
                            document: doc,
                            idempotency_key: String::new(),
                        }))
                        .await
                        .map_err(s)?
                        .into_inner(),
                    None => c
                        .admin()
                        .get_policy(c.req(v1::GetPolicyRequest { repo, stream: stream.clone() }))
                        .await
                        .map_err(s)?
                        .into_inner(),
                };
                if *explain {
                    let policy = ag_policy::Policy::parse(&v.document).context("the server's policy does not parse")?;
                    let statements = ag_policy::explain::explain(&policy);
                    print(cli, &serde_json::json!({"version": v.version, "stream": v.stream, "explanation": statements}), || {
                        format!(
                            "# policy v{} of '{}'{}\n{}",
                            v.version,
                            v.stream,
                            if v.version == 0 { " (the built-in default; nothing stored yet)" } else { "" },
                            ag_policy::explain::render(&statements)
                        )
                    })?;
                } else {
                    print(cli, &v, || format!("policy v{} of '{}':\n{}", v.version, v.stream, v.document))?;
                }
            }
            RemoteCmd::Log { rev, limit } => {
                // The server clamps a page; ask for as many pages as `-n` needs instead of getting fewer entries
                // than were asked for without a word.
                let mut changesets = Vec::new();
                let mut token = String::new();
                loop {
                    let want = (*limit as usize).saturating_sub(changesets.len());
                    if want == 0 {
                        break;
                    }
                    let page = c
                        .history()
                        .log(c.req(v1::LogRequest {
                            repo: repo.clone(),
                            rev: rev.clone().unwrap_or_default(),
                            page_size: want.min(ag_protocol::limits::MAX_PAGE_SIZE as usize) as u32,
                            page_token: token,
                            ..Default::default()
                        }))
                        .await
                        .map_err(s)?
                        .into_inner();
                    changesets.extend(page.changesets);
                    token = page.next_page_token;
                    if token.is_empty() {
                        break;
                    }
                }
                let r = v1::LogResponse { changesets, next_page_token: token };
                print(cli, &r, || {
                    r.changesets
                        .iter()
                        .map(|x| format!("{}  {}  {}", &x.id[..12], x.author, x.message.lines().next().unwrap_or("")))
                        .collect::<Vec<_>>()
                        .join("\n")
                })?;
            }
            RemoteCmd::Blame { spec, lines, max_commits } => {
                let (rev, path) = spec
                    .split_once(':')
                    .map(|(r, p)| (r.to_string(), p.to_string()))
                    .unwrap_or((String::new(), spec.clone()));
                let (first_line, last_line) = match lines {
                    None => (0, 0),
                    Some(l) => {
                        let (a, b) = l.split_once(',').ok_or_else(|| anyhow!("--lines takes START,END, e.g. 10,20"))?;
                        (
                            a.trim().parse().map_err(|_| anyhow!("bad line number '{a}'"))?,
                            b.trim().parse().map_err(|_| anyhow!("bad line number '{b}'"))?,
                        )
                    }
                };
                let b = c
                    .history()
                    .blame(c.req(v1::BlameRequest {
                        repo,
                        rev,
                        path,
                        first_line,
                        last_line,
                        max_commits: *max_commits,
                    }))
                    .await
                    .map_err(s)?
                    .into_inner();
                print(cli, &b, || {
                    let width = b.lines.last().map(|l| l.line.to_string().len()).unwrap_or(1);
                    let author_w = b.lines.iter().map(|l| l.author.len()).max().unwrap_or(0).min(28);
                    let mut t = String::new();
                    for l in &b.lines {
                        let mark = if l.boundary { "^" } else { " " };
                        t.push_str(&format!(
                            "{mark}{} {:<author_w$} {:>width$}) {}\n",
                            &l.changeset[..8],
                            l.author,
                            l.line,
                            l.text
                        ));
                    }
                    if b.truncated {
                        t.push_str(&format!("(stopped after {} changesets; lines marked ^ may be older)\n", b.visited));
                    }
                    t
                })?;
            }
            RemoteCmd::Cat { spec } => {
                let (rev, path) =
                    spec.split_once(':').ok_or_else(|| anyhow!("expected REV:PATH, e.g. main:src/lib.rs"))?;
                let bytes = c.get_file(&repo, rev, path).await.map_err(e)?;
                crate::ui::print_bytes(&bytes);
            }
            RemoteCmd::Add { .. } | RemoteCmd::List | RemoteCmd::Remove { .. } => unreachable!("handled offline"),
        }
        Ok::<u8, anyhow::Error>(exit::OK)
    })
}
