use std::sync::Arc;

use vite_path::{AbsolutePath, AbsolutePathBuf};
use wax::Glob;

use crate::Error;

/// A glob pattern anchored to an absolute directory.
///
/// Created by partitioning a glob into an invariant prefix and a variant (dynamic)
/// part, then resolving the prefix against a base directory. The prefix is cleaned
/// to normalize `..` components.
///
/// For example, `../shared/dist/**` relative to `/ws/packages/app` produces:
/// - `prefix`: `/ws/packages/shared/dist`
/// - `variant`: `Some(Glob("**"))`
#[derive(Debug)]
pub struct AnchoredGlob {
    prefix: Arc<AbsolutePath>,
    variant: Option<Glob<'static>>,
}

impl AnchoredGlob {
    /// Create an `AnchoredGlob` by resolving `pattern` relative to `base_dir`.
    ///
    /// The pattern is partitioned into an invariant prefix and a variant glob.
    /// The prefix is joined with `base_dir` and cleaned (normalizing `..`).
    ///
    /// # Errors
    ///
    /// Returns an error if the glob pattern is invalid.
    ///
    /// # Panics
    ///
    /// Panics if cleaning an absolute path somehow produces a non-absolute path.
    pub fn new(pattern: &str, base_dir: &AbsolutePath) -> Result<Self, Error> {
        use path_clean::PathClean as _;

        let glob = Glob::new(pattern)?;
        let (prefix_path, variant) = glob.partition();
        let cleaned = base_dir.as_path().join(&prefix_path).clean();
        // Cleaning an absolute path always produces an absolute path
        let prefix = Arc::<AbsolutePath>::from(
            vite_path::AbsolutePathBuf::new(cleaned)
                .expect("cleaning an absolute path produces an absolute path"),
        );
        Ok(Self { prefix, variant: variant.map(Glob::into_owned) })
    }

    /// The invariant prefix directory of this glob.
    #[must_use]
    pub(crate) fn prefix(&self) -> &AbsolutePath {
        &self.prefix
    }

    /// The variant (dynamic) portion of this glob, if any.
    #[must_use]
    pub(crate) const fn variant(&self) -> Option<&Glob<'static>> {
        self.variant.as_ref()
    }

    /// Whether this glob's prefix is an ancestor or descendant of `other`,
    /// meaning a rerooting between them is possible.
    #[must_use]
    pub(crate) fn has_related_prefix(&self, other: &AbsolutePath) -> bool {
        self.prefix.as_path().starts_with(other.as_path())
            || other.as_path().starts_with(self.prefix.as_path())
    }

    /// Reroot this glob relative to `new_root`, returning a wax `Glob` whose
    /// invariant prefix bridges from `new_root` to this glob's prefix.
    ///
    /// Returns `None` if this glob's prefix is not a descendant of `new_root`
    /// (unrelated prefixes), or if the glob is variant-less and sits exactly
    /// at `new_root` (cannot exclude/include files from the root itself).
    ///
    /// # Errors
    ///
    /// Returns an error if the rerooted glob pattern is invalid.
    pub(crate) fn reroot(&self, new_root: &AbsolutePath) -> Result<Option<Glob<'static>>, Error> {
        let Some(bridge) = path_bridge(new_root, &self.prefix) else {
            return Ok(None);
        };
        match &self.variant {
            Some(variant) => {
                let pattern = rerooted_pattern(&bridge, variant);
                Ok(Some(Glob::new(&pattern)?.into_owned()))
            }
            None if !bridge.is_empty() => Ok(Some(Glob::new(&wax::escape(&bridge))?.into_owned())),
            None => Ok(None),
        }
    }

    /// Check if an absolute path matches this anchored glob.
    #[must_use]
    pub fn is_match(&self, path: &AbsolutePath) -> bool {
        use wax::Program as _;
        let Ok(remainder) = path.as_path().strip_prefix(self.prefix.as_path()) else {
            return false;
        };
        let Some(v) = &self.variant else {
            return remainder.as_os_str().is_empty();
        };
        v.is_match(remainder)
    }
}

/// Compute the longest common ancestor of two absolute paths.
#[expect(
    clippy::disallowed_types,
    reason = "collecting std::path::Components requires std::path::PathBuf"
)]
pub fn common_ancestor(a: &AbsolutePath, b: &AbsolutePath) -> AbsolutePathBuf {
    let common: std::path::PathBuf = a
        .as_path()
        .components()
        .zip(b.as_path().components())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a)
        .collect();
    AbsolutePathBuf::new(common).expect("common ancestor of absolute paths is absolute")
}

/// Compute the "bridge" — the relative path from `ancestor` to `path` — as a
/// `/`-separated string. Returns `None` if `path` is not under `ancestor`
/// (i.e. the prefixes are unrelated and no rerooting is possible).
#[expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "bridge computation requires std String and str::replace for wax glob patterns"
)]
fn path_bridge(ancestor: &AbsolutePath, path: &AbsolutePath) -> Option<String> {
    let remainder = path.as_path().strip_prefix(ancestor.as_path()).ok()?;
    Some(remainder.to_string_lossy().replace('\\', "/"))
}

/// Build a rerooted glob pattern by joining an escaped bridge path with a
/// variant glob. When the bridge is empty (prefix == walk root), the variant
/// is returned unchanged.
#[expect(clippy::disallowed_types, reason = "building glob pattern string for wax requires String")]
fn rerooted_pattern(bridge: &str, variant: &Glob<'_>) -> String {
    if bridge.is_empty() {
        variant.to_string()
    } else {
        [&*wax::escape(bridge), "/", &variant.to_string()].concat()
    }
}

