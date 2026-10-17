//! What a checkout destination can hold, and what to do about the rest.
//!
//! A tree is the same on every machine; a directory is not. [`Platform`] describes the destination (does it
//! have symlinks? an executable bit? does it fold case?), and the functions here decide, before a single
//! file is written, whether the tree can be materialised faithfully:
//!
//! * names the platform cannot represent or that would merge ([`ag_types::portable`]) **refuse** the whole
//!   checkout — a partial checkout would silently drop files, and writing them anyway would overwrite a
//!   sibling or reach a device;
//! * the two things a platform may simply lack, symlinks and the executable bit, are **adapted**, loudly:
//!   a symlink becomes a plain file holding its target, an executable file is written without the bit.
//!   `scan` understands both adaptations (it is given the tree the directory was checked out from), so
//!   `status` does not report them as changes.
//!
//! Nothing here depends on the clock or on randomness except the case-sensitivity probe, which only reads
//! the file system it is asked about.

use std::fs;
use std::path::Path;

use ag_tree::Tree;
use ag_types::FileType;
use ag_types::portable::{self, Findings, Target};

use crate::{Error, Result};

/// The capabilities of a checkout destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    /// Name rules and case/normalisation folding.
    pub target: Target,
    /// Symlinks can be created. If not they are written as plain files holding the target path.
    pub symlinks: bool,
    /// Files have an executable bit. If not it cannot be applied (and is remembered from the tree).
    pub exec_bit: bool,
}

impl Platform {
    /// Linux and other POSIX systems with a case-sensitive file system.
    pub const POSIX: Platform = Platform { target: Target::POSIX, symlinks: true, exec_bit: true };
    /// macOS with a default (case-insensitive, normalisation-insensitive) volume.
    pub const MACOS: Platform = Platform { target: Target::MACOS, symlinks: true, exec_bit: true };
    /// Windows with a default NTFS volume. Symlinks need a privilege most users do not have, so they are never
    /// attempted; see `docs/operations/windows.md`.
    pub const WINDOWS: Platform = Platform { target: Target::WINDOWS, symlinks: false, exec_bit: false };

    /// What the platform this program was built for normally gives.
    pub const fn host() -> Platform {
        if cfg!(windows) {
            Self::WINDOWS
        } else if cfg!(target_os = "macos") {
            Self::MACOS
        } else {
            Self::POSIX
        }
    }

    /// The host platform, with case sensitivity measured on `dir` itself: a case-sensitive APFS volume, a
    /// case-sensitive directory on NTFS (WSL) or a case-insensitive mount on Linux all differ from the default.
    /// If the directory cannot be probed the host default stands.
    pub fn for_dir(dir: &Path) -> Platform {
        let mut p = Self::host();
        if let Ok(ci) = probe_case_insensitive(dir) {
            p.target.case_insensitive = ci;
        }
        p
    }

    /// A platform by name, for `--target` and for tests: `windows`, `macos` or `linux` (also `posix`).
    pub fn named(name: &str) -> Option<Platform> {
        match name.to_ascii_lowercase().as_str() {
            "windows" | "win" => Some(Self::WINDOWS),
            "macos" | "mac" | "darwin" => Some(Self::MACOS),
            "linux" | "posix" | "unix" => Some(Self::POSIX),
            _ => None,
        }
    }

    /// A short name for messages.
    pub fn name(&self) -> &'static str {
        if self.target.windows_names {
            "Windows"
        } else if self.target.normalization_insensitive {
            "macOS"
        } else {
            "Linux"
        }
    }
}

/// Whether `dir` treats names that differ only in case as the same file, found by creating a file and looking
/// for it under another spelling.
pub fn probe_case_insensitive(dir: &Path) -> std::io::Result<bool> {
    let id = ag_crypto::random_id("p").map_err(|e| std::io::Error::other(e.to_string()))?;
    let lower = dir.join(format!(".ag-case-probe-{id}"));
    let upper = dir.join(format!(".AG-CASE-PROBE-{}", id.to_uppercase()));
    fs::OpenOptions::new().write(true).create_new(true).open(&lower)?;
    let folded = fs::symlink_metadata(&upper).is_ok();
    let _ = fs::remove_file(&lower);
    Ok(folded)
}

/// Everything in `tree` that `platform` cannot hold.
pub fn inspect_tree(tree: &Tree, platform: &Platform) -> Findings {
    portable::inspect(tree.iter().map(|(p, _)| p), &platform.target)
}

/// Refuses a tree that `platform` cannot hold, naming what is wrong. Called before anything is written.
pub fn ensure_checkout_possible(tree: &Tree, platform: &Platform) -> Result<()> {
    let findings = inspect_tree(tree, platform);
    if findings.is_clean() {
        return Ok(());
    }
    let mut msg = format!("this tree cannot be checked out on a {} file system:", platform.name());
    for line in findings.describe(10) {
        msg.push_str("\n  ");
        msg.push_str(&line);
    }
    msg.push_str(
        "\nNothing was written. Rename or remove these paths in the repository (from a platform that can hold \
         them), or check the tree out on one that can; `ag portability` lists them for any revision.",
    );
    Err(Error::UnsafePath(msg))
}

/// What a checkout adapted because the platform lacks it. Computed from the tree, not recorded while writing,
/// so it describes the directory whatever sequence of operations produced it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Adaptations {
    /// Symlinks that exist as plain files holding the target path.
    pub symlinks_as_files: Vec<String>,
    /// Files that are executable in the tree but cannot be marked executable here.
    pub executables_without_bit: Vec<String>,
}

impl Adaptations {
    /// Nothing was adapted.
    pub fn is_empty(&self) -> bool {
        self.symlinks_as_files.is_empty() && self.executables_without_bit.is_empty()
    }

    /// A short, loud, human-readable account; empty if nothing was adapted.
    pub fn describe(&self) -> Vec<String> {
        const SHOW: usize = 5;
        let list = |v: &[String]| {
            let mut s = v.iter().take(SHOW).cloned().collect::<Vec<_>>().join(", ");
            if v.len() > SHOW {
                s.push_str(&format!(", ... ({} more)", v.len() - SHOW));
            }
            s
        };
        let mut out = Vec::new();
        if !self.symlinks_as_files.is_empty() {
            out.push(format!(
                "{} symlink(s) were written as plain files containing the link target, because this platform \
                 cannot create symlinks: {}",
                self.symlinks_as_files.len(),
                list(&self.symlinks_as_files)
            ));
        }
        if !self.executables_without_bit.is_empty() {
            out.push(format!(
                "{} executable file(s) have no executable bit here (this platform has none; the repository still \
                 records it): {}",
                self.executables_without_bit.len(),
                list(&self.executables_without_bit)
            ));
        }
        out
    }
}

/// What materialising `tree` on `platform` adapts.
pub fn adaptations(tree: &Tree, platform: &Platform) -> Adaptations {
    let mut a = Adaptations::default();
    for (path, entry) in tree.iter() {
        match entry.file_type {
            FileType::Symlink if !platform.symlinks => a.symlinks_as_files.push(path.to_string()),
            FileType::Regular if entry.executable && !platform.exec_bit => {
                a.executables_without_bit.push(path.to_string())
            }
            _ => {}
        }
    }
    a
}
