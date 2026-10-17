//! Whether repository paths can be materialised safely on a given kind of file system.
//!
//! A [`RepoPath`] is always `/`-separated UTF-8 and means the same thing everywhere. Writing it to disk does
//! not: Windows reads `aux.txt` as a device, `a.` as `a`, `a:b` as a stream `b` of file `a` and `GIT~1` as the
//! short name of `.git`; macOS and default Windows volumes treat `README` and `readme` as one file. A tree
//! built on Linux can therefore contain paths that, checked out elsewhere, overwrite a sibling, hit a device,
//! or plant a `.git` directory. This module decides, with no I/O and no clock, which paths are such
//! hazards, so the same tests prove the rules on every platform.
//!
//! Two layers, deliberately separate:
//!
//! * [`is_reserved_component`] is the **universal** rule that [`RepoPath::new`] enforces: `.git` and `.ag`
//!   *and every spelling a file system folds into them* (`.GIT`, `.git.`, `.git `, `.git::$INDEX_ALLOCATION`,
//!   `git~1`, `.g\u{200c}it` on HFS+). Git applies the same rule on every platform, because a repository is
//!   shared between platforms.
//! * [`Target`] describes the *destination* file system. [`path_hazard`] and [`find_collisions`] apply the
//!   target-specific rules (device names, forbidden characters, case collisions) when a checkout is about to
//!   write. A path that is a hazard for Windows is perfectly valid data in the repository.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::RepoPath;

/// Components that may never appear in a repository tree (compared as described on [`is_reserved_component`]).
pub(crate) const RESERVED: [&str; 2] = [".ag", ".git"];

/// Code points that HFS+ ignores when it compares names, so `.g\u{200c}it` *is* `.git` there (git's
/// `protectHFS` list): U+200C–U+200F, U+202A–U+202E, U+206A–U+206F, U+FEFF.
fn is_hfs_ignorable(c: char) -> bool {
    matches!(c, '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}')
}

/// True if `rest` — what follows a reserved name — is only what NTFS drops (spaces and dots), optionally
/// followed by an alternate-data-stream suffix (`:` and anything after it).
fn only_ntfs_noise(rest: &str) -> bool {
    let body = rest.split(':').next().unwrap_or("");
    body.chars().all(|c| c == ' ' || c == '.')
}

/// Whether `comp` names one of the reserved directories, under any spelling a file system folds into it.
///
/// Matches `.git` / `.ag` ASCII case-insensitively, plus:
///
/// * NTFS: trailing dots and spaces (`.git.`, `.git `), an alternate-data-stream suffix (`.git::$INDEX_ALLOCATION`,
///   `.git:x`) and the 8.3 short name (`git~1`, `GIT~1.`, any `~<digits>`);
/// * HFS+: the code points the file system ignores when comparing names.
///
/// `.gitignore`, `.git-notes` and `.agignore` are *not* reserved.
pub fn is_reserved_component(comp: &str) -> bool {
    let folded: String;
    let comp = if comp.chars().any(is_hfs_ignorable) {
        folded = comp.chars().filter(|c| !is_hfs_ignorable(*c)).collect();
        folded.as_str()
    } else {
        comp
    };
    RESERVED.iter().any(|dotted| {
        let name = &dotted[1..]; // "git" / "ag"
        let ci_prefix = |s: &str, p: &str| {
            s.len() >= p.len() && s.is_char_boundary(p.len()) && s[..p.len()].eq_ignore_ascii_case(p)
        };
        if ci_prefix(comp, dotted) && only_ntfs_noise(&comp[dotted.len()..]) {
            return true;
        }
        // 8.3 short name of the dot-directory: NAME~<digits>
        if ci_prefix(comp, name) {
            let rest = &comp[name.len()..];
            if let Some(after_tilde) = rest.strip_prefix('~') {
                let digits = after_tilde.chars().take_while(char::is_ascii_digit).count();
                return digits > 0 && only_ntfs_noise(&after_tilde[digits..]);
            }
        }
        false
    })
}

/// What the destination file system is like. Decided by the platform, or probed from the directory being
/// written (see `ag_integrate::workdir`), and passed in explicitly so tests can exercise every rule anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// Win32 name rules: device names, `<>:"|?*`, trailing dots and spaces are not usable.
    pub windows_names: bool,
    /// Names that differ only in case are the same file (default NTFS, APFS, HFS+, FAT).
    pub case_insensitive: bool,
    /// Canonically equivalent Unicode spellings (`é` as one code point or two) are the same name (APFS, HFS+).
    pub normalization_insensitive: bool,
}

