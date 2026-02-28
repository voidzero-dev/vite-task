//! Package filter types and parsing for pnpm-style `--filter` selectors.
//!
//! # Design
//!
//! Package selection is deliberately separated from task matching (two-stage model).
//! This module handles only Stage 1: which packages to include/exclude.
//! Stage 2 (which tasks to run in those packages) lives in `vite_task_graph`.
//!
//! # Filter syntax
//!
//! Follows pnpm's `--filter` specification:
//!
//! - `foo`           → exact package name
//! - `@scope/*`      → glob pattern
//! - `./path`        → packages whose root is at or under this directory (one-way)
//! - `{./path}`      → same, brace syntax
//! - `name{./dir}`   → name AND directory (intersection)
//! - `foo...`        → foo + its transitive dependencies
//! - `...foo`        → foo + its transitive dependents
//! - `foo^...`       → foo's dependencies only (exclude foo itself)
//! - `...^foo`       → foo's dependents only (exclude foo itself)
//! - `...foo...`     → foo + dependencies + dependents
//! - `!foo`          → exclude foo from results
//!
//! The `ContainingPackage` selector variant is NOT produced by `parse_filter`.
//! It is synthesized internally for `vp run build` (implicit cwd) and `vp run -t build`
//! to walk up the directory tree and find the package that contains the given path.
//! This mirrors pnpm's `findPrefix` behaviour (not [`parsePackageSelector`] behaviour).
//!
//! [`parsePackageSelector`]: https://github.com/pnpm/pnpm/blob/05dd45ea82fff9c0b687cdc8f478a1027077d343/workspace/filter-workspace-packages/src/parsePackageSelector.ts#L14-L61

use std::sync::Arc;

use vec1::Vec1;
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_str::Str;

use crate::package_graph::PackageQuery;

// ────────────────────────────────────────────────────────────────────────────
// Types
// ────────────────────────────────────────────────────────────────────────────

/// Exact name or glob pattern for matching package names.
#[derive(Debug, Clone)]
pub(crate) enum PackageNamePattern {
    /// Exact name (e.g. `foo`, `@scope/pkg`). O(1) hash lookup.
    ///
    /// Scoped auto-completion applies during resolution: if `"bar"` has no exact match
    /// but exactly one `@*/bar` package exists, that package is matched.
    /// pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L303-L306>
    Exact(Str),

    /// Glob pattern (e.g. `@scope/*`, `*-utils`). Iterates all packages.
    ///
    /// Only `*` and `?` wildcards are supported (pnpm semantics).
    /// Stored as an owned `Glob<'static>` so the filter can outlive the input string.
    Glob(Box<wax::Glob<'static>>),
}

/// Directory matching pattern for `--filter` selectors.
///
/// Follows pnpm v7+ glob-dir semantics: plain paths are exact-only,
/// `*` / `**` opt in to descendant matching.
///
/// pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L200-L202>
#[derive(Debug, Clone)]
pub(crate) enum DirectoryPattern {
    /// Exact path match (no glob metacharacters in selector).
    Exact(Arc<AbsolutePath>),

    /// Glob: resolved base directory (non-glob prefix) + relative glob pattern.
    ///
    /// Matching strips `base` from a candidate path, then tests the remainder
    /// against `pattern`. For example, `./packages/*` with cwd `/ws` produces
    /// `base = /ws/packages`, `pattern = *`, which matches `/ws/packages/app`
    /// (remainder `app` matches `*`).
    Glob { base: Arc<AbsolutePath>, pattern: Box<wax::Glob<'static>> },
}

/// What packages to initially match.
///
/// The enum prevents the all-`None` invalid state that would arise from a struct
/// with all optional fields (as in pnpm's independent optional fields).
#[derive(Debug, Clone)]
pub(crate) enum PackageSelector {
    /// Match by name only. Produced by `--filter foo` or `--filter "@scope/*"`.
    Name(PackageNamePattern),

    /// Match by directory. Produced by `--filter .`, `--filter ./path`, `--filter {dir}`.
    ///
    /// Uses pnpm v7+ glob-dir semantics: plain paths are exact-match only,
    /// `*` / `**` globs opt in to descendant matching.
    Directory(DirectoryPattern),

