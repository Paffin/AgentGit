//! Policy and validation commands.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use ag_agent::registry::KeyStore;
use ag_crypto::{KeyPair, random_id};
use ag_policy::{Attestation, CheckResult};
use ag_types::{Identity, TreeId};
use anyhow::{Context, Result, anyhow, bail};
use clap::Subcommand;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::cmds::StreamSel;
use crate::ctx::Ctx;
use crate::exit;

const EXAMPLE_POLICY: &str = r#"{
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "<64 hex chars from `ag validate keygen ci`>" }
  ],
  "required_validations": [
    { "check": "tests", "issuers": ["service:ci"], "max_age_secs": 86400 }
  ],
  "approvals": { "min": 1, "min_for_agents": 1, "forbid_self_approval": true },
  "protected_paths": [
    { "glob": ".github/**", "min_approvals": 2 },
    { "glob": "SECURITY.md", "min_approvals": 2 }
  ],
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 10 },
  "require_provenance_for_agents": true,
  "max_changed_files": 500
}"#;

#[derive(Subcommand, Debug)]
pub enum PolicyCmd {
    /// Show the policy in force.
    Show {
        #[command(flatten)]
        stream: StreamSel,
    },
    /// Replace the policy with the contents of a JSON file (needs an admin).
    Set {
        /// JSON policy file.
        file: PathBuf,
        /// Stream (default: the default stream).
        #[arg(long)]
        stream: Option<String>,
    },
    /// List policy versions.
    History {
        #[command(flatten)]
        stream: StreamSel,
    },
    /// Print an example policy document.
    Example,
}

pub fn policy(ctx: &Ctx, cmd: &PolicyCmd) -> Result<u8> {
    match cmd {
        PolicyCmd::Show { stream } => {
            let s = stream.name(ctx);
            let (p, v) = ctx.repo.policy(&s)?;
            ctx.out(&json!({"stream": s, "version": v, "policy": p}), || {
                format!(
                    "# policy v{v} of stream '{s}'{}\n{}",
                    if v == 0 { " (built-in default; nothing stored yet)" } else { "" },
                    p.to_json()
                )
            })?;
        }
        PolicyCmd::Set { file, stream } => {
            let s = stream.clone().unwrap_or_else(|| ctx.default_stream());
            let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
            let before = ctx.repo.policy(&s)?.1;
            let row = ctx.repo.set_policy(&s, &text, &ctx.me)?;
            ctx.done(&row, || {
                if before == row.version {
                    // The same document again is not a new version (and invalidates no attestation).
                    format!(
                        "Policy of '{s}' is unchanged: still version {} (hash {})",
                        row.version,
                        &row.doc_hash[..12]
                    )
                } else {
                    format!("Policy of '{s}' is now version {} (hash {})", row.version, &row.doc_hash[..12])
                }
            })?;
        }
        PolicyCmd::History { stream } => {
            let s = stream.name(ctx);
            let h = ctx.repo.policy_history(&s)?;
            if h.is_empty() {
                ctx.note(format!("no policy has been stored for '{s}' yet (the built-in default applies); `ag policy example` shows a document to start from"));
            }
            ctx.out(&h, || {
                let mut t = String::new();
                for r in &h {
                    let _ =
                        writeln!(t, "v{:<3} {}  {}  by {}", r.version, &r.doc_hash[..12], r.created_at, r.created_by);
                }
                t
            })?;
        }
        PolicyCmd::Example => crate::ui::print_line(EXAMPLE_POLICY),
    }
    Ok(exit::OK)
}