impl Target {
    /// A case-sensitive POSIX file system (ext4, xfs, APFS case-sensitive volumes, ...).
    pub const POSIX: Target =
        Target { windows_names: false, case_insensitive: false, normalization_insensitive: false };
    /// A Windows volume with default settings (NTFS or ReFS).
    pub const WINDOWS: Target =
        Target { windows_names: true, case_insensitive: true, normalization_insensitive: false };
    /// A default macOS volume (APFS or HFS+, case-insensitive).
    pub const MACOS: Target = Target { windows_names: false, case_insensitive: true, normalization_insensitive: true };

    /// What the platform this program was built for normally gives: used when the directory cannot be probed.
    pub const fn host() -> Target {
        if cfg!(windows) {
            Self::WINDOWS
        } else if cfg!(target_os = "macos") {
            Self::MACOS
        } else {
            Self::POSIX
        }
    }
}

/// Why a path cannot be written on a [`Target`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HazardKind {
    /// A Windows device name (`CON`, `NUL`, `COM1`, ...), with or without an extension: opening it talks to
    /// the device, not a file.
    DeviceName(String),
    /// A component ends in `.` or a space: Windows silently strips it, so it names a different file.
    TrailingDotOrSpace,
    /// A character Windows does not allow in names; `:` also means "alternate data stream" or a drive prefix.
    InvalidChar(char),
}

/// A path that cannot be safely written on a [`Target`], and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hazard {
    /// The repository path.
    pub path: String,
    /// The offending component.
    pub component: String,
    /// What is wrong with it.
    pub kind: HazardKind,
}

impl fmt::Display for Hazard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            HazardKind::DeviceName(d) => {
                write!(f, "'{}': '{}' is the Windows device name {d}, not a file name", self.path, self.component)
            }
            HazardKind::TrailingDotOrSpace => {
                write!(f, "'{}': Windows drops the trailing dot or space of '{}'", self.path, self.component)
            }
            HazardKind::InvalidChar(':') => write!(
                f,
                "'{}': ':' in '{}' means an alternate data stream or a drive on Windows",
                self.path, self.component
            ),
            HazardKind::InvalidChar(c) => {
                write!(f, "'{}': '{}' contains {c:?}, which Windows does not allow in names", self.path, self.component)
            }
        }
    }
}

const DEVICES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

/// The Windows device a component names, if any. The device name counts with any extension (`nul.txt`,
/// `con.tar.gz`) and with trailing spaces before the extension (`aux .c`), case-insensitively.
fn windows_device(comp: &str) -> Option<String> {
    let base = comp.split('.').next().unwrap_or(comp).trim_end_matches(' ');
    let up = base.to_uppercase();
    if DEVICES.contains(&up.as_str()) {
        return Some(up);
    }
    // COM0-COM9 / LPT0-LPT9, plus the superscript digits ¹ ² ³ that Windows also reserves.
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = up.strip_prefix(prefix) {
            let mut it = rest.chars();
            if let (Some(d), None) = (it.next(), it.next())
                && (d.is_ascii_digit() || matches!(d, '\u{b9}' | '\u{b2}' | '\u{b3}'))
            {
                return Some(up);
            }
        }
    }
    None
}

fn component_hazard(comp: &str, target: &Target) -> Option<HazardKind> {
    if !target.windows_names {
        return None;
    }
    if let Some(c) = comp.chars().find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')) {
        return Some(HazardKind::InvalidChar(c));
    }
    if comp.ends_with('.') || comp.ends_with(' ') {
        return Some(HazardKind::TrailingDotOrSpace);
    }
    windows_device(comp).map(HazardKind::DeviceName)
}

/// The first reason `path` cannot be written on `target`, if any.
pub fn path_hazard(path: &RepoPath, target: &Target) -> Option<Hazard> {
    path.components().find_map(|comp| {
        component_hazard(comp, target).map(|kind| Hazard {
            path: path.as_str().to_string(),
            component: comp.to_string(),
            kind,
        })
    })
}

/// Names that a case-insensitive `target` cannot keep apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collision {
    /// The spellings (paths or directory prefixes) that are one name on the target, sorted.
    pub spellings: Vec<String>,
}

impl fmt::Display for Collision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} are the same name on a case-insensitive file system", self.spellings.join(" and "))
    }
}

