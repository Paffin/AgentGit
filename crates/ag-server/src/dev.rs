//! `ag-server dev`: a zero-configuration server for trying AgentGit out.
//!
//! One command makes the data directory, an administrator with a fresh short-lived credential, a demo repository
//! with sample files and the `agents` policy preset, and starts the server on the loopback interface. Everything
//! here goes through the same code as production: the credential is a normal minted credential (hashed in the
//! control database), the repository is created by the repository manager, the sample files land through the
//! engine's own proposal and integration steps, and the policy is a stored policy version. What differs from a
//! real deployment is only what [`banner`] says: loopback only, plain HTTP, a credential that lives
//! [`TOKEN_TTL_SECS`] seconds and changes at every start, and a fixed demo policy.
//!
//! Starting again reuses the data directory: the repository, its history and any policy change you made stay;
//! only the credential is replaced (the previous one is revoked).

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ag_integrate::{IntegrateRequest, VirtualChange, VirtualContent};
use ag_types::{FileType, Identity, RepoPath};
use anyhow::{Context, Result, anyhow, bail};

use crate::app::App;
use crate::authz::Role;
use crate::control::CredentialRow;

/// The administrator `dev` signs you in as.
pub const ADMIN: &str = "user:dev";
/// The demo repository.
pub const DEMO_REPO: &str = "demo/welcome";
/// How long the credential lives (eight hours; a deployment's default is 24 hours, its cap 30 days).
pub const TOKEN_TTL_SECS: u32 = 8 * 3600;
/// Marks the credentials `dev` minted, so the next start can revoke them.
pub const DESCRIPTION: &str = "ag-server dev (replaced at every start)";

/// Refuses to listen anywhere but on the loopback interface unless that was asked for explicitly.
pub fn check_listen(listen: SocketAddr, allow_non_loopback: bool) -> Result<()> {
    if listen.ip().is_loopback() || allow_non_loopback {
        return Ok(());
    }
    bail!(
        "refusing to listen on {listen}: `ag-server dev` is for evaluation on one machine, serves plain HTTP and prints \
         a credential, so it stays on the loopback interface (127.0.0.1). If you really mean to (for example inside a \
         container whose port you publish only to 127.0.0.1), add --allow-non-loopback."
    )
}

/// The files of the demo repository: real, small, and chosen so that the preset has something to say about each.
pub fn sample_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "README.md",
            "# Welcome to AgentGit\n\nThis is a demo repository on your own `ag-server dev`. Try the loop:\n\n1. `ag clone` it (the command is printed when the server starts).\n2. Edit `src/greeting.py`, then `ag snapshot -m \"...\"`, `ag propose -t \"...\"`, `ag integrate <proposal>`.\n3. Ask what the policy lets an agent do alone: `ag policy explain`.\n",
        ),
        (
            "docs/guide.md",
            "# Guide\n\nDocumentation lives here. Under the demo policy an AI agent may change files in `docs/` and Markdown files\nby itself (up to ten at a time); anything else it proposes waits for a person to approve it.\n",
        ),
        (
            "src/greeting.py",
            "def greet(name: str) -> str:\n    return f\"Hello, {name}!\"\n\n\nif __name__ == \"__main__\":\n    print(greet(\"AgentGit\"))\n",
        ),
        (
            "SECURITY.md",
            "# Security\n\nThe demo policy protects this file: a change to it needs an approval from somebody other than its author.\n",
        ),
    ]
}

/// What [`bootstrap`] did and made.
#[derive(Debug)]
pub struct Bootstrapped {
    /// The new credential (a secret: shown once, kept in [`Bootstrapped::token_file`]).
    pub token: String,
    /// The private file that holds it.
    pub token_file: PathBuf,
    /// When it expires (milliseconds since the epoch).
    pub expires_at_ms: i64,
    /// The demo repository was created by this start (not found from an earlier one).
    pub repo_created: bool,
    /// The sample files were landed by this start.
    pub seeded: bool,
    /// The `agents` preset was stored by this start (an earlier start, or you, may have set a policy already).
    pub policy_applied: bool,
    /// Credentials of earlier starts that were revoked.
    pub revoked: usize,
}