/// Escape wax glob metacharacters in a literal path string. The bridge is
/// always a literal path (derived from invariant prefixes), but it may
/// contain characters that wax interprets as glob syntax.
fn escape_glob(s: &str) -> Cow<'_, str> {
    const GLOB_CHARS: &[char] = &['?', '*', '$', ':', '<', '>', '(', ')', '[', ']', '{', '}', ','];
    if !s.contains(GLOB_CHARS) {
        return Cow::Borrowed(s);
    }
    let mut escaped = s.to_owned();
    escaped.clear();
    escaped.reserve(s.len() + 4);
    for c in s.chars() {
        if GLOB_CHARS.contains(&c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> AbsolutePathBuf {
        AbsolutePathBuf::new(std::path::PathBuf::from(p)).expect("test path should be absolute")
    }

    #[test]
    fn common_ancestor_same_path() {
        let a = abs("/app/src");
        let result = common_ancestor(&a, &a);
        assert_eq!(result, a);
    }

    #[test]
    fn common_ancestor_parent_child() {
        let parent = abs("/app");
        let child = abs("/app/src/lib");
        assert_eq!(common_ancestor(&parent, &child), parent);
        assert_eq!(common_ancestor(&child, &parent), parent);
    }

    #[test]
    fn common_ancestor_siblings() {
        let a = abs("/app/src");
        let b = abs("/app/dist");
        assert_eq!(common_ancestor(&a, &b), abs("/app"));
    }

    #[test]
    fn common_ancestor_only_root() {
        let a = abs("/foo/bar");
        let b = abs("/baz/qux");
        assert_eq!(common_ancestor(&a, &b), abs("/"));
    }

    #[test]
    fn reroot_same_prefix() {
        let root = abs("/app/src");
        let glob = AnchoredGlob::new("**/*.rs", &root).unwrap();
        let rerooted = glob.reroot(&root).unwrap().unwrap();
        assert_eq!(rerooted.to_string(), "**/*.rs");
    }

    #[test]
    fn reroot_descendant_prefix() {
        let base = abs("/app/src");
        let glob = AnchoredGlob::new("lib/**/*.rs", &base).unwrap();
        // prefix = /app/src/lib, variant = **/*.rs
        // reroot to /app → bridge = "src/lib"
        let root = abs("/app");
        let rerooted = glob.reroot(&root).unwrap().unwrap();
        assert_eq!(rerooted.to_string(), "src/lib/**/*.rs");
    }

    #[test]
    fn reroot_unrelated_returns_none() {
        let base = abs("/app/src");
        let glob = AnchoredGlob::new("**/*.rs", &base).unwrap();
        let root = abs("/other");
        assert!(glob.reroot(&root).unwrap().is_none());
    }

    #[test]
    fn reroot_variantless_with_bridge() {
        let base = abs("/app");
        let glob = AnchoredGlob::new("src/main.rs", &base).unwrap();
        // prefix = /app/src/main.rs, variant = None
        let root = abs("/app");
        let rerooted = glob.reroot(&root).unwrap().unwrap();
        assert_eq!(rerooted.to_string(), "src/main.rs");
    }

    #[test]
    fn reroot_variantless_at_root_returns_none() {
        let base = abs("/app");
        // A pattern that is just the base dir itself (no variant, no bridge)
        let glob = AnchoredGlob::new(".", &base).unwrap();
        let result = glob.reroot(&base).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn has_related_prefix_ancestor() {
        let glob = AnchoredGlob::new("**", &abs("/app/src")).unwrap();
        assert!(glob.has_related_prefix(&abs("/app")));
    }

    #[test]
    fn has_related_prefix_descendant() {
        let glob = AnchoredGlob::new("**", &abs("/app")).unwrap();
        assert!(glob.has_related_prefix(&abs("/app/src")));
    }

    #[test]
    fn has_related_prefix_same() {
        let glob = AnchoredGlob::new("**", &abs("/app")).unwrap();
        assert!(glob.has_related_prefix(&abs("/app")));
    }

    #[test]
    fn has_related_prefix_unrelated() {
        let glob = AnchoredGlob::new("**", &abs("/app")).unwrap();
        assert!(!glob.has_related_prefix(&abs("/other")));
    }

    #[test]
    fn escape_glob_no_metacharacters() {
        assert_eq!(escape_glob("src/lib"), "src/lib");
    }

    #[test]
    fn escape_glob_with_metacharacters() {
        assert_eq!(escape_glob("a[b]*c?d"), "a\\[b\\]\\*c\\?d");
    }

    #[test]
    fn path_bridge_direct_child() {
        let ancestor = abs("/app");
        let path = abs("/app/src");
        assert_eq!(path_bridge(&ancestor, &path).unwrap(), "src");
    }

    #[test]
    fn path_bridge_same_path() {
        let path = abs("/app");
        assert_eq!(path_bridge(&path, &path).unwrap(), "");
    }

    #[test]
    fn path_bridge_unrelated() {
        let a = abs("/app");
        let b = abs("/other");
        assert!(path_bridge(&a, &b).is_none());
    }

    #[test]
    fn rerooted_pattern_empty_bridge() {
        let glob = Glob::new("**/*.rs").unwrap();
        assert_eq!(rerooted_pattern("", &glob), "**/*.rs");
    }

    #[test]
    fn rerooted_pattern_with_bridge() {
        let glob = Glob::new("**/*.rs").unwrap();
        assert_eq!(rerooted_pattern("src/lib", &glob), "src/lib/**/*.rs");
    }
}
