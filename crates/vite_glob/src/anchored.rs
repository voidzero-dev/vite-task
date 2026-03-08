use std::sync::Arc;

use vite_path::AbsolutePath;
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
