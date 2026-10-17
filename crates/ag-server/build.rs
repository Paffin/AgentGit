//! Embeds the git commit and its date in `--version`: `ag 0.1.0 (a1b2c3d4e5f6 2026-10-17)`.
//!
//! The same file is used, byte for byte, by `ag-cli`, `ag-server` and `ag-worker`
//! (`crates/ag-cli/tests/version.rs` keeps them identical and exercises the fallbacks).
//!
//! Where the values come from, first match wins:
//!
//! * commit: `AG_BUILD_COMMIT` (container builds have no `.git`), else `git rev-parse HEAD`, else the
//!   `sha1` that `cargo package` records in `.cargo_vcs_info.json`;
//! * date: `AG_BUILD_DATE` (`YYYY-MM-DD`), else `SOURCE_DATE_EPOCH`, else the *commit* date
//!   (`git show -s --format=%cs`).
//!
//! The date is deliberately not the wall clock: building the same commit twice gives the same binary.
//! Without git (a crates.io tarball, a stripped container context) both fall back to `unknown`, and the
//! version string is just `0.1.0 (no git metadata)`.

use std::path::Path;
use std::process::Command;

fn git_unchecked(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Whether the git repository around the build is this workspace (and not, say, a home directory under
/// version control that happens to contain `~/.cargo/registry/src/…/ag-cli-0.1.0`).
fn in_own_checkout() -> bool {
    let name = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    let Some(top) = git_unchecked(&["rev-parse", "--show-toplevel"]) else { return false };
    Path::new(&top).join("crates").join(name).join("build.rs").is_file()
}

fn git(args: &[&str]) -> Option<String> {
    static OWN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    OWN.get_or_init(in_own_checkout).then(|| git_unchecked(args)).flatten()
}

/// The commit `cargo package` recorded in the tarball, if this is a packaged crate.
fn packaged_commit() -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let info = std::fs::read_to_string(Path::new(&dir).join(".cargo_vcs_info.json")).ok()?;
    let rest = info.split("\"sha1\"").nth(1)?;
    rest.split('"').nth(1).map(str::to_string)
}

/// A hex commit id shortened to 12 characters, or `None` for anything else.
fn clean_commit(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (raw.len() >= 7 && raw.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| raw[..raw.len().min(12)].to_ascii_lowercase())
}

/// `YYYY-MM-DD` for a count of days since 1970-01-01 (proleptic Gregorian; Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn clean_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let b = raw.as_bytes();
    let shaped = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    shaped.then(|| raw.to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=AG_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=AG_BUILD_DATE");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    // Re-run when the checked-out commit changes (works in worktrees too).
    if let (Some(gitdir), Some(common)) =
        (git(&["rev-parse", "--absolute-git-dir"]), git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]))
    {
        println!("cargo:rerun-if-changed={}", Path::new(&gitdir).join("HEAD").display());
        println!("cargo:rerun-if-changed={}", Path::new(&common).join("packed-refs").display());
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}", Path::new(&common).join(head).display());
        }
    }

    let commit = std::env::var("AG_BUILD_COMMIT")
        .ok()
        .and_then(|c| clean_commit(&c))
        .or_else(|| git(&["rev-parse", "HEAD"]).and_then(|c| clean_commit(&c)))
        .or_else(|| packaged_commit().and_then(|c| clean_commit(&c)));
    let date = std::env::var("AG_BUILD_DATE")
        .ok()
        .and_then(|d| clean_date(&d))
        .or_else(|| {
            let secs: i64 = std::env::var("SOURCE_DATE_EPOCH").ok()?.trim().parse().ok()?;
            let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
            Some(format!("{y:04}-{m:02}-{d:02}"))
        })
        .or_else(|| git(&["show", "-s", "--format=%cs", "HEAD"]).and_then(|d| clean_date(&d)));

    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let long = match (&commit, &date) {
        (Some(c), Some(d)) => format!("{pkg} ({c} {d})"),
        (Some(c), None) => format!("{pkg} ({c})"),
        _ => format!("{pkg} (no git metadata)"),
    };
    println!("cargo:rustc-env=AG_GIT_COMMIT={}", commit.as_deref().unwrap_or("unknown"));
    println!("cargo:rustc-env=AG_BUILD_DATE={}", date.as_deref().unwrap_or("unknown"));
    println!("cargo:rustc-env=AG_LONG_VERSION={long}");
}