    /// Find the package that **contains** this path (walks up the directory tree).
    ///
    /// Produced internally for `vp run build` (implicit cwd) and `vp run -t build`.
    /// Uses `IndexedPackageGraph::get_package_index_from_cwd` semantics.
    /// Never produced by `parse_filter`.
    ContainingPackage(Arc<AbsolutePath>),

    /// Match by name AND directory (intersection).
    /// Produced by `--filter "pattern{./dir}"`.
    NameAndDirectory { name: PackageNamePattern, directory: DirectoryPattern },
}

/// Direction to traverse the package dependency graph from the initially matched packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraversalDirection {
    /// Transitive dependencies (outgoing edges). Produced by `foo...`.
    Dependencies,

    /// Transitive dependents (incoming edges). Produced by `...foo`.
    Dependents,

    /// Both: walk dependents first, then walk all dependencies of every found dependent.
    /// Produced by `...foo...`.
    /// pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L265-L267>
    Both,
}

/// Graph traversal specification: how to expand from the initially matched packages.
///
/// Only present when `...` appears in the filter. The absence of this struct prevents
/// the invalid state of `exclude_self = true` without any expansion.
#[derive(Debug, Clone)]
pub(crate) struct GraphTraversal {
    pub(crate) direction: TraversalDirection,

    /// Exclude the initially matched packages from the result.
    ///
    /// Produced by `^` in `foo^...` (keep dependencies, drop foo)
    /// or `...^foo` (keep dependents, drop foo).
    pub(crate) exclude_self: bool,
}

/// A single package filter, corresponding to one `--filter` argument.
///
/// Multiple filters are composed at the `PackageQuery` level:
/// inclusions are unioned, then exclusions are subtracted.
#[derive(Debug, Clone)]
pub(crate) struct PackageFilter {
    /// When `true`, packages matching this filter are **excluded** from the result.
    /// Produced by a leading `!` in the filter string.
    pub(crate) exclude: bool,

    /// Which packages to initially match.
    pub(crate) selector: PackageSelector,

    /// Optional graph expansion from the initial match.
    /// `None` = exact match only (no traversal).
    pub(crate) traversal: Option<GraphTraversal>,
}

// ────────────────────────────────────────────────────────────────────────────
// Error
// ────────────────────────────────────────────────────────────────────────────

/// Errors that can occur when parsing a `--filter` string.
#[derive(Debug, thiserror::Error)]
pub enum PackageFilterParseError {
    #[error("Empty filter selector")]
    EmptySelector,

    #[error("Invalid glob pattern: {0}")]
    InvalidGlob(#[from] wax::BuildError),
}

// ────────────────────────────────────────────────────────────────────────────
// CLI package query
// ────────────────────────────────────────────────────────────────────────────

/// Errors that can occur when converting [`PackageQueryArgs`] into a [`PackageQuery`].
#[derive(Debug, thiserror::Error)]
pub enum PackageQueryError {
    #[error("--recursive and --transitive cannot be used together")]
    RecursiveTransitiveConflict,

    #[error("--filter and --transitive cannot be used together")]
    FilterWithTransitive,

    #[error("--filter and --recursive cannot be used together")]
    FilterWithRecursive,

    #[error("cannot specify package name with --recursive")]
    PackageNameWithRecursive { package_name: Str },

    #[error("cannot specify package name with --filter")]
    PackageNameWithFilter { package_name: Str },

    #[error("--filter value contains no selectors (whitespace-only)")]
    EmptyFilter,

    #[error("invalid --filter expression")]
    InvalidFilter(#[from] PackageFilterParseError),
}

/// CLI arguments for selecting which packages a command applies to.
///
/// Use `#[clap(flatten)]` to embed these in a parent clap struct.
/// Call [`into_package_query`](Self::into_package_query) to convert into an opaque [`PackageQuery`].
#[derive(Debug, Clone, clap::Args)]
pub struct PackageQueryArgs {
    /// Run tasks found in all packages in the workspace, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    pub recursive: bool,

    /// Run tasks found in the current package and all its transitive dependencies, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    pub transitive: bool,

