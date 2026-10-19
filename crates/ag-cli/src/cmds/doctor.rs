//! `ag doctor`: is this installation, repository, identity and remote set up so that things will work?
//!
//! Every check answers "ok", "warn" (works, but look at this), "fail" (something will not work) or "skip"
//! (not applicable here), and a failing check says what to do. The repository check is a *quick* one that
//! reads metadata and a few objects; `ag verify` is the full integrity check that hashes everything.
//! Exit code: 0 unless a check failed (with `--strict`, unless a check failed or warned).

use std::path::Path;
use std::time::Duration;

use ag_integrate::{Layout, Repository};
use ag_protocol::v1;
use ag_storage::meta::IntentState;
use anyhow::Result;
use clap::Args;
use serde::Serialize;

use crate::cmds::remote::{self, Remote};
use crate::ctx::{self, identity_with_source};
use crate::ui::{self, Tone};
use crate::{Cli, JSON_SCHEMA_VERSION, exit, userconf};

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Do not contact any server: check that credentials exist, not that they work.
    #[arg(long)]
    pub offline: bool,
    /// Exit with 1 when a check warns, not only when one fails.
    #[arg(long)]
    pub strict: bool,
    /// How long to wait for each server before giving up.
    #[arg(long, default_value_t = 10, value_name = "SECONDS")]
    pub timeout: u64,
}

/// The result of one check.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Fine.
    Ok,
    /// Works, but look at this.
    Warn,
    /// Will not work.
    Fail,
    /// Not applicable here.
    Skip,
}

/// One line of the report.
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    /// Stable identifier, e.g. `repository` or `remote.origin`.
    pub id: String,
    /// The verdict.
    pub status: Status,
    /// What was found.
    pub summary: String,
    /// What to do about it (for warn and fail).
    pub hint: Option<String>,
}

fn check(id: impl Into<String>, status: Status, summary: impl Into<String>, hint: Option<&str>) -> Check {
    Check { id: id.into(), status, summary: summary.into(), hint: hint.map(str::to_string) }
}

/// Counts per status.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Counts {
    /// Fine.
    pub ok: usize,
    /// Warnings.
    pub warn: usize,
    /// Failures.
    pub fail: usize,
    /// Skipped.
    pub skip: usize,
}

/// The whole report.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    /// Is nothing failing (and, with `--strict`, nothing warning)?
    pub ok: bool,
    /// The `ag` version.
    pub version: &'static str,
    /// The checks, in the order they ran.
    pub checks: Vec<Check>,
    /// Tally.
    pub summary: Counts,
}