/// Prepares everything a first visit needs. Safe to repeat: it reuses what exists and replaces only the credential.
/// Blocking (SQLite): call it from a blocking thread.
pub fn bootstrap(app: &App, data_dir: &Path) -> Result<Bootstrapped> {
    let me = Identity::new(ADMIN)?;
    let by = "system:dev";
    let now = app.clock.now().as_millis();

    // The administrator, and a credential that replaces the last one.
    app.control.grant_role(ADMIN, "*", Role::Admin, by).map_err(|e| anyhow!("{e}"))?;
    app.invalidate_bindings();
    let mut revoked = 0;
    for old in app.control.credentials(Some(ADMIN), false).map_err(|e| anyhow!("{e}"))?.rows {
        if old.description == DESCRIPTION && app.control.revoke_credential(&old.id, now).map_err(|e| anyhow!("{e}"))? {
            revoked += 1;
        }
    }
    let ttl = app.cfg.auth.credential_ttl(me.kind(), TOKEN_TTL_SECS).map_err(|e| anyhow!("{e}"))?;
    let minted = crate::auth::mint_token().map_err(|e| anyhow!("{e}"))?;
    let expires_at_ms = now + i64::from(ttl) * 1000;
    app.control
        .insert_credential(&CredentialRow {
            id: minted.credential_id.clone(),
            secret_hash: minted.secret_hash,
            subject: ADMIN.into(),
            kind: me.kind().prefix().into(),
            description: DESCRIPTION.into(),
            scope: Default::default(),
            created_at_ms: now,
            expires_at_ms,
            revoked_at_ms: None,
            last_used_ms: 0,
            created_by: by.into(),
        })
        .map_err(|e| anyhow!("{e}"))?;
    let token_file = write_token(data_dir, &minted.token)?;

    // The demo repository, its content, then its policy (so that the content lands under the open default).
    let repo_created = if app.control.repo(DEMO_REPO).map_err(|e| anyhow!("{e}"))?.is_none() {
        app.repos
            .create(
                DEMO_REPO,
                &app.cfg.repos.default_stream,
                "A demo repository made by `ag-server dev`",
                &me,
                app.default_quota(),
            )
            .map_err(|e| anyhow!("{e}"))?;
        true
    } else {
        false
    };
    let repo = app.repos.handle(DEMO_REPO).map_err(|e| anyhow!("{e}"))?;
    let stream = repo.default_stream().to_string();
    let head = repo.meta().read(|tx| tx.stream(&stream))?.and_then(|s| s.state.head);
    let seeded = if head.is_none() {
        seed(&repo, &stream, &me)?;
        true
    } else {
        false
    };
    let policy_applied = if repo.policy(&stream)?.1 == 0 {
        use ag_policy::presets::{Preset, PresetOptions};
        let policy = Preset::Agents.build(&PresetOptions { admins: vec![ADMIN.into()], ci: None })?;
        repo.set_policy(&stream, &policy.to_json(), &me)?;
        true
    } else {
        false
    };
    Ok(Bootstrapped { token: minted.token, token_file, expires_at_ms, repo_created, seeded, policy_applied, revoked })
}

/// Lands the sample files through a virtual workspace, a proposal and an integration.
fn seed(repo: &ag_integrate::Repository, stream: &str, me: &Identity) -> Result<()> {
    let stream_name = ag_types::StreamName::new(stream)?;
    repo.workspace_create("seed", &stream_name, me)?;
    let changes: Vec<VirtualChange> = sample_files()
        .into_iter()
        .map(|(path, text)| {
            Ok(VirtualChange::Put {
                path: RepoPath::new(path)?,
                content: VirtualContent::Bytes(text.as_bytes().to_vec()),
                executable: false,
                file_type: FileType::Regular,
            })
        })
        .collect::<Result<_>>()?;
    let ws = repo.workspace_modify("seed", &changes, "Sample content", me, None)?;
    let head = ws.head.ok_or_else(|| anyhow!("the sample content produced no changeset"))?;
    let proposal = repo.propose(stream, &head, "Sample content", "Made by `ag-server dev`", me, None)?;
    let result = repo.integrate(&IntegrateRequest {
        proposal_id: proposal.id,
        actor: me.clone(),
        idempotency_key: None,
        fencing_tokens: Vec::new(),
    })?;
    if result.outcome.status() != "integrated" {
        bail!("the sample content did not land: {:?}", result.outcome);
    }
    repo.workspace_remove("seed", me)?;
    Ok(())
}

/// Writes the credential where only you can read it: `<data-dir>/dev/token`.
fn write_token(data_dir: &Path, token: &str) -> Result<PathBuf> {
    let dir = data_dir.join("dev");
    ag_crypto::private_file::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("token");
    let mut f = ag_crypto::private_file::create(&path, false).with_context(|| format!("writing {}", path.display()))?;
    f.write_all(token.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    Ok(path)
}

/// The address to open in a browser. The credential is in the *fragment* (`#token=...`): a browser never sends a
/// fragment to the server, so it appears in no access log and no `Referer`; the Web UI reads it, signs in and
/// removes it from the address bar at once.
pub fn sign_in_url(base: &str, token: &str) -> String {
    format!("{}/repos/{DEMO_REPO}#token={token}", base.trim_end_matches('/'))
}

/// What to print once the server is listening.
pub fn banner(base: &str, b: &Bootstrapped, data_dir: &Path, ui: bool, expires: &str) -> String {
    let token_file = b.token_file.display();
    let mut s = String::new();
    s.push_str("AgentGit dev server: for trying AgentGit out, not for real work\n");
    s.push_str("  - loopback only (127.0.0.1), plain HTTP; do not expose this port\n");
    s.push_str(&format!(
        "  - one administrator ({ADMIN}); its credential lives {} hours, is replaced at every start and is kept in {token_file}\n",
        TOKEN_TTL_SECS / 3600
    ));
    s.push_str(&format!(
        "  - the demo repository {DEMO_REPO} has the `agents` policy preset (`ag remote policy main --explain` says what it means); data in {}\n",
        data_dir.display()
    ));
    s.push('\n');
    if ui {
        s.push_str(&format!(
            "Open in a browser (signs you in; the link carries the credential, so do not share it):\n  {}\n\n",
            sign_in_url(base, &b.token)
        ));
    } else {
        s.push_str("This build has no Web UI embedded; the API and the command line work.\n\n");
    }
    s.push_str("Command line:\n");
    s.push_str(&format!("  ag login {base} --token-file {token_file}\n"));
    s.push_str(&format!("  ag clone {base}/{DEMO_REPO}\n"));
    s.push_str(&format!(
        "\nCredential expires {expires}. Stop with Ctrl-C; start again with the same --data-dir to keep everything."
    ));
    s
}

/// Opens `url` in the default browser; says why if there is no way to.
pub fn open_browser(url: &str) -> Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map(|_| ()).map_err(|e| anyhow!("could not start a browser ({e}); open the link by hand"))
}