#[derive(Subcommand, Debug)]
pub enum ValidateCmd {
    /// Generate a signing key for an issuer and print its public key.
    Keygen {
        /// Key name, e.g. `ci`.
        name: String,
    },
    /// Check out the exact candidate tree of a proposal, run a command on it
    /// and record a signed attestation of the result.
    ///
    /// WARNING: the command runs on this machine without a sandbox. This
    /// runner is for trusted code and local workflows; use an isolated
    /// execution plane for untrusted code.
    Run {
        /// Proposal id.
        proposal: String,
        /// Check name, as listed in the policy's `required_validations`.
        #[arg(long)]
        check: String,
        /// Name of the issuer key created with `ag validate keygen`.
        #[arg(long)]
        key: String,
        /// Issuer identity (default: `service:<key name>`).
        #[arg(long)]
        issuer: Option<String>,
        /// Environment digest to record (default: derived from the command and platform).
        #[arg(long)]
        env: Option<String>,
        /// The command and its arguments.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// List attestations recorded for a tree.
    List {
        /// Tree id (64 hex characters).
        tree: String,
    },
}

pub fn validate(ctx: &Ctx, cmd: &ValidateCmd) -> Result<u8> {
    match cmd {
        ValidateCmd::Keygen { name } => {
            ag_agent::registry::validate_name(name)?;
            let keys = KeyStore::open(ctx.repo.layout().keys_dir())?;
            let kp = keys.create(&format!("issuer-{name}"))?;
            let pk = kp.public().to_hex();
            // With --quiet: just the public key, so `KEY=$(ag -q validate keygen ci)` works.
            ctx.created(&pk, &json!({"name": name, "identity": format!("service:{name}"), "public_key": pk, "key_file": keys.path(&format!("issuer-{name}"))}), || {
                format!(
                    "Created issuer key '{name}' ({}).\nAdd this to your policy's trusted_issuers:\n  {{ \"identity\": \"service:{name}\", \"public_key\": \"{pk}\" }}\nKeep the key file out of any environment that runs untrusted code.",
                    keys.path(&format!("issuer-{name}")).display()
                )
            })?;
        }
        ValidateCmd::Run { proposal, check, key, issuer, env, command } => {
            let prop = ctx.repo.find_proposal(proposal)?;
            let proposal = &prop.id;
            let pv = ctx.repo.merge_preview(proposal)?;
            if !pv.conflicts.is_empty() || pv.problem.is_some() {
                bail!(
                    "the proposal cannot be integrated as it stands ({}); fix that before validating",
                    pv.problem.unwrap_or_else(|| format!("{} conflict(s)", pv.conflicts.len()))
                );
            }
            let tree = pv.tree.ok_or_else(|| anyhow!("nothing to validate: the proposal is already integrated"))?;
            let tree_id = tree.id();
            let (_, policy_version) = ctx.repo.policy(&prop.stream)?;

            let work = std::env::temp_dir().join(format!("ag-validate-{}", random_id("v")?));
            std::fs::create_dir(&work)?;
            let result = (|| -> Result<bool> {
                ag_integrate::workdir::checkout_tree(&ctx.repo, &tree, &work)?;
                eprintln!("running `{}` on tree {} (NOT sandboxed)", command.join(" "), tree_id.short());
                let mut cmd = Command::new(&command[0]);
                cmd.args(&command[1..])
                    .current_dir(&work)
                    .env_clear()
                    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                    .env("HOME", &work)
                    .env("AG_TREE_ID", tree_id.to_hex())
                    .env("AG_PROPOSAL", proposal);
                // A cleared environment is not enough to run anything on Windows: the system root, the command
                // interpreter and the file-extension list are what `cmd`, PowerShell and most tools need.
                // The user's profile and temp directories are replaced by the scratch directory.
                #[cfg(windows)]
                {
                    for var in ["SystemRoot", "SystemDrive", "ComSpec", "PATHEXT", "windir"] {
                        if let Some(v) = std::env::var_os(var) {
                            cmd.env(var, v);
                        }
                    }
                    cmd.env("USERPROFILE", &work).env("TEMP", &work).env("TMP", &work);
                }
                let status = cmd.status().with_context(|| format!("failed to start `{}`", command[0]))?;
                Ok(status.success())
            })();
            let _ = std::fs::remove_dir_all(&work);
            let passed = result?;

            let keys = KeyStore::open(ctx.repo.layout().keys_dir())?;
            let kp: KeyPair = keys.load(&format!("issuer-{key}"))?;
            let issuer = match issuer {
                Some(i) => Identity::new(i.clone()).map_err(|e| anyhow!("{e}"))?,
                None => Identity::new(format!("service:{key}")).map_err(|e| anyhow!("{e}"))?,
            };
            let env_digest = env.clone().unwrap_or_else(|| {
                let mut h = Sha256::new();
                h.update(command.join("\u{1f}").as_bytes());
                h.update(std::env::consts::OS.as_bytes());
                h.update(std::env::consts::ARCH.as_bytes());
                format!("sha256:{}", hex::encode(h.finalize()))
            });
            let att = Attestation::sign(
                &kp,
                random_id("att")?,
                tree_id,
                check.clone(),
                if passed { CheckResult::Pass } else { CheckResult::Fail },
                policy_version,
                env_digest,
                issuer,
                ctx.repo.clock().now(),
            )?;
            ctx.repo.submit_attestation(&att)?;
            let report =
                |ctx: &Ctx, text: &dyn Fn() -> String| if passed { ctx.done(&att, text) } else { ctx.out(&att, text) };
            report(ctx, &|| {
                format!(
                    "check '{check}' {} on tree {} (policy v{policy_version}); attestation {} signed by {}",
                    if passed { "PASSED" } else { "FAILED" },
                    tree_id.short(),
                    att.id,
                    att.issuer
                )
            })?;
            return Ok(if passed { exit::OK } else { exit::REFUSED });
        }
        ValidateCmd::List { tree } => {
            let t: TreeId = tree.parse().map_err(|e| anyhow!("bad tree id: {e}"))?;
            let rows = ctx.repo.attestations(&t)?;
            ctx.out(&rows, || {
                let mut s = String::new();
                for r in &rows {
                    let _ = writeln!(
                        s,
                        "{}  {:<6} {:<4} policy v{}  {}  {}",
                        r.id, r.check_name, r.result, r.policy_version, r.issuer, r.issued_at
                    );
                }
                s
            })?;
        }
    }
    Ok(exit::OK)
}
