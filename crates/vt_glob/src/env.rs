//! Glob matching for environment-variable **names** (flat strings, never paths).
//!
//! Backed by `globset` with path-separator handling disabled, so `*`, `?`,
//! `[...]`, and `{a,b}` behave as plain-string wildcards. Matching is
//! case-sensitive on Unix and case-insensitive on Windows, mirroring how
//! environment variables are looked up on each platform.
//!
//! [`EnvGlobSet`] supports negation: a `!`-prefixed pattern *excludes* names,
//! and a name matches the set when it matches an include pattern and no exclude
//! pattern. [`EnvGlob`] matches a single pattern literally — `!` is an ordinary
//! character there (no negation), since a lone exclude has nothing to subtract
//! from.

use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};

/// Error compiling an environment-variable name pattern.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct EnvGlobError(#[from] globset::Error);

/// Compiles `pattern` into a `globset::Glob` configured for env-name matching:
/// separators are not special, and case follows the platform's env semantics.
fn build(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern)
        // Env names contain no path separators, so disabling separator handling
        // makes `*`/`?` match any character — a pure string match.
        .literal_separator(false)
        // Env lookups are case-insensitive on Windows, case-sensitive elsewhere.
        .case_insensitive(cfg!(windows))
        .build()
}

/// Matches a single environment-variable name against one glob pattern.
#[derive(Debug, Clone)]
pub struct EnvGlob {
    matcher: GlobMatcher,
}

impl EnvGlob {
    /// Compiles `pattern` into an env-name matcher.
    ///
    /// # Errors
    /// Returns an error if `pattern` is not a valid glob.
    pub fn new(pattern: &str) -> Result<Self, EnvGlobError> {
        Ok(Self { matcher: build(pattern)?.compile_matcher() })
    }

    /// Returns whether `name` matches the pattern.
    #[must_use]
    pub fn is_match(&self, name: &str) -> bool {
        self.matcher.is_match(name)
    }
}

/// Matches an environment-variable name against a **set** of glob patterns,
/// with negation.
///
/// Patterns are split into includes and excludes: a `!`-prefixed pattern is an
/// **exclude**, any other pattern is an **include**.
///
/// A name matches when it matches some include pattern and no exclude pattern.
/// A set with no include patterns matches nothing (an exclude has nothing to
/// subtract from), so an empty set — or a set of only excludes — never matches.
#[derive(Debug, Clone)]
pub struct EnvGlobSet {
    include: GlobSet,
    exclude: GlobSet,
}

impl EnvGlobSet {
    /// Compiles `patterns` into a combined env-name matcher.
    ///
    /// # Errors
    /// Returns an error if any pattern is not a valid glob.
    pub fn new<I, S>(patterns: I) -> Result<Self, EnvGlobError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut include = GlobSetBuilder::new();
        let mut exclude = GlobSetBuilder::new();
        for pattern in patterns {
            let pattern = pattern.as_ref();
            if let Some(rest) = pattern.strip_prefix('!') {
                exclude.add(build(rest)?);
            } else {
                include.add(build(pattern)?);
            }
        }
        Ok(Self { include: include.build()?, exclude: exclude.build()? })
    }

    /// Returns whether `name` matches an include pattern and no exclude pattern.
    #[must_use]
    pub fn is_match(&self, name: &str) -> bool {
        self.include.is_match(name) && !self.exclude.is_match(name)
    }
}

#[cfg(test)]
mod tests {
    use super::{EnvGlob, EnvGlobSet};

    #[test]
    fn matches_star_prefix_and_suffix() {
        let g = EnvGlob::new("VITE_*").unwrap();
        assert!(g.is_match("VITE_FOO"));
        assert!(g.is_match("VITE_")); // `*` matches the empty string
        assert!(!g.is_match("MYVITE_FOO"));

        let g = EnvGlob::new("*_KEY").unwrap();
        assert!(g.is_match("MY_KEY"));
        assert!(!g.is_match("MY_KEYS"));

        let g = EnvGlob::new("*_CREDENTIAL*").unwrap();
        assert!(g.is_match("AWS_CREDENTIALS"));
        assert!(g.is_match("X_CREDENTIAL_Y"));
    }