fn human_duration(ms: i64) -> String {
    let s = ms / 1000;
    match s {
        s if s >= 86_400 => format!("{}d {}h", s / 86_400, (s % 86_400) / 3600),
        s if s >= 3600 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// Entry point.
pub fn doctor(cli: &Cli, a: &DoctorArgs) -> Result<u8> {
    let mut checks = vec![check(
        "version",
        Status::Ok,
        format!(
            "ag {} ({} {}), protocol {}, json schema {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            ag_protocol::VERSION,
            JSON_SCHEMA_VERSION
        ),
        None,
    )];
    checks.push(user_config_check());
    checks.push(identity_check(cli));
    checks.push(git_check());

    let start = cli.repo.clone().or_else(|| std::env::current_dir().ok());
    match start.as_deref().and_then(Layout::discover) {
        None => checks.push(match &cli.repo {
            Some(p) => check(
                "repository",
                Status::Fail,
                format!("{} is not an AgentGit repository", p.display()),
                Some("create one with `ag init`, or point -C at the repository"),
            ),
            None => check(
                "repository",
                Status::Skip,
                "not inside an AgentGit repository",
                Some("`ag init` creates one here; `ag clone URL` copies one from a server"),
            ),
        }),
        Some(layout) => repository_checks(&layout, a, &mut checks),
    }

    let mut summary = Counts::default();
    for c in &checks {
        match c.status {
            Status::Ok => summary.ok += 1,
            Status::Warn => summary.warn += 1,
            Status::Fail => summary.fail += 1,
            Status::Skip => summary.skip += 1,
        }
    }
    let healthy = summary.fail == 0 && !(a.strict && summary.warn > 0);
    let report = Report { ok: healthy, version: env!("CARGO_PKG_VERSION"), checks, summary };
    ctx::emit(cli.json, &report, || render(&report))?;
    Ok(if healthy { exit::OK } else { exit::ERROR })
}

fn render(r: &Report) -> String {
    let w = r.checks.iter().map(|c| c.id.len()).max().unwrap_or(0);
    let mut s = String::new();
    for c in &r.checks {
        let (label, tone) = match c.status {
            Status::Ok => ("ok  ", Tone::Ok),
            Status::Warn => ("warn", Tone::Warn),
            Status::Fail => ("FAIL", Tone::Bad),
            Status::Skip => ("skip", Tone::Dim),
        };
        s.push_str(&format!("[{}] {:<w$}  {}\n", ui::tone(tone, label), c.id, c.summary));
        if let Some(h) = &c.hint {
            s.push_str(&format!("       {:<w$}  {} {h}\n", "", ui::tone(Tone::Hint, "->")));
        }
    }
    s.push_str(&format!(
        "\n{} ok, {} warning(s), {} failed, {} skipped.",
        r.summary.ok, r.summary.warn, r.summary.fail, r.summary.skip
    ));
    if !r.ok {
        s.push_str(" Fix the failed checks above and run `ag doctor` again.");
    }
    s
}

fn user_config_check() -> Check {
    match userconf::load() {
        Err(e) => check(
            "user_config",
            Status::Fail,
            e.to_string(),
            Some("edit or delete the file; `ag config path` prints it"),
        ),
        Ok(c) => {
            let path = userconf::path();
            match path.filter(|p| p.is_file()) {
                None => check("user_config", Status::Ok, "no user configuration file (defaults apply)", None),
                Some(p) => {
                    let set: Vec<&str> = ["identity", "color"].into_iter().filter(|k| c.get(k).is_some()).collect();
                    check(
                        "user_config",
                        Status::Ok,
                        format!("{} ({})", p.display(), if set.is_empty() { "empty".into() } else { set.join(", ") }),
                        None,
                    )
                }
            }
        }
    }
}

fn identity_check(cli: &Cli) -> Check {
    match identity_with_source(cli) {
        Err(e) => check(
            "identity",
            Status::Fail,
            e.to_string(),
            Some(
                "identities look like user:alice, agent:bot#run_… or service:ci; fix --as, AG_IDENTITY or `ag config set identity …`",
            ),
        ),
        Ok((id, src)) => {
            let hint = (src == ctx::IdentitySource::Default)
                .then_some("pin a stable identity with `ag config set identity user:NAME` or export AG_IDENTITY");
            check("identity", Status::Ok, format!("{id} (from {})", src.describe()), hint)
        }
    }
}

fn git_check() -> Check {
    match std::process::Command::new("git").arg("--version").output() {
        Ok(o) if o.status.success() => {
            check("git", Status::Ok, String::from_utf8_lossy(&o.stdout).trim().to_string(), None)
        }
        _ => check("git", Status::Skip, "not installed (only `ag git import|export` needs it)", None),
    }
}

fn repository_checks(layout: &Layout, a: &DoctorArgs, out: &mut Vec<Check>) {
    let root = layout.root().to_path_buf();
    let repo = match Repository::open(&root) {
        Ok(r) => r,
        Err(e) => {
            let msg = crate::errors::message_of(&anyhow::Error::from(e));
            out.push(check(
                "repository",
                Status::Fail,
                format!("{} cannot be opened: {msg}", root.display()),
                Some("`ag verify` and docs/operations/troubleshooting.md say what each message means"),
            ));
            return;
        }
    };
    out.push(check(
        "repository",
        Status::Ok,
        format!("{} (namespace {}, default stream {})", root.display(), repo.config().namespace, repo.default_stream()),
        None,
    ));
    out.push(check("storage", Status::Ok, format!("durability {}", repo.objects().durability().name), None));
    out.push(keys_check(&repo));
    out.push(integrity_check(&repo));
    out.push(operations_check(&repo));
    out.push(derive_check(&repo));
    out.push(references_check(&repo));
    let waiting: Vec<String> = repo
        .workspaces()
        .unwrap_or_default()
        .into_iter()
        .filter(|w| repo.workspace_is_merging(&w.name).unwrap_or(false))
        .map(|w| w.name)
        .collect();
    if !waiting.is_empty() {
        out.push(check(
            "workspace",
            Status::Warn,
            format!("a conflicted sync is waiting in workspace(s): {}", waiting.join(", ")),
            Some("resolve the files with conflict markers, then `ag snapshot -m \"resolve\" --workspace NAME`"),
        ));
    }
    match remote::load_remotes_if_any(&root) {
        Err(e) => out.push(check(
            "remotes",
            Status::Fail,
            format!("{e:#}"),
            Some("fix or delete .ag/remotes.json, then re-add with `ag remote add`"),
        )),
        Ok(rs) if rs.is_empty() => out.push(check(
            "remotes",
            Status::Skip,
            "no remotes configured",
            Some("`ag remote add origin http://host:8080/owner/name` adds one"),
        )),
        Ok(rs) => {
            for (name, r) in rs {
                out.push(remote_check(&root, &name, &r, a));
            }
        }
    }
}

fn keys_check(repo: &Repository) -> Check {
    let dir = repo.layout().keys_dir();
    let agents = repo.meta().read(|tx| tx.agents()).unwrap_or_default();
    if !dir.is_dir() {
        return if agents.is_empty() {
            check("keys", Status::Ok, "no agent or issuer keys yet", None)
        } else {
            check(
                "keys",
                Status::Warn,
                format!("{} registered agent(s) but no key directory at {}", agents.len(), dir.display()),
                Some("restore the keys, or set AG_KEY_DIR to where they are"),
            )
        };
    }
    let mut files = 0usize;
    let mut open_perms: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                files += 1;
                if readable_by_others(&e) {
                    open_perms.push(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
    }
    let missing: Vec<&str> =
        agents.iter().filter(|a| !dir.join(format!("{}.json", a.name)).is_file()).map(|a| a.name.as_str()).collect();
    if !open_perms.is_empty() {
        return check(
            "keys",
            Status::Warn,
            format!(
                "{} key file(s) in {} can be read by other users: {}",
                open_perms.len(),
                dir.display(),
                open_perms.join(", ")
            ),
            Some("run `chmod 600` on them; keys must stay out of untrusted environments"),
        );
    }
    if !missing.is_empty() {
        return check(
            "keys",
            Status::Warn,
            format!("no key file for registered agent(s): {}", missing.join(", ")),
            Some("these agents cannot sign provenance; restore their keys or re-register them"),
        );
    }
    // Only claim what was looked at: modes are checked on Unix, ACLs are not read back on Windows.
    let perms = if cfg!(unix) { ", permissions private" } else { "" };
    check("keys", Status::Ok, format!("{files} key file(s) in {}{perms}", dir.display()), None)
}

/// Whether group or world can read or write the file (mode `go+rwx`).
#[cfg(unix)]
fn readable_by_others(entry: &std::fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt;
    entry.metadata().is_ok_and(|m| m.permissions().mode() & 0o077 != 0)
}

/// Windows has no mode bits: `ag-crypto::private_file` writes keys with an owner-only ACL, which `std` cannot
/// read back, so there is nothing to compare here (and nothing to claim).
#[cfg(not(unix))]
fn readable_by_others(_entry: &std::fs::DirEntry) -> bool {
    false
}

fn integrity_check(repo: &Repository) -> Check {
    let run = || -> Result<(usize, u64)> {
        let streams = repo.meta().read(|tx| tx.list_streams())?;
        for s in &streams {
            if let Some(head) = s.state.head {
                if !repo.meta().read(|tx| tx.is_ready(&head))? {
                    anyhow::bail!("stream '{}' points at the unpublished changeset {}", s.name, head.short());
                }
                match repo.objects().verify(&head.object_id())? {
                    ag_storage::ObjectStatus::Ok { .. } => {}
                    other => anyhow::bail!("the head of stream '{}' ({}) is damaged: {other:?}", s.name, head.short()),
                }
            }
            let log = repo.meta().read(|tx| tx.reflog(s.name.as_str(), 1))?;
            match log.first() {
                Some(last) if last.new_head != s.state.head || last.version != s.state.version => {
                    anyhow::bail!("the reflog of stream '{}' does not end at its current head", s.name)
                }
                None if s.state.head.is_some() => anyhow::bail!("stream '{}' has a head but an empty reflog", s.name),
                _ => {}
            }
        }
        let audit = repo.meta().read(|tx| tx.audit_verify())?;
        if let Some(seq) = audit.first_bad {
            anyhow::bail!("the audit chain is broken at entry #{seq}");
        }
        Ok((streams.len(), audit.entries))
    };
    match run() {
        Ok((streams, entries)) => check(
            "integrity",
            Status::Ok,
            format!(
                "quick check: {streams} stream head(s) published and readable, reflogs consistent, audit chain intact ({entries} entries); `ag verify` re-hashes everything"
            ),
            None,
        ),
        Err(e) => check(
            "integrity",
            Status::Fail,
            format!("{e:#}"),
            Some("run `ag verify` for the full report; restore from `ag backup` or a mirror if it confirms damage"),
        ),
    }
}

/// The post-commit derive pass (tree cache, outbox): events that stayed pending, and why the last pass was
/// incomplete. Nothing is lost - the data is committed - but derived data is behind until a pass succeeds.
fn derive_check(repo: &Repository) -> Check {
    match repo.derive_status() {
        Err(e) => check(
            "derive",
            Status::Fail,
            format!("cannot read the derive status: {e}"),
            Some("the metadata store is failing: `ag verify` says more"),
        ),
        Ok(s) if s.pending == 0 && s.last_error.is_none() => {
            check("derive", Status::Ok, "no derive work pending, the last pass was complete", None)
        }
        Ok(s) => check(
            "derive",
            Status::Warn,
            format!(
                "{} derive event(s) pending{}",
                s.pending,
                s.last_error.map(|e| format!("; the last pass: {e}")).unwrap_or_default()
            ),
            Some("the next write retries them; `ag reindex` rebuilds the derived data now"),
        ),
    }
}

/// Pins, open proposals and workspaces that point at a changeset the object store cannot produce
/// (`dangling_pin`, `dangling_proposal_head`, `dangling_workspace_ref`). Garbage collection starts from these
/// references and refuses to run while one is unreadable.
fn references_check(repo: &Repository) -> Check {
    match repo.dangling_refs() {
        Err(e) => check(
            "references",
            Status::Fail,
            format!("cannot check the references: {e}"),
            Some("the metadata store is failing: `ag verify` says more"),
        ),
        Ok(issues) if issues.is_empty() => check(
            "references",
            Status::Ok,
            "every pin, open proposal and workspace points at a readable changeset",
            None,
        ),
        Ok(issues) => {
            let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
            for i in &issues {
                *kinds.entry(i.kind.as_str()).or_default() += 1;
            }
            let counts: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
            check(
                "references",
                Status::Fail,
                format!("{}: {}", counts.join(", "), issues[0].detail),
                Some(
                    "garbage collection refuses to run until they are fixed; `ag verify` lists each one; restore the missing objects from `ag backup` or a mirror, or unpin / close what points at nothing",
                ),
            )
        }
    }
}

fn operations_check(repo: &Repository) -> Check {
    let now = repo.clock().now();
    let rows = repo.meta().read(|tx| tx.intents_recent(500)).unwrap_or_default();
    let interrupted = rows
        .iter()
        .filter(|r| {
            matches!(
                r.state,
                IntentState::Prepared | IntentState::Uploaded | IntentState::Verified | IntentState::Ready
            ) && now.as_millis().saturating_sub(r.updated_at.as_millis()) > 10 * 60 * 1000
        })
        .count();
    let blocked = rows.iter().filter(|r| r.state == IntentState::Blocked).count();
    if interrupted > 0 {
        return check(
            "operations",
            Status::Warn,
            format!("{interrupted} operation(s) look interrupted (in flight for over 10 minutes)"),
            Some("`ag recover` resumes them; `ag op list` shows them"),
        );
    }
    let note = if blocked > 0 {
        format!("{blocked} blocked operation(s) are waiting for validation, approval or a lease")
    } else {
        "none interrupted".to_string()
    };
    check("operations", Status::Ok, note, None)
}

fn remote_check(root: &Path, name: &str, r: &Remote, a: &DoctorArgs) -> Check {
    let id = format!("remote.{name}");
    let target = format!("{}/{}", r.url, r.repo);
    let (_, source) = match remote::credential_source(root, name, r) {
        Ok(x) => x,
        Err(e) => {
            return check(
                id,
                Status::Fail,
                format!("{target}: {e}"),
                Some(
                    "export AG_TOKEN (or AG_TOKEN_<REMOTE>), or store a token with `--token-stdin`; `ag help config` lists the order",
                ),
            );
        }
    };
    if a.offline {
        return check(
            id,
            Status::Skip,
            format!("{target}: credential found ({source}); server not contacted (--offline)"),
            None,
        );
    }
    let timeout = Duration::from_secs(a.timeout.max(1));
    let probe = async {
        let client = remote::connect_with_timeout(root, name, r, timeout).await?;
        let neg = client.negotiate().await.ok();
        let who = client.protocol().whoami(client.req(v1::WhoamiRequest {})).await;
        let repo = client.repos().describe(client.req(v1::GetRepoRequest { repo: r.repo.clone() })).await;
        Ok::<_, anyhow::Error>((neg, who, repo))
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return check(id, Status::Fail, format!("cannot start the network runtime: {e}"), None),
    };
    let outcome = rt.block_on(async { tokio::time::timeout(timeout + Duration::from_secs(2), probe).await });
    let (neg, who, repo) = match outcome {
        Err(_) => {
            return check(
                id,
                Status::Fail,
                format!("{target}: no answer within {}s", timeout.as_secs()),
                Some("is the server up and the URL right? try `--timeout 30`, or `curl -sS URL/readyz`"),
            );
        }
        Ok(Err(e)) => {
            let msg = crate::errors::message_of(&e);
            let hint = if msg.contains("cannot reach") {
                "is `ag-server` running at that URL, and is the port open? for https with a private CA add `--ca-file` to the remote"
            } else {
                "check the URL and the credential: `ag remote list`, `ag help config`"
            };
            return check(id, Status::Fail, format!("{target}: {msg}"), Some(hint));
        }
        Ok(Ok(x)) => x,
    };
    let p = match who {
        Ok(p) => p.into_inner(),
        Err(status) => {
            let e = ag_client::ClientError::from(status);
            let (summary, hint) = match e.code() {
                Some(ag_protocol::ErrorCode::Unauthenticated) => (
                    format!(
                        "{target}: the server rejected the credential ({source}): expired, revoked, or minted by another server"
                    ),
                    "mint a new credential on the server (`ag-server admin mint …`) and export it as AG_TOKEN",
                ),
                _ => (format!("{target}: {e}"), "run again with --verbose, or check the server's logs"),
            };
            return check(id, Status::Fail, summary, Some(hint));
        }
    };
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let mut facts = vec![format!("authenticated as {} via {} ({source})", p.identity, p.method)];
    if !p.roles.is_empty() {
        facts.push(format!("roles {}", p.roles.join(", ")));
    }
    let mut status = Status::Ok;
    let mut hint: Option<String> = None;
    if p.expires_at_ms > 0 {
        let left = p.expires_at_ms - now_ms;
        facts.push(format!("expires in {}", human_duration(left.max(0))));
        if left < 24 * 3_600_000 {
            status = Status::Warn;
            hint = Some("mint a fresh credential before it expires".into());
        }
    } else {
        facts.push("does not expire".into());
    }
    if let Some(s) = &p.scope {
        let mut parts = Vec::new();
        if !s.repos.is_empty() {
            parts.push(format!("repos {}", s.repos.join(",")));
        }
        if !s.streams.is_empty() {
            parts.push(format!("streams {}", s.streams.join(",")));
        }
        if !s.operations.is_empty() {
            parts.push(format!("operations {}", s.operations.join(",")));
        }
        if !parts.is_empty() {
            facts.push(format!("scope: {}", parts.join("; ")));
        }
    }
    if p.method == "bootstrap" {
        status = Status::Warn;
        hint = Some("the bootstrap token is for first setup; mint a scoped, expiring credential for daily use".into());
    }
    let mut head = target.clone();
    if let Some(n) = neg {
        head.push_str(&format!(" (server {}, {})", n.server_version, n.protocol_version));
    }
    match repo {
        Ok(rp) => facts.push(format!("repository visible (default stream {})", rp.into_inner().default_stream)),
        Err(status_err) => {
            let e = ag_client::ClientError::from(status_err);
            return check(
                id,
                Status::Fail,
                format!("{head}: {}; but the repository '{}' is not usable with it: {e}", facts[0], r.repo),
                Some(
                    "check the repository name in `ag remote list` and that your credential's scope and roles include it",
                ),
            );
        }
    }
    check(id, status, format!("{head}: {}", facts.join("; ")), hint.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(human_duration(45_000), "45s");
        assert_eq!(human_duration(5 * 60_000), "5m");
        assert_eq!(human_duration(3 * 3_600_000 + 20 * 60_000), "3h 20m");
        assert_eq!(human_duration(2 * 86_400_000 + 5 * 3_600_000), "2d 5h");
    }
}
