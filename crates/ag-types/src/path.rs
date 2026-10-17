use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum length of a whole path in bytes.
pub const MAX_PATH_LEN: usize = 4096;
/// Maximum length of one path component in bytes.
pub const MAX_COMPONENT_LEN: usize = 255;

/// Reasons a string is not a valid [`RepoPath`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The path is empty.
    #[error("path is empty")]
    Empty,
    /// The whole path exceeds [`MAX_PATH_LEN`].
    #[error("path is longer than {MAX_PATH_LEN} bytes")]
    TooLong,
    /// A component exceeds [`MAX_COMPONENT_LEN`].
    #[error("path component is longer than {MAX_COMPONENT_LEN} bytes")]
    ComponentTooLong,
    /// The path starts with `/`.
    #[error("path must be relative")]
    Absolute,
    /// The path ends with `/`.
    #[error("path must not end with '/'")]
    TrailingSlash,
    /// The path contains `//`.
    #[error("path contains an empty component")]
    EmptyComponent,
    /// The path contains a `.` or `..` component.
    #[error("path contains a '.' or '..' component")]
    DotComponent,
    /// The path contains a NUL, control character or backslash.
    #[error("path contains a forbidden character")]
    ForbiddenChar,
    /// The path contains a reserved component such as `.git` or `.ag`.
    #[error("path contains the reserved component '{0}'")]
    Reserved(String),
}

/// A validated, normalized, `/`-separated relative path inside a repository.
///
/// Validation rules (all enforced in [`RepoPath::new`]):
///
/// * valid UTF-8, non-empty, at most 4096 bytes, components at most 255 bytes;
/// * no leading or trailing `/`, no empty components;
/// * no `.` or `..` components (no traversal);
/// * no NUL, ASCII control characters or backslashes;
/// * no `.git` / `.ag` components under any spelling a file system folds into them: ASCII case-insensitive,
///   and the NTFS / HFS+ aliases (`.git.`, `.git `, `.git::$INDEX_ALLOCATION`, `git~1`, ignorable Unicode).
///   `.ag` is the AgentGit metadata directory and a planted `.git` is a well-known way to get code execution
///   from a checkout (see [`crate::portable::is_reserved_component`]).
///
/// Names that are a problem only on some file systems (`aux.txt`, `a:b`, `README` next to `readme`) are valid
/// here and are refused when a checkout is about to write them ([`crate::portable`]).
///
/// Ordering is bytewise on the string, which is the canonical order used
/// everywhere in the system.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoPath(String);

impl RepoPath {
    /// Validates and wraps a path.
    pub fn new(s: impl Into<String>) -> Result<Self, PathError> {
        let s: String = s.into();
        Self::validate(&s)?;
        Ok(Self(s))
    }

    fn validate(s: &str) -> Result<(), PathError> {
        if s.is_empty() {
            return Err(PathError::Empty);
        }
        if s.len() > MAX_PATH_LEN {
            return Err(PathError::TooLong);
        }
        if s.starts_with('/') {
            return Err(PathError::Absolute);
        }
        if s.ends_with('/') {
            return Err(PathError::TrailingSlash);
        }
        if s.chars().any(|c| c.is_control() || c == '\\') {
            return Err(PathError::ForbiddenChar);
        }
        for comp in s.split('/') {
            if comp.is_empty() {
                return Err(PathError::EmptyComponent);
            }
            if comp == "." || comp == ".." {
                return Err(PathError::DotComponent);
            }
            if comp.len() > MAX_COMPONENT_LEN {
                return Err(PathError::ComponentTooLong);
            }
            if crate::portable::is_reserved_component(comp) {
                return Err(PathError::Reserved(comp.to_string()));
            }
        }
        Ok(())
    }

    /// The path as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Iterator over the path components.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// The final component.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Every proper ancestor directory, shortest first (`a`, `a/b` for `a/b/c`).
    pub fn ancestors(&self) -> impl Iterator<Item = &str> {
        self.0.match_indices('/').map(|(i, _)| &self.0[..i])
    }

    /// True if `self` is a (non-strict) path prefix of `other` on component
    /// boundaries, i.e. `other` equals `self` or lies inside the directory `self`.
    pub fn contains(&self, other: &RepoPath) -> bool {
        other.0 == self.0
            || (other.0.len() > self.0.len()
                && other.0.starts_with(&self.0)
                && other.0.as_bytes()[self.0.len()] == b'/')
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RepoPath({:?})", self.0)
    }
}

impl AsRef<str> for RepoPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for RepoPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for RepoPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        RepoPath::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn accepts_normal_paths() {
        for p in ["a", "a/b", "src/main.rs", "dir with space/файл.txt", ".gitignore", "a.git/x"] {
            assert!(RepoPath::new(p).is_ok(), "{p}");
        }
    }

    #[test]
    fn rejects_dangerous_paths() {
        let cases = [
            ("", PathError::Empty),
            ("/etc/passwd", PathError::Absolute),
            ("a/", PathError::TrailingSlash),
            ("a//b", PathError::EmptyComponent),
            ("../x", PathError::DotComponent),
            ("a/./b", PathError::DotComponent),
            ("a/../b", PathError::DotComponent),
            ("a\\b", PathError::ForbiddenChar),
            ("a\0b", PathError::ForbiddenChar),
            ("a\nb", PathError::ForbiddenChar),
        ];
        for (p, e) in cases {
            assert_eq!(RepoPath::new(p), Err(e), "{p:?}");
        }
        for p in [".git/config", "a/.GIT/hooks", ".ag/db", "x/.Ag"] {
            assert!(matches!(RepoPath::new(p), Err(PathError::Reserved(_))), "{p}");
        }
    }

    #[test]
    fn rejects_file_system_aliases_of_the_reserved_directories() {
        for p in [
            ".git./config",
            ".git /config",
            "a/.git::$INDEX_ALLOCATION/hooks/post-checkout",
            "GIT~1/hooks/post-checkout",
            "x/git~1./config",
            ".g\u{200c}it/config",
            ".ag./db",
            "AG~1/x",
        ] {
            assert!(matches!(RepoPath::new(p), Err(PathError::Reserved(_))), "{p}");
        }
        for p in [".gitignore", ".gitattributes", "a/.git-notes/x", ".agignore", "git~1x/y", "x/.git.d"] {
            assert!(RepoPath::new(p).is_ok(), "{p}");
        }
    }

    #[test]
    fn length_limits() {
        assert_eq!(RepoPath::new("a".repeat(256)), Err(PathError::ComponentTooLong));
        assert!(RepoPath::new("a".repeat(255)).is_ok());
        let long = vec!["abcdefghij"; 500].join("/");
        assert_eq!(RepoPath::new(long), Err(PathError::TooLong));
    }

    #[test]
    fn ancestors_and_contains() {
        let p = RepoPath::new("a/b/c").unwrap();
        assert_eq!(p.ancestors().collect::<Vec<_>>(), vec!["a", "a/b"]);
        assert_eq!(p.file_name(), "c");
        let dir = RepoPath::new("a/b").unwrap();
        assert!(dir.contains(&p));
        assert!(!p.contains(&dir));
        assert!(!RepoPath::new("a/bb").unwrap().contains(&p));
        assert!(p.contains(&p));
    }

    proptest! {
        #[test]
        fn valid_paths_have_no_traversal(s in "\\PC{0,40}") {
            if let Ok(p) = RepoPath::new(s) {
                for c in p.components() {
                    prop_assert!(c != ".." && c != "." && !c.is_empty());
                }
                prop_assert!(!p.as_str().starts_with('/'));
            }
        }
    }
}