    /// Filter packages (pnpm --filter syntax). Can be specified multiple times.
    #[clap(short = 'F', long, num_args = 1)]
    pub filter: Vec<Str>,
}

impl PackageQueryArgs {
    /// Convert CLI arguments into an opaque [`PackageQuery`].
    ///
    /// `package_name` is the optional package name from a `package#task` specifier.
    /// `cwd` is the working directory (used as fallback when no package name or filter is given).
    ///
    /// # Errors
    ///
    /// Returns [`PackageQueryError`] if conflicting flags are set, a package name
    /// is specified with `--recursive` or `--filter`, or a filter expression is invalid.
    pub fn into_package_query(
        self,
        package_name: Option<Str>,
        cwd: &Arc<AbsolutePath>,
    ) -> Result<PackageQuery, PackageQueryError> {
        let Self { recursive, transitive, filter } = self;

        if recursive {
            if transitive {
                return Err(PackageQueryError::RecursiveTransitiveConflict);
            }
            if !filter.is_empty() {
                return Err(PackageQueryError::FilterWithRecursive);
            }
            if let Some(package_name) = package_name {
                return Err(PackageQueryError::PackageNameWithRecursive { package_name });
            }
            return Ok(PackageQuery::all());
        }

        if !filter.is_empty() {
            if transitive {
                return Err(PackageQueryError::FilterWithTransitive);
            }
            if let Some(package_name) = package_name {
                return Err(PackageQueryError::PackageNameWithFilter { package_name });
            }
            // Normalize: split each --filter value by whitespace into individual tokens.
            // This makes `--filter "a b"` equivalent to `--filter a --filter b` (pnpm behaviour).
            let tokens: Vec1<Str> = Vec1::try_from_vec(
                filter
                    .into_iter()
                    .flat_map(|f| f.split_ascii_whitespace().map(Str::from).collect::<Vec<_>>())
                    .collect(),
            )
            .map_err(|_| PackageQueryError::EmptyFilter)?;
            let parsed: Vec1<PackageFilter> = tokens.try_mapped(|f| parse_filter(&f, cwd))?;
            return Ok(PackageQuery::filters(parsed));
        }

        // No --filter, no --recursive: implicit cwd or package-name specifier.
        let selector = package_name.map_or_else(
            || PackageSelector::ContainingPackage(Arc::clone(cwd)),
            |name| PackageSelector::Name(PackageNamePattern::Exact(name)),
        );
        let traversal = if transitive {
            Some(GraphTraversal {
                direction: TraversalDirection::Dependencies,
                exclude_self: false,
            })
        } else {
            None
        };
        Ok(PackageQuery::filters(Vec1::new(PackageFilter { exclude: false, selector, traversal })))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Parsing
// ────────────────────────────────────────────────────────────────────────────

/// Parse a `--filter` string into a [`PackageFilter`].
///
/// `cwd` is used to resolve relative paths (`.`, `./path`, `{./path}`).
///
/// # Errors
///
/// Returns [`PackageFilterParseError::EmptySelector`] if the core selector is empty,
/// or [`PackageFilterParseError::InvalidGlob`] if the pattern contains an invalid glob.
///
/// # Syntax
///
/// Follows pnpm's [`parsePackageSelector`] algorithm. See module-level docs for examples.
///
/// [`parsePackageSelector`]: https://github.com/pnpm/pnpm/blob/05dd45ea82fff9c0b687cdc8f478a1027077d343/workspace/filter-workspace-packages/src/parsePackageSelector.ts#L14-L61
pub(crate) fn parse_filter(
    input: &str,
    cwd: &AbsolutePath,
) -> Result<PackageFilter, PackageFilterParseError> {
    // Step 1: strip leading `!` → exclusion filter.
    let (exclude, rest) =
        input.strip_prefix('!').map_or((false, input), |without_bang| (true, without_bang));

    // Step 2: strip trailing `...` → transitive dependencies.
    // Check for `^` immediately before `...` → exclude the seed packages themselves.
    let (include_dependencies, deps_exclude_self, rest) =
        rest.strip_suffix("...").map_or((false, false, rest), |before_dots| {
            before_dots
                .strip_suffix('^')
                .map_or((true, false, before_dots), |before_caret| (true, true, before_caret))
        });

    // Step 3: strip leading `...` → transitive dependents.
    // Check for `^` immediately after `...` → exclude the seed packages themselves.
    let (include_dependents, dependents_exclude_self, core) =
        rest.strip_prefix("...").map_or((false, false, rest), |after_dots| {
            after_dots
                .strip_prefix('^')
                .map_or((true, false, after_dots), |after_caret| (true, true, after_caret))
        });

    // exclude_self is true if either direction had `^`.
    let exclude_self = deps_exclude_self || dependents_exclude_self;

    // Step 4–5: build the traversal descriptor.
    let traversal = match (include_dependencies, include_dependents) {
        (false, false) => None,
        (true, false) => {
            Some(GraphTraversal { direction: TraversalDirection::Dependencies, exclude_self })
        }
        (false, true) => {
            Some(GraphTraversal { direction: TraversalDirection::Dependents, exclude_self })
        }
        (true, true) => Some(GraphTraversal { direction: TraversalDirection::Both, exclude_self }),
    };

    // Step 6–9: parse the remaining core selector.
    let selector = parse_core_selector(core, cwd)?;

    Ok(PackageFilter { exclude, selector, traversal })
}

/// Parse the core selector string (after stripping `!` and `...` markers).
///
/// Implements pnpm's [`SELECTOR_REGEX`] logic: `^([^.][^{}[\]]*)?(\{[^}]+\})?$`
///
/// [`SELECTOR_REGEX`]: https://github.com/pnpm/pnpm/blob/05dd45ea82fff9c0b687cdc8f478a1027077d343/workspace/filter-workspace-packages/src/parsePackageSelector.ts#L37
///
/// Decision tree:
/// 1. If the string ends with `}` and contains a `{`, split name and brace-directory.
///    The name part must not start with `.` for the brace split to be valid
///    (per the regex rule that Group 1 must not start with `.`).
/// 2. If the string starts with `.`, treat the whole thing as a relative path.
/// 3. Otherwise treat as a name pattern (exact or glob).
fn parse_core_selector(
    core: &str,
    cwd: &AbsolutePath,
) -> Result<PackageSelector, PackageFilterParseError> {
    // Try to extract a brace-enclosed directory suffix: `{...}`.
    // The name part before the brace must not start with `.` (pnpm regex Group 1 constraint).
    if let Some(without_closing) = core.strip_suffix('}')
        && let Some(brace_pos) = without_closing.rfind('{')
    {
        let name_part = &without_closing[..brace_pos];
        let dir_inner = &without_closing[brace_pos + 1..];

        // Per pnpm's regex: Group 1 (`[^.][^{}[\]]*`) must NOT start with `.`.
        // If name_part starts with `.`, fall through to the `.`-prefix check.
        if !name_part.starts_with('.') {
            let directory = resolve_directory_pattern(dir_inner, cwd)?;

            return if name_part.is_empty() {
                // Only a directory selector: `{./foo}` or `{packages/app}`.
                Ok(PackageSelector::Directory(directory))
            } else {
                // Name and directory combined: `foo{./bar}`.
                let name = build_name_pattern(name_part)?;
                Ok(PackageSelector::NameAndDirectory { name, directory })
            };
        }
        // name_part starts with `.`: fall through — treat entire core as a relative path.
    }

    // If the core starts with `.`, it's a relative path to a directory.
    // This handles `.`, `..`, `./foo`, `../foo`, `./foo/*`, `./foo/**`.
    if core.starts_with('.') {
        let directory = resolve_directory_pattern(core, cwd)?;
        return Ok(PackageSelector::Directory(directory));
    }

    // Guard against an empty selector reaching here.
    if core.is_empty() {
        return Err(PackageFilterParseError::EmptySelector);
    }

    // Plain name or glob pattern.
    Ok(PackageSelector::Name(build_name_pattern(core)?))
}

/// Resolve a directory selector string into a [`DirectoryPattern`].
///
/// Uses [`wax::Glob::partition`] to split into an invariant base path and an
/// optional glob pattern. If `partition` yields no pattern, the result is an
/// exact path match; otherwise it is a glob match.
fn resolve_directory_pattern(
    path_str: &str,
    cwd: &AbsolutePath,
) -> Result<DirectoryPattern, PackageFilterParseError> {
    let glob = wax::Glob::new(path_str)?.into_owned();
    let (base_pathbuf, pattern) = glob.partition();
    let base_str = base_pathbuf.to_str().expect("filter paths are always valid UTF-8");
    let base = resolve_filter_path(if base_str.is_empty() { "." } else { base_str }, cwd);

    match pattern {
        Some(pattern) => Ok(DirectoryPattern::Glob { base, pattern: Box::new(pattern) }),
        None => Ok(DirectoryPattern::Exact(base)),
    }
}

/// Resolve a path string relative to `cwd`, normalising away `.` and `..`.
///
/// `path_str` may be `"."`, `".."`, `"./foo"`, `"../foo"`, or a bare name like `"packages/app"`.
///
/// Uses lexical normalization (no filesystem access), which can produce incorrect
/// results when symlinks are involved (e.g. `/a/symlink/../b` → `/a/b`). This
/// matches pnpm's behaviour.
fn resolve_filter_path(path_str: &str, cwd: &AbsolutePath) -> Arc<AbsolutePath> {
    let cleaned = path_clean::clean(cwd.join(path_str).as_path());
    let normalized = AbsolutePathBuf::new(cleaned)
        .expect("invariant: cleaning an absolute path preserves absoluteness");
    normalized.into()
}

/// Build a [`PackageNamePattern`] from a name or glob string.
///
/// A string containing `*`, `?`, or `[` is treated as a glob; otherwise exact.
fn build_name_pattern(name: &str) -> Result<PackageNamePattern, PackageFilterParseError> {
    if name.contains(['*', '?', '[']) {
        // Validate and compile the glob, then make it owned (lifetime: 'static).
        let glob = wax::Glob::new(name)?.into_owned();
        Ok(PackageNamePattern::Glob(Box::new(glob)))
    } else {
        Ok(PackageNamePattern::Exact(name.into()))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct an [`AbsolutePath`] from a Unix-style literal (test helper).
    ///
    /// On Windows, a `C:` prefix is prepended so `/workspace` becomes `C:/workspace`.
    #[cfg_attr(
        windows,
        expect(
            clippy::disallowed_macros,
            reason = "test helper constructs Windows paths from Unix-style literals"
        )
    )]
    fn abs(path: &'static str) -> &'static AbsolutePath {
        #[cfg(unix)]
        {
            AbsolutePath::new(path).expect("test path must be absolute")
        }
        #[cfg(windows)]
        {
            let leaked = Box::leak(std::format!("C:{path}").into_boxed_str());
            AbsolutePath::new(leaked).expect("test path must be absolute")
        }
    }

    // ── Helpers to assert selector shapes ───────────────────────────────────

    fn assert_exact_name(filter: &PackageFilter, expected: &str) {
        match &filter.selector {
            PackageSelector::Name(PackageNamePattern::Exact(n)) => {
                assert_eq!(n.as_str(), expected, "exact name mismatch");
            }
            other => panic!("expected Name(Exact({expected:?})), got {other:?}"),
        }
    }

    fn assert_glob_name(filter: &PackageFilter, expected_pattern: &str) {
        match &filter.selector {
            PackageSelector::Name(PackageNamePattern::Glob(g)) => {
                assert_eq!(g.to_string(), expected_pattern, "glob pattern mismatch");
            }
            other => panic!("expected Name(Glob({expected_pattern:?})), got {other:?}"),
        }
    }

    fn assert_directory(filter: &PackageFilter, expected_path: &AbsolutePath) {
        match &filter.selector {
            PackageSelector::Directory(DirectoryPattern::Exact(dir)) => {
                assert_eq!(dir.as_ref(), expected_path, "directory mismatch");
            }
            other => panic!("expected Directory(Exact({expected_path:?})), got {other:?}"),
        }
    }

    fn assert_directory_glob(
        filter: &PackageFilter,
        expected_base: &AbsolutePath,
        expected_pattern: &str,
    ) {
        match &filter.selector {
            PackageSelector::Directory(DirectoryPattern::Glob { base, pattern }) => {
                assert_eq!(base.as_ref(), expected_base, "base mismatch");
                assert_eq!(pattern.to_string(), expected_pattern, "pattern mismatch");
            }
            other => panic!(
                "expected Directory(Glob {{ base: {expected_base:?}, pattern: {expected_pattern:?} }}), got {other:?}"
            ),
        }
    }

    fn assert_name_and_directory(
        filter: &PackageFilter,
        expected_name: &str,
        expected_dir: &AbsolutePath,
    ) {
        match &filter.selector {
            PackageSelector::NameAndDirectory {
                name: PackageNamePattern::Exact(n),
                directory: DirectoryPattern::Exact(dir),
            } => {
                assert_eq!(n.as_str(), expected_name, "name mismatch");
                assert_eq!(dir.as_ref(), expected_dir, "directory mismatch");
            }
            other => panic!(
                "expected NameAndDirectory(Exact({expected_name:?}), Exact({expected_dir:?})), got {other:?}"
            ),
        }
    }

    fn assert_name_and_directory_glob(
        filter: &PackageFilter,
        expected_name: &str,
        expected_base: &AbsolutePath,
        expected_pattern: &str,
    ) {
        match &filter.selector {
            PackageSelector::NameAndDirectory {
                name: PackageNamePattern::Exact(n),
                directory: DirectoryPattern::Glob { base, pattern },
            } => {
                assert_eq!(n.as_str(), expected_name, "name mismatch");
                assert_eq!(base.as_ref(), expected_base, "base mismatch");
                assert_eq!(pattern.to_string(), expected_pattern, "pattern mismatch");
            }
            other => panic!(
                "expected NameAndDirectory(Exact({expected_name:?}), Glob {{ base: {expected_base:?}, pattern: {expected_pattern:?} }}), got {other:?}"
            ),
        }
    }

    fn assert_no_traversal(filter: &PackageFilter) {
        assert!(filter.traversal.is_none(), "expected no traversal, got {:?}", filter.traversal);
    }

    fn assert_traversal(filter: &PackageFilter, direction: TraversalDirection, exclude_self: bool) {
        match &filter.traversal {
            Some(t) => {
                assert_eq!(t.direction, direction, "direction mismatch");
                assert_eq!(t.exclude_self, exclude_self, "exclude_self mismatch");
            }
            None => panic!("expected traversal {direction:?}/{exclude_self}, got None"),
        }
    }

    // ── Tests ported from pnpm parsePackageSelector.ts ──────────────────────

    #[test]
    fn exact_name() {
        let cwd = abs("/workspace");
        let f = parse_filter("foo", cwd).unwrap();
        assert!(!f.exclude);
        assert_exact_name(&f, "foo");
        assert_no_traversal(&f);
    }

    #[test]
    fn name_with_dependencies() {
        let cwd = abs("/workspace");
        let f = parse_filter("foo...", cwd).unwrap();
        assert!(!f.exclude);
        assert_exact_name(&f, "foo");
        assert_traversal(&f, TraversalDirection::Dependencies, false);
    }

    #[test]
    fn name_with_dependents() {
        let cwd = abs("/workspace");
        let f = parse_filter("...foo", cwd).unwrap();
        assert!(!f.exclude);
        assert_exact_name(&f, "foo");
        assert_traversal(&f, TraversalDirection::Dependents, false);
    }

    #[test]
    fn name_with_both_directions() {
        let cwd = abs("/workspace");
        let f = parse_filter("...foo...", cwd).unwrap();
        assert!(!f.exclude);
        assert_exact_name(&f, "foo");
        assert_traversal(&f, TraversalDirection::Both, false);
    }

    #[test]
    fn name_with_dependencies_exclude_self() {
        let cwd = abs("/workspace");
        let f = parse_filter("foo^...", cwd).unwrap();
        assert!(!f.exclude);
        assert_exact_name(&f, "foo");
        assert_traversal(&f, TraversalDirection::Dependencies, true);
    }

    #[test]
    fn name_with_dependents_exclude_self() {
        let cwd = abs("/workspace");
        let f = parse_filter("...^foo", cwd).unwrap();
        assert!(!f.exclude);
        assert_exact_name(&f, "foo");
        assert_traversal(&f, TraversalDirection::Dependents, true);
    }

    #[test]
    fn relative_path_dot_slash_foo() {
        let cwd = abs("/workspace");
        let f = parse_filter("./foo", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory(&f, abs("/workspace/foo"));
        assert_no_traversal(&f);
    }

    #[test]
    fn relative_path_dot() {
        let cwd = abs("/workspace/packages/app");
        let f = parse_filter(".", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory(&f, abs("/workspace/packages/app"));
        assert_no_traversal(&f);
    }

    #[test]
    fn relative_path_dotdot() {
        let cwd = abs("/workspace/packages/app");
        let f = parse_filter("..", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory(&f, abs("/workspace/packages"));
        assert_no_traversal(&f);
    }

    #[test]
    fn exclusion_prefix() {
        let cwd = abs("/workspace");
        let f = parse_filter("!foo", cwd).unwrap();
        assert!(f.exclude);
        assert_exact_name(&f, "foo");
        assert_no_traversal(&f);
    }

    #[test]
    fn brace_directory_relative_path() {
        let cwd = abs("/workspace");
        let f = parse_filter("{./foo}", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory(&f, abs("/workspace/foo"));
        assert_no_traversal(&f);
    }

    #[test]
    fn brace_directory_with_dependents() {
        let cwd = abs("/workspace");
        let f = parse_filter("...{./foo}", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory(&f, abs("/workspace/foo"));
        assert_traversal(&f, TraversalDirection::Dependents, false);
    }

    #[test]
    fn name_and_directory_combined() {
        let cwd = abs("/workspace");
        let f = parse_filter("pattern{./dir}", cwd).unwrap();
        assert!(!f.exclude);
        assert_name_and_directory(&f, "pattern", abs("/workspace/dir"));
        assert_no_traversal(&f);
    }

    #[test]
    fn glob_pattern() {
        let cwd = abs("/workspace");
        let f = parse_filter("@scope/*", cwd).unwrap();
        assert!(!f.exclude);
        assert_glob_name(&f, "@scope/*");
        assert_no_traversal(&f);
    }

    #[test]
    fn empty_selector_error() {
        let cwd = abs("/workspace");
        let err = parse_filter("", cwd).unwrap_err();
        assert!(matches!(err, PackageFilterParseError::EmptySelector));
    }

    /// A filter with only `!` (exclusion of empty selector) should also error.
    #[test]
    fn exclusion_with_empty_selector_error() {
        let cwd = abs("/workspace");
        let err = parse_filter("!", cwd).unwrap_err();
        assert!(matches!(err, PackageFilterParseError::EmptySelector));
    }

    #[test]
    fn scoped_package_name() {
        let cwd = abs("/workspace");
        let f = parse_filter("@test/app", cwd).unwrap();
        assert_exact_name(&f, "@test/app");
        assert_no_traversal(&f);
    }

    #[test]
    fn path_normalisation_dotdot_in_middle() {
        // `./foo/../bar` should normalise to `cwd/bar`
        let cwd = abs("/workspace");
        let f = parse_filter("{./foo/../bar}", cwd).unwrap();
        assert_directory(&f, abs("/workspace/bar"));
    }

    #[test]
    fn path_normalisation_trailing_dot() {
        // `./foo/.` should normalise to `cwd/foo`
        let cwd = abs("/workspace");
        let f = parse_filter("{./foo/.}", cwd).unwrap();
        assert_directory(&f, abs("/workspace/foo"));
    }

    // ── Directory glob tests ─────────────────────────────────────────────────

    #[test]
    fn directory_glob_star() {
        let cwd = abs("/workspace");
        let f = parse_filter("./packages/*", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory_glob(&f, abs("/workspace/packages"), "*");
        assert_no_traversal(&f);
    }

    #[test]
    fn directory_glob_double_star() {
        let cwd = abs("/workspace");
        let f = parse_filter("./packages/**", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory_glob(&f, abs("/workspace/packages"), "**");
        assert_no_traversal(&f);
    }

    #[test]
    fn brace_directory_glob() {
        let cwd = abs("/workspace");
        let f = parse_filter("{./packages/*}", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory_glob(&f, abs("/workspace/packages"), "*");
        assert_no_traversal(&f);
    }

    #[test]
    fn name_and_directory_glob_combined() {
        let cwd = abs("/workspace");
        let f = parse_filter("app{./packages/*}", cwd).unwrap();
        assert!(!f.exclude);
        assert_name_and_directory_glob(&f, "app", abs("/workspace/packages"), "*");
        assert_no_traversal(&f);
    }

    #[test]
    fn directory_glob_with_traversal() {
        let cwd = abs("/workspace");
        let f = parse_filter("...{./packages/*}", cwd).unwrap();
        assert!(!f.exclude);
        assert_directory_glob(&f, abs("/workspace/packages"), "*");
        assert_traversal(&f, TraversalDirection::Dependents, false);
    }

    #[test]
    fn directory_glob_parent_prefix() {
        // `../*` from a subdirectory should resolve base to parent
        let cwd = abs("/workspace/packages/app");
        let f = parse_filter("../*", cwd).unwrap();
        assert_directory_glob(&f, abs("/workspace/packages"), "*");
    }

    #[test]
    fn directory_glob_dotdot_in_base() {
        // `../foo/*` — `..` in the non-glob base is normalised before glob matching.
        // Matches Node's path.join('/ws/packages/app', '../foo/*') → '/ws/packages/foo/*'.
        let cwd = abs("/workspace/packages/app");
        let f = parse_filter("../foo/*", cwd).unwrap();
        assert_directory_glob(&f, abs("/workspace/packages/foo"), "*");
    }

    // ── Direct resolve_directory_pattern tests ──────────────────────────────

    #[test]
    fn dir_pattern_plain_path() {
        let cwd = abs("/workspace");
        let dp = resolve_directory_pattern("./packages/app", cwd).unwrap();
        assert!(
            matches!(&dp, DirectoryPattern::Exact(p) if p.as_ref() == abs("/workspace/packages/app"))
        );
    }

    #[test]
    fn dir_pattern_dot() {
        let cwd = abs("/workspace/packages/app");
        let dp = resolve_directory_pattern(".", cwd).unwrap();
        assert!(
            matches!(&dp, DirectoryPattern::Exact(p) if p.as_ref() == abs("/workspace/packages/app"))
        );
    }

    #[test]
    fn dir_pattern_dotdot() {
        let cwd = abs("/workspace/packages/app");
        let dp = resolve_directory_pattern("..", cwd).unwrap();
        assert!(
            matches!(&dp, DirectoryPattern::Exact(p) if p.as_ref() == abs("/workspace/packages"))
        );
    }

    #[test]
    fn dir_pattern_normalises_dotdot_in_middle() {
        let cwd = abs("/workspace");
        let dp = resolve_directory_pattern("./foo/../bar", cwd).unwrap();
        assert!(matches!(&dp, DirectoryPattern::Exact(p) if p.as_ref() == abs("/workspace/bar")));
    }

    #[test]
    fn dir_pattern_glob_star() {
        let cwd = abs("/workspace");
        let dp = resolve_directory_pattern("./packages/*", cwd).unwrap();
        match &dp {
            DirectoryPattern::Glob { base, pattern } => {
                assert_eq!(base.as_ref(), abs("/workspace/packages"));
                assert_eq!(pattern.to_string(), "*");
            }
            DirectoryPattern::Exact(p) => panic!("expected Glob, got Exact({p:?})"),
        }
    }

    #[test]
    fn dir_pattern_glob_double_star() {
        let cwd = abs("/workspace");
        let dp = resolve_directory_pattern("./packages/**", cwd).unwrap();
        match &dp {
            DirectoryPattern::Glob { base, pattern } => {
                assert_eq!(base.as_ref(), abs("/workspace/packages"));
                assert_eq!(pattern.to_string(), "**");
            }
            DirectoryPattern::Exact(p) => panic!("expected Glob, got Exact({p:?})"),
        }
    }

    #[test]
    fn dir_pattern_bare_glob_star() {
        // `*` with no path prefix — base should resolve to cwd
        let cwd = abs("/workspace");
        let dp = resolve_directory_pattern("*", cwd).unwrap();
        match &dp {
            DirectoryPattern::Glob { base, pattern } => {
                assert_eq!(base.as_ref(), abs("/workspace"));
                assert_eq!(pattern.to_string(), "*");
            }
            DirectoryPattern::Exact(p) => panic!("expected Glob, got Exact({p:?})"),
        }
    }

    #[test]
    fn dir_pattern_dotdot_before_glob() {
        let cwd = abs("/workspace/packages/app");
        let dp = resolve_directory_pattern("../*", cwd).unwrap();
        match &dp {
            DirectoryPattern::Glob { base, pattern } => {
                assert_eq!(base.as_ref(), abs("/workspace/packages"));
                assert_eq!(pattern.to_string(), "*");
            }
            DirectoryPattern::Exact(p) => panic!("expected Glob, got Exact({p:?})"),
        }
    }

    #[test]
    fn dir_pattern_nested_glob() {
        let cwd = abs("/workspace");
        let dp = resolve_directory_pattern("./packages/*/src", cwd).unwrap();
        match &dp {
            DirectoryPattern::Glob { base, pattern } => {
                assert_eq!(base.as_ref(), abs("/workspace/packages"));
                assert_eq!(pattern.to_string(), "*/src");
            }
            DirectoryPattern::Exact(p) => panic!("expected Glob, got Exact({p:?})"),
        }
    }
}