    #[test]
    fn question_mark_matches_exactly_one_char() {
        let g = EnvGlob::new("APP?_*").unwrap();
        assert!(g.is_match("APP1_TOKEN"));
        assert!(g.is_match("APP2_NAME"));
        // `?` requires exactly one character, so `APP_X` (nothing before `_`) does not match.
        assert!(!g.is_match("APP_X"));
    }

    #[test]
    fn brace_alternation_is_supported() {
        let g = EnvGlob::new("{VITE,NEXT}_*").unwrap();
        assert!(g.is_match("VITE_FOO"));
        assert!(g.is_match("NEXT_BAR"));
        assert!(!g.is_match("NUXT_BAR"));
    }

    #[test]
    fn dot_and_separators_are_literal_not_path_special() {
        // Env names are flat strings: `*` spans `.` and `/` (no path semantics),
        // and a literal `.` in the pattern matches a literal `.`.
        assert!(EnvGlob::new("A*").unwrap().is_match("A.B"));
        assert!(EnvGlob::new("A*").unwrap().is_match("A/B"));
        assert!(EnvGlob::new("*.local").unwrap().is_match("APP.local"));
        assert!(!EnvGlob::new("*.local").unwrap().is_match("APPXlocal"));
    }

    #[test]
    fn single_glob_bang_is_a_literal_character() {
        // A single `EnvGlob` has no negation: `!FOO` matches the literal name
        // `!FOO`, not `FOO`.
        let g = EnvGlob::new("!FOO").unwrap();
        assert!(g.is_match("!FOO"));
        assert!(!g.is_match("FOO"));
    }

    #[test]
    fn non_match_default_is_false() {
        assert!(!EnvGlob::new("VITE_*").unwrap().is_match("PATH"));
    }

    #[test]
    fn set_matches_any_pattern() {
        let set = EnvGlobSet::new(["VITE_*", "*_KEY", "APP?_*"]).unwrap();
        assert!(set.is_match("VITE_FOO"));
        assert!(set.is_match("MY_KEY"));
        assert!(set.is_match("APP1_TOKEN"));
        assert!(!set.is_match("PATH"));
        assert!(!set.is_match("APP_X"));
    }

    #[test]
    fn empty_set_matches_nothing() {
        let set = EnvGlobSet::new(std::iter::empty::<&str>()).unwrap();
        assert!(!set.is_match("VITE_FOO"));
    }

    #[test]
    fn set_negation_excludes_matching_names() {
        // `!VITE_SECRET` excludes that name from the `VITE_*` include set.
        let set = EnvGlobSet::new(["VITE_*", "!VITE_SECRET"]).unwrap();
        assert!(set.is_match("VITE_FOO"));
        assert!(set.is_match("VITE_BAR"));
        assert!(!set.is_match("VITE_SECRET"));
        assert!(!set.is_match("PATH"));

        // An exclude glob can itself be a wildcard.
        let set = EnvGlobSet::new(["*", "!*_SECRET"]).unwrap();
        assert!(set.is_match("VITE_FOO"));
        assert!(!set.is_match("API_SECRET"));
    }

    #[test]
    fn set_only_excludes_matches_nothing() {
        // With no include patterns there is nothing to subtract from.
        let set = EnvGlobSet::new(["!FOO"]).unwrap();
        assert!(!set.is_match("FOO"));
        assert!(!set.is_match("BAR"));
    }

    #[test]
    #[cfg(not(windows))]
    fn unix_matching_is_case_sensitive() {
        let g = EnvGlob::new("VITE_*").unwrap();
        assert!(g.is_match("VITE_FOO"));
        assert!(!g.is_match("vite_foo"));
        let set = EnvGlobSet::new(["VITE_*"]).unwrap();
        assert!(!set.is_match("vite_foo"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_matching_is_case_insensitive() {
        let g = EnvGlob::new("VITE_*").unwrap();
        assert!(g.is_match("VITE_FOO"));
        assert!(g.is_match("vite_foo"));
        let set = EnvGlobSet::new(["VITE_*"]).unwrap();
        assert!(set.is_match("vite_foo"));
    }
}
