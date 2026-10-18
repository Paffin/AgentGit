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
            let g = GlobBuilder::new(p).literal_separator(true).build().map_err(|e| format!("bad glob '{p}': {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_pattern_names_either_the_whole_identity_or_its_principal() {
        let run = Identity::new("agent:bot#run_1").unwrap();
        // the principal only: "agent:bot" names every run of the bot
        assert!(IdentityMatcher::new(&patterns(&["agent:bot"])).unwrap().is_match(&run));
        // the whole identity only: one run
        assert!(IdentityMatcher::new(&patterns(&["agent:bot#run_1"])).unwrap().is_match(&run));
        // neither
        assert!(!IdentityMatcher::new(&patterns(&["agent:bot#run_2", "agent:other"])).unwrap().is_match(&run));
        // and a plain user, which has no run part
        let user = Identity::new("user:alice").unwrap();
        assert!(IdentityMatcher::new(&patterns(&["user:*"])).unwrap().is_match(&user));
        assert!(!IdentityMatcher::new(&patterns(&["agent:*"])).unwrap().is_match(&user));
    }

    #[test]
    fn matchers_know_whether_they_have_patterns() {
        assert!(IdentityMatcher::new(&[]).unwrap().is_empty());
        assert!(!IdentityMatcher::new(&patterns(&["user:*"])).unwrap().is_empty());
        assert!(PathMatcher::new(&[]).unwrap().is_empty());
        assert!(!PathMatcher::new(&patterns(&["docs/**"])).unwrap().is_empty());
        // an empty matcher matches nothing, which is not the same as matching everything
        assert!(!PathMatcher::new(&[]).unwrap().is_match("docs/a.md"));
    }

    #[test]
    fn a_star_stays_inside_one_directory_and_two_stars_do_not() {
        let m = PathMatcher::new(&patterns(&["docs/*.md", "src/**"])).unwrap();
        assert!(m.is_match("docs/a.md"));
        assert!(!m.is_match("docs/deep/a.md"));
        assert!(m.is_match("src/a/b/c.rs"));
        assert!(!m.is_match("other/a.md"));
    }
}