/// One-to-one Unicode case mapping, the way NTFS's upcase table works: a character whose uppercase is a single
/// character maps to that; the result is lowercased the same way. Multi-character mappings (`ß` → `SS`) are
/// left alone.
fn simple_fold(c: char) -> char {
    fn single<I: Iterator<Item = char>>(mut it: I, fallback: char) -> char {
        match (it.next(), it.next()) {
            (Some(x), None) => x,
            _ => fallback,
        }
    }
    let up = single(c.to_uppercase(), c);
    single(up.to_lowercase(), up)
}

fn fold_key(comp: &str, target: &Target) -> String {
    let s: String = if target.case_insensitive { comp.chars().map(simple_fold).collect() } else { comp.to_string() };
    if target.normalization_insensitive { s.nfc().collect() } else { s }
}

/// Finds paths (and directories on the way to them) that `target` would merge into one name, e.g. `README`
/// and `readme`, or `Src/a` and `src/b`. Empty for a case-sensitive target. Deterministic: the result is
/// sorted and does not depend on the iteration order of `paths`.
pub fn find_collisions<'a>(paths: impl IntoIterator<Item = &'a RepoPath>, target: &Target) -> Vec<Collision> {
    if !target.case_insensitive && !target.normalization_insensitive {
        return Vec::new();
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
    for path in paths {
        let s = path.as_str();
        let ends = s.match_indices('/').map(|(i, _)| i).chain(std::iter::once(s.len()));
        for end in ends {
            let prefix = &s[..end];
            if !seen.insert(prefix) {
                continue;
            }
            let key = prefix.split('/').map(|c| fold_key(c, target)).collect::<Vec<_>>().join("/");
            by_key.entry(key).or_default().push(prefix.to_string());
        }
    }
    let sorted: BTreeMap<String, Vec<String>> = by_key.into_iter().filter(|(_, v)| v.len() > 1).collect();
    let all: Vec<Vec<String>> = sorted
        .into_values()
        .map(|mut spellings| {
            spellings.sort();
            spellings
        })
        .collect();
    // `Src/a` + `src/a` collide because `Src` and `src` do: report the directories once, not every file below.
    let collided: HashSet<&str> = all.iter().flatten().map(String::as_str).collect();
    let below_a_collision = |s: &str| s.match_indices('/').any(|(i, _)| collided.contains(&s[..i]));
    let kept: Vec<Collision> = all
        .iter()
        .filter(|spellings| !spellings.iter().all(|s| below_a_collision(s)))
        .map(|spellings| Collision { spellings: spellings.clone() })
        .collect();
    kept
}

/// Everything about a set of paths that a [`Target`] cannot hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Findings {
    /// Paths with a component the target cannot represent, sorted by path.
    pub hazards: Vec<Hazard>,
    /// Names the target would merge.
    pub collisions: Vec<Collision>,
}

impl Findings {
    /// Nothing the target cannot hold.
    pub fn is_clean(&self) -> bool {
        self.hazards.is_empty() && self.collisions.is_empty()
    }

    /// Number of problems.
    pub fn len(&self) -> usize {
        self.hazards.len() + self.collisions.len()
    }

    /// Same as `is_clean`, for clippy.
    pub fn is_empty(&self) -> bool {
        self.is_clean()
    }

    /// One line per problem, at most `limit` of them, then a count of the rest.
    pub fn describe(&self, limit: usize) -> Vec<String> {
        let all: Vec<String> = self
            .hazards
            .iter()
            .map(ToString::to_string)
            .chain(self.collisions.iter().map(ToString::to_string))
            .collect();
        let mut lines: Vec<String> = all.iter().take(limit).cloned().collect();
        if all.len() > limit {
            lines.push(format!("... and {} more", all.len() - limit));
        }
        lines
    }
}

/// Inspects `paths` for everything `target` cannot hold.
pub fn inspect<'a>(paths: impl IntoIterator<Item = &'a RepoPath>, target: &Target) -> Findings {
    let paths: Vec<&RepoPath> = paths.into_iter().collect();
    let mut hazards: Vec<Hazard> = paths.iter().filter_map(|p| path_hazard(p, target)).collect();
    hazards.sort_by(|a, b| a.path.cmp(&b.path));
    // A path that is a hazard anyway is not also reported as a collision.
    let rest = paths.iter().copied().filter(|p| path_hazard(p, target).is_none());
    Findings { hazards, collisions: find_collisions(rest, target) }
}

#[cfg(test)]
mod tests;
