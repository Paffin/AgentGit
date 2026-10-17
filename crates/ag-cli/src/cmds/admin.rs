//! Audit, integrity, retention and maintenance commands.

use std::fmt::Write as _;

use ag_integrate::GcOptions;
use anyhow::{Result, anyhow};
use clap::Args;
use serde_json::json;

use crate::ctx::{Ctx, parse_duration_ms};
use crate::exit;

#[derive(Args, Debug)]
pub struct TraceArgs {
    /// What to trace: a proposal id, run id, task id, stream, agent identity or changeset (prefix). Default: everything.
    pub target: Option<String>,
    /// Maximum entries.
    #[arg(short = 'n', long, default_value_t = 50)]
    pub limit: usize,
}

pub fn trace(ctx: &Ctx, a: &TraceArgs) -> Result<u8> {
    let needle = a.target.as_ref().map(|t| ctx.repo.resolve_rev(t).map(|c| c.to_hex()).unwrap_or_else(|_| t.clone()));
    let entries = ctx.repo.meta().read(|tx| tx.audit_list(needle.as_deref(), a.limit))?;
    ctx.out(&entries, || {
        let mut s = String::new();
        for e in entries.iter().rev() {
            let _ = writeln!(
                s,
                "#{:<5} {}  {:<22} {:<28} {}  {}",
                e.seq, e.timestamp, e.action, e.actor, e.target, e.details
            );
        }
        s
    })?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct AuditArgs {
    /// List recent entries instead of verifying the chain.
    #[arg(long)]
    pub list: bool,
    /// Number of entries for `--list`.
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: usize,
}

pub fn audit(ctx: &Ctx, a: &AuditArgs) -> Result<u8> {
    if a.list {
        return trace(ctx, &TraceArgs { target: None, limit: a.limit });
    }
    let rep = ctx.repo.meta().read(|tx| tx.audit_verify())?;
    ctx.out(&rep, || match rep.first_bad {
        None => format!("Audit chain intact: {} entries, head {}.\nRecord the head hash somewhere you trust to detect later rollbacks.", rep.entries, rep.head),
        Some(seq) => format!("AUDIT CHAIN BROKEN at entry #{seq} ({} entries checked).", rep.entries),
    })?;
    Ok(if rep.first_bad.is_none() { exit::OK } else { exit::ERROR })
}

pub fn verify(ctx: &Ctx) -> Result<u8> {
    let rep = ctx.repo.fsck()?;
    ctx.out(&rep, || {
        let mut s = format!(
            "{} object(s) hashed, {} changeset(s) cross-checked, {} head tree(s) recomputed, {} audit entries.\n",
            rep.objects_checked, rep.changesets_checked, rep.heads_recomputed, rep.audit_entries
        );
        if rep.is_ok() {
            s.push_str("Repository is healthy.");
        } else {
            let _ = writeln!(s, "{} problem(s):", rep.issues.len());
            for i in &rep.issues {
                let _ = writeln!(s, "  [{}] {}", i.kind, i.detail);
            }
        }
        s
    })?;
    Ok(if rep.is_ok() { exit::OK } else { exit::ERROR })
}

#[derive(Args, Debug)]
pub struct GcArgs {
    /// Keep unreachable objects younger than this.
    #[arg(long, default_value = "24h")]
    pub grace: String,
    /// Keep heads that appeared in a reflog within this window.
    #[arg(long, default_value = "90d")]
    pub reflog_retention: String,
    /// Report only; delete nothing.
    #[arg(long)]
    pub dry_run: bool,
}

pub fn gc(ctx: &Ctx, a: &GcArgs) -> Result<u8> {
    let rep = ctx.repo.gc(
        &GcOptions {
            grace_ms: parse_duration_ms(&a.grace)?,
            reflog_retention_ms: parse_duration_ms(&a.reflog_retention)?,
            dry_run: a.dry_run,
        },
        ctx.me.as_str(),
    )?;
    ctx.out(&rep, || {
        format!(
            "{}{} unreachable object(s), {} byte(s); kept {} young and {} staged. Reachable: {} changeset(s), {} blob(s).",
            if rep.dry_run { "[dry run] would delete " } else { "Deleted " },
            rep.deleted.len(), rep.bytes, rep.kept_young, rep.kept_staged, rep.reachable_changesets, rep.reachable_blobs
        )
    })?;
    Ok(exit::OK)
}

pub fn reindex(ctx: &Ctx) -> Result<u8> {
    let n = ctx.repo.reindex()?;
    ctx.out(&json!({"changesets": n}), || format!("Rebuilt derived indexes for {n} changeset(s)."))?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct PinArgs {
    /// Pin name.
    pub name: String,
    /// Revision to keep.
    pub rev: String,
    /// Why.
    #[arg(long, default_value = "")]
    pub reason: String,
}

pub fn pin(ctx: &Ctx, a: &PinArgs) -> Result<u8> {
    let id = ctx.repo.resolve_rev(&a.rev)?;
    ctx.repo.pin(&a.name, &id, &a.reason, &ctx.me)?;
    ctx.out(&json!({"pin": a.name, "changeset": id}), || format!("Pinned {} as '{}'", id.short(), a.name))?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct UnpinArgs {
    /// Pin name.
    pub name: String,
}

pub fn unpin(ctx: &Ctx, a: &UnpinArgs) -> Result<u8> {
    if !ctx.repo.unpin(&a.name, &ctx.me)? {
        return Err(anyhow!("no pin named '{}'", a.name));
    }
    ctx.out(&json!({"unpinned": a.name}), || format!("Removed pin '{}'", a.name))?;
    Ok(exit::OK)
}

pub fn pins(ctx: &Ctx) -> Result<u8> {
    let rows = ctx.repo.pins()?;
    ctx.out(&rows, || {
        let mut s = String::new();
        for p in &rows {
            let _ = writeln!(s, "{:<20} {}  {}", p.name, p.changeset.short(), p.reason);
        }
        s
    })?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct BackupArgs {
    /// Destination directory (new or empty).
    pub dest: std::path::PathBuf,
    /// Also copy private keys (agent and issuer identities). Off by default.
    #[arg(long)]
    pub include_keys: bool,
}

pub fn backup(ctx: &Ctx, a: &BackupArgs) -> Result<u8> {
    let rep = ctx.repo.backup(&a.dest, a.include_keys)?;
    ctx.out(&rep, || {
        format!(
            "Backed up {} object file(s), {} byte(s) to {}\nverification: {}{}",
            rep.objects,
            rep.bytes,
            a.dest.display(),
            if rep.verification.is_ok() {
                "healthy".to_string()
            } else {
                format!("{} problem(s)!", rep.verification.issues.len())
            },
            if rep.keys_included {
                "\nprivate keys were included: protect this backup"
            } else {
                "\nprivate keys were NOT included (use --include-keys)"
            }
        )
    })?;
    Ok(if rep.verification.is_ok() { exit::OK } else { exit::ERROR })
}
