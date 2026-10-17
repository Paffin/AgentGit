use ag_types::Identity;
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// Matches repository paths against glob patterns. `*` does not cross `/`;
/// `**` does.
#[derive(Clone, Debug)]
pub struct PathMatcher {
    set: GlobSet,
    patterns: Vec<String>,
}

impl PathMatcher {
    /// Compiles the patterns.
    pub fn new(patterns: &[String]) -> Result<Self, String> {
        let mut b = GlobSetBuilder::new();
        for p in patterns {
            let g = GlobBuilder::new(p)
                .literal_separator(true)
                .build()
                .map_err(|e| format!("bad glob '{p}': {e}"))?;
            b.add(g);
        }
        Ok(Self { set: b.build().map_err(|e| e.to_string())?, patterns: patterns.to_vec() })
    }

    /// True if any pattern matches `path`.
    pub fn is_match(&self, path: &str) -> bool {
        self.set.is_match(path)
    }

    /// True if there are no patterns.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Matches identities against patterns such as `user:*` or `agent:docs-*`.
/// A pattern matches an identity if it matches the full string or its
/// principal (the part before `#run`).
#[derive(Clone, Debug)]
pub struct IdentityMatcher {
    set: GlobSet,
    empty: bool,
}

impl IdentityMatcher {
    /// Compiles the patterns.
    pub fn new(patterns: &[String]) -> Result<Self, String> {
        let mut b = GlobSetBuilder::new();
        for p in patterns {
            b.add(Glob::new(p).map_err(|e| format!("bad identity pattern '{p}': {e}"))?);
        }
        Ok(Self { set: b.build().map_err(|e| e.to_string())?, empty: patterns.is_empty() })
    }

    /// True if no patterns were configured.
    pub fn is_empty(&self) -> bool {
        self.empty
    }

    /// True if any pattern matches.
    pub fn is_match(&self, id: &Identity) -> bool {
        self.set.is_match(id.as_str()) || self.set.is_match(id.principal().as_str())
    }
}
