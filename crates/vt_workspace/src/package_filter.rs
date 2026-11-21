//! Package filter types and parsing for pnpm-style `--filter` selectors.
//!
//! # Design
//!
//! Package selection is deliberately separated from task matching (two-stage model).
//! This module handles only Stage 1: which packages to include/exclude.
//! Stage 2 (which tasks to run in those packages) lives in `vt_graph`.
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
//! - `[<since>]`     → packages with files changed since a git revision
//! - `foo[<since>]`  → same, narrowed to packages matching `foo`
//!   (`{./dir}[<since>]` and `foo{./dir}[<since>]` narrow by directory)
//! - `...[<since>]`  → changed packages + their dependents (all traversal
//!   markers above apply)
//!
//! As in pnpm, a trailing `[...]` group is the `<since>` revision, so
//! `foo[ab]` is not a glob character class; one elsewhere, as in `foo[ab]*`, is.
//!
//! The `ContainingPackage` selector variant is NOT produced by `parse_filter`.
//! It is synthesized internally for `vp run build` (implicit cwd) and `vp run -t build`
//! to walk up the directory tree and find the package that contains the given path.
//! This mirrors pnpm's `findPrefix` behaviour (not [`parsePackageSelector`] behaviour).
//!
//! [`parsePackageSelector`]: https://github.com/pnpm/pnpm/blob/05dd45ea82fff9c0b687cdc8f478a1027077d343/workspace/filter-workspace-packages/src/parsePackageSelector.ts#L14-L61

use std::{ops::Deref, sync::Arc};

use vec1::Vec1;
use vt_path::AbsolutePath;
use vt_str::Str;

use crate::package_graph::PackageQuery;

/// Compiled glob pattern with its source string preserved for equality comparison.
///
/// `wax::Glob` does not implement `PartialEq`, so this wrapper compares
/// patterns by their source string representation.
#[derive(Debug, Clone)]
pub(crate) struct GlobPattern {
    glob: wax::Glob<'static>,
    source: Str,
}

impl GlobPattern {
    pub(crate) const fn new(glob: wax::Glob<'static>, source: Str) -> Self {
        Self { glob, source }
    }
}

impl PartialEq for GlobPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Deref for GlobPattern {
    type Target = wax::Glob<'static>;

    fn deref(&self) -> &Self::Target {
        &self.glob
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Types
// ────────────────────────────────────────────────────────────────────────────

/// Exact name or glob pattern for matching package names.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PackageNamePattern {
    /// Exact name (e.g. `foo`, `@scope/pkg`). O(1) hash lookup.
    ///
    /// Scoped auto-completion applies during resolution: if `"bar"` has no exact match
    /// but exactly one `@*/bar` package exists, that package is matched.
    /// pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L303-L306>
    ///
    /// When `exactly_one` is true, resolution errors unless the name picks
    /// exactly one package: none found, several sharing the name, or several
    /// scoped candidates. Set for `pkg#task` CLI specifiers; false for
    /// `--filter`, where an unmatched name only warns.
    Exact { name: Str, exactly_one: bool },

    /// Glob pattern (e.g. `@scope/*`, `*-utils`). Iterates all packages.
    ///
    /// Only `*` and `?` wildcards are supported (pnpm semantics).
    /// Stored as an owned `Glob<'static>` so the filter can outlive the input string.
    Glob(Box<GlobPattern>),
}

/// Directory matching pattern for `--filter` selectors.
///
/// Follows pnpm v7+ glob-dir semantics: plain paths are exact-only,
/// `*` / `**` opt in to descendant matching.
///
/// pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L200-L202>
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DirectoryPattern {
    /// Exact path match (no glob metacharacters in selector).
    Exact(Arc<AbsolutePath>),

    /// Glob: resolved base directory (non-glob prefix) + relative glob pattern.
    ///
    /// Matching strips `base` from a candidate path, then tests the remainder
    /// against `pattern`. For example, `./packages/*` with cwd `/ws` produces
    /// `base = /ws/packages`, `pattern = *`, which matches `/ws/packages/app`
    /// (remainder `app` matches `*`).
    Glob { base: Arc<AbsolutePath>, pattern: Box<GlobPattern> },
}

/// What packages to initially match.
///
/// The enum prevents the all-`None` invalid state that would arise from a struct
/// with all optional fields (as in pnpm's independent optional fields).
#[derive(Debug, Clone, PartialEq)]
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

    /// Select the workspace root package (the package with empty relative path).
    /// Produced by `-w` / `--workspace-root`.
    WorkspaceRoot,

    /// Packages with files changed since a git revision, optionally narrowed
    /// to those that also match `name` and `directory`.
    /// Produced by `--filter "[<since>]"`, `"foo[<since>]"`, `"{./dir}[<since>]"`
    /// and `"foo{./dir}[<since>]"`.
    ChangedSince {
        since: Str,
        name: Option<PackageNamePattern>,
        directory: Option<DirectoryPattern>,
    },
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PackageFilter {
    /// When `true`, packages matching this filter are **excluded** from the result.
    /// Produced by a leading `!` in the filter string.
    pub(crate) exclude: bool,

    /// Which packages to initially match.
    pub(crate) selector: PackageSelector,

    /// Optional graph expansion from the initial match.
    /// `None` = exact match only (no traversal).
    pub(crate) traversal: Option<GraphTraversal>,

    /// Original `--filter` token that produced this filter.
    /// `None` for synthetic filters (implicit cwd, package name, `-w`).
    pub(crate) source: Option<Str>,
}

// ────────────────────────────────────────────────────────────────────────────
// Error
// ────────────────────────────────────────────────────────────────────────────

/// Errors that can occur when parsing a `--filter` string.
#[derive(Debug, thiserror::Error)]
pub enum PackageFilterParseError {
    #[error("Empty filter selector")]
    EmptySelector,

    #[error(transparent)]
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

    #[error("cannot specify package name with --workspace-root")]
    PackageNameWithWorkspaceRoot { package_name: Str },

    #[error("--filter value is empty")]
    EmptyFilter,

    #[error("invalid --filter expression")]
    InvalidFilter(#[from] PackageFilterParseError),
}

/// CLI arguments for selecting which packages a command applies to.
///
/// Use `#[clap(flatten)]` to embed these in a parent clap struct.
/// Call [`into_package_query`](Self::into_package_query) to convert into an opaque [`PackageQuery`].
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[expect(clippy::struct_excessive_bools, reason = "CLI flags are naturally boolean")]
pub struct PackageQueryArgs {
    /// Select all packages in the workspace.
    #[clap(default_value = "false", short, long)]
    recursive: bool,

    /// Select the current package and its transitive dependencies.
    #[clap(default_value = "false", short, long)]
    transitive: bool,

    /// Select the workspace root package.
    #[clap(default_value = "false", short = 'w', long = "workspace-root")]
    workspace_root: bool,

    /// Match packages by name, directory, or glob pattern.
    #[clap(
        short = 'F',
        long = "filter",
        num_args = 1,
        long_help = "\
Match packages by name, directory, or glob pattern.

  --filter <pattern>        Select by package name (e.g. foo, @scope/*)
  --filter ./<dir>          Select packages under a directory
  --filter {<dir>}          Same as ./<dir>, but allows traversal suffixes
  --filter <pattern>...     Select package and its dependencies
  --filter ...<pattern>     Select package and its dependents
  --filter <pattern>^...    Select only the dependencies (exclude the package itself)
  --filter [<since>]        Select packages changed since a git revision (e.g. [origin/main])
  --filter ...[<since>]     Select changed packages and their dependents
  --filter !<pattern>       Exclude packages matching the pattern"
    )]
    filters: Vec<Str>,

    /// Exit with a non-zero status if a `--filter` expression matches no packages.
    ///
    /// Without this flag, an unmatched filter (a typo, an empty glob, or a
    /// traversal like `{.}^...` that collapses to zero on a leaf package) only
    /// produces a warning and the command exits successfully.
    #[clap(long = "fail-if-no-match", default_value = "false")]
    pub fail_if_no_match: bool,
}

impl PackageQueryArgs {
    /// Convert CLI arguments into an opaque [`PackageQuery`].
    ///
    /// `package_name` is the optional package name from a `package#task` specifier.
    /// `cwd` is the working directory (used as fallback when no package name or filter is given).
    ///
    /// Returns `(query, is_cwd_only)` where `is_cwd_only` is `true` when the query
    /// falls through to the implicit-cwd path (no `-r`, `-t`, `-w`, `--filter`,
    /// or explicit package name).
    ///
    /// # Errors
    ///
    /// Returns [`PackageQueryError`] if conflicting flags are set, a package name
    /// is specified with `--recursive` or `--filter`, or a filter expression is invalid.
    #[expect(clippy::too_many_lines, reason = "single exhaustive match")]
    pub fn into_package_query(
        self,
        package_name: Option<Str>,
        cwd: &Arc<AbsolutePath>,
    ) -> Result<(PackageQuery, bool), PackageQueryError> {
        // `fail_if_no_match` is read directly from the field before this
        // method consumes `self`; it does not affect query construction.
        let Self { recursive, transitive, workspace_root, filters, fail_if_no_match: _ } = self;

        // Collect filter tokens from all `--filter` arguments, splitting on whitespace.
        let mut filter_tokens = Vec::<Str>::with_capacity(filters.len());
        for filter in filters {
            let mut is_empty = true;
            for filter_token in filter.split_ascii_whitespace() {
                is_empty = false;
                filter_tokens.push(filter_token.into());
            }
            // Error if any --filter value is empty or whitespace-only.
            if is_empty {
                return Err(PackageQueryError::EmptyFilter);
            }
        }
        // We have checked that filter_tokens is non-empty if any filters were provided,
        // If no tokens are collected, it means no filters were provided.
        let filter_tokens: Option<Vec1<Str>> = Vec1::try_from_vec(filter_tokens).ok();

        // Error arms only match the conflicting fields (wildcards for the rest).
        // Success arms explicitly match every field — no wildcards.
        match (recursive, transitive, workspace_root, filter_tokens, package_name) {
            // ------------------------- error cases --------------------------------

            // --recursive --transitive
            (true, true, _, _, _) => Err(PackageQueryError::RecursiveTransitiveConflict),
            // --recursive --filter
            (true, _, _, Some(_), _) => Err(PackageQueryError::FilterWithRecursive),
            // --recursive <pkg>#<task>
            (true, false, _, _, Some(package_name)) => {
                Err(PackageQueryError::PackageNameWithRecursive { package_name })
            }
            // --transitive --filter
            (false, true, _, Some(_), _) => Err(PackageQueryError::FilterWithTransitive),
            // --filter <pkg>#<task>
            (_, _, _, Some(_), Some(package_name)) => {
                Err(PackageQueryError::PackageNameWithFilter { package_name })
            }
            // --workspace-root <pkg>#<task>
            (_, _, true, _, Some(package_name)) => {
                Err(PackageQueryError::PackageNameWithWorkspaceRoot { package_name })
            }

            // ------------------------ success cases -------------------------------

            // --recursive (--workspace-root is redundant)
            (true, false, true | false, None, None) => Ok((PackageQuery::all(), false)),
            // --filter [--workspace-root]
            (false, false, workspace_root, Some(filter_tokens), None) => {
                let mut parsed: Vec1<PackageFilter> =
                    filter_tokens.try_mapped(|f| parse_filter(&f, cwd))?;
                if workspace_root {
                    parsed.push(PackageFilter {
                        exclude: false,
                        selector: PackageSelector::WorkspaceRoot,
                        traversal: None,
                        source: None,
                    });
                }
                Ok((PackageQuery::filters(parsed), false))
            }
            // --workspace-root [--transitive]
            (false, transitive, true, None, None) => {
                let traversal = if transitive {
                    Some(GraphTraversal {
                        direction: TraversalDirection::Dependencies,
                        exclude_self: false,
                    })
                } else {
                    None
                };
                Ok((
                    PackageQuery::filters(Vec1::new(PackageFilter {
                        exclude: false,
                        selector: PackageSelector::WorkspaceRoot,
                        traversal,
                        source: None,
                    })),
                    false,
                ))
            }
            // [--transitive] <pkg>#<task>
            (false, transitive, false, None, Some(name)) => {
                let traversal = if transitive {
                    Some(GraphTraversal {
                        direction: TraversalDirection::Dependencies,
                        exclude_self: false,
                    })
                } else {
                    None
                };
                Ok((
                    PackageQuery::filters(Vec1::new(PackageFilter {
                        exclude: false,
                        selector: PackageSelector::Name(PackageNamePattern::Exact {
                            name,
                            exactly_one: true,
                        }),
                        traversal,
                        source: None,
                    })),
                    false,
                ))
            }
            // --transitive
            (false, true, false, None, None) => Ok((
                PackageQuery::filters(Vec1::new(PackageFilter {
                    exclude: false,
                    selector: PackageSelector::ContainingPackage(Arc::clone(cwd)),
                    traversal: Some(GraphTraversal {
                        direction: TraversalDirection::Dependencies,
                        exclude_self: false,
                    }),
                    source: None,
                })),
                false,
            )),
            // (no flags, implicit cwd)
            (false, false, false, None, None) => Ok((
                PackageQuery::filters(Vec1::new(PackageFilter {
                    exclude: false,
                    selector: PackageSelector::ContainingPackage(Arc::clone(cwd)),
                    traversal: None,
                    source: None,
                })),
                true,
            )),
        }
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
    let (selector, supports_traversal) = parse_core_selector(core, cwd)?;

    // pnpm discards traversal on unbraced path selectors — `..` (parent dir)
    // and `...` (traversal) are ambiguous. Braces disambiguate: `{./path}...`.
    // Ref: https://github.com/pnpm/pnpm/issues/1651
    let traversal = if supports_traversal { traversal } else { None };

    Ok(PackageFilter { exclude, selector, traversal, source: Some(Str::from(input)) })
}

/// Parse the core selector string (after stripping `!` and `...` markers).
///
/// Implements pnpm's [`SELECTOR_REGEX`] logic: `^([^.][^{}[\]]*)?(\{[^}]+\})?(\[[^\]]+\])?$`
///
/// [`SELECTOR_REGEX`]: https://github.com/pnpm/pnpm/blob/05dd45ea82fff9c0b687cdc8f478a1027077d343/workspace/filter-workspace-packages/src/parsePackageSelector.ts#L37
///
/// Decision tree:
/// 0. If the string ends with a `[<since>]` group (see [`split_since`]), parse
///    the rest as an optional name and brace-directory narrowing the changed packages.
/// 1. If the string ends with `}` and contains a `{`, split name and brace-directory.
///    The name part must not start with `.` for the brace split to be valid
///    (per the regex rule that Group 1 must not start with `.`).
/// 2. If the string starts with `.`, treat the whole thing as a relative path.
/// 3. Otherwise treat as a name pattern (exact or glob).
///
/// Returns `(selector, supports_traversal)`. Unbraced `.`-prefix path selectors
/// return `false` because pnpm discards `...` traversal on them (ambiguity with `..`).
fn parse_core_selector(
    core: &str,
    cwd: &AbsolutePath,
) -> Result<(PackageSelector, bool), PackageFilterParseError> {
    if let Some((name_part, dir_inner, since)) = split_since(core) {
        let name = if name_part.is_empty() { None } else { Some(build_name_pattern(name_part)?) };
        let directory = dir_inner.map(|dir| resolve_directory_pattern(dir, cwd)).transpose()?;
        return Ok((PackageSelector::ChangedSince { since: since.into(), name, directory }, true));
    }

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
                Ok((PackageSelector::Directory(directory), true))
            } else {
                // Name and directory combined: `foo{./bar}`.
                let name = build_name_pattern(name_part)?;
                Ok((PackageSelector::NameAndDirectory { name, directory }, true))
            };
        }
        // name_part starts with `.`: fall through — treat entire core as a relative path.
    }

    // If the core starts with `.`, it's a relative path to a directory.
    // This handles `.`, `..`, `./foo`, `../foo`, `./foo/*`, `./foo/**`.
    // Traversal is NOT supported — pnpm discards `...` on unbraced path selectors.
    if core.starts_with('.') {
        let directory = resolve_directory_pattern(core, cwd)?;
        return Ok((PackageSelector::Directory(directory), false));
    }

    // Guard against an empty selector reaching here.
    if core.is_empty() {
        return Err(PackageFilterParseError::EmptySelector);
    }

    // Plain name or glob pattern.
    Ok((PackageSelector::Name(build_name_pattern(core)?), true))
}

/// Split a selector that ends with a `[<since>]` group into its name, its
/// brace-directory contents and the revision.
///
/// Mirrors the `^([^.][^{}[\]]*)?(\{[^}]+\})?(\[[^\]]+\])?$` regex of pnpm's
/// [`parse_project_selector`]: the revision is non-empty and has no `]`, the
/// brace group (if any) has no `}`, and the name doesn't start with `.` and has
/// no `{`, `}`, `[` or `]` after its first character. Returns `None` when the
/// selector doesn't have that shape, so it's parsed as before (e.g. `[ab]*`
/// stays a glob).
///
/// [`parse_project_selector`]: https://github.com/pnpm/pnpm/blob/main/pnpm/crates/workspace-projects-filter/src/parse_project_selector.rs
fn split_since(core: &str) -> Option<(&str, Option<&str>, &str)> {
    let without_closing = core.strip_suffix(']')?;
    let open = without_closing.rfind('[')?;
    let since = &without_closing[open + 1..];
    if since.is_empty() || since.contains(']') {
        return None;
    }
    let before = &without_closing[..open];

    let (name, dir_inner) =
        match before.strip_suffix('}').and_then(|b| b.rfind('{').map(|i| (b, i))) {
            Some((without_brace, brace)) => {
                let dir_inner = &without_brace[brace + 1..];
                if dir_inner.is_empty() || dir_inner.contains('}') {
                    return None;
                }
                (&without_brace[..brace], Some(dir_inner))
            }
            None => (before, None),
        };

    let mut name_chars = name.chars();
    if let Some(first) = name_chars.next()
        && (first == '.' || name_chars.any(|c| matches!(c, '{' | '}' | '[' | ']')))
    {
        return None;
    }
    Some((name, dir_inner, since))
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
        Some(pattern) => Ok(DirectoryPattern::Glob {
            base,
            pattern: Box::new(GlobPattern::new(pattern, path_str.into())),
        }),
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
    cwd.join(path_str).clean().into()
}

/// Build a [`PackageNamePattern`] from a name or glob string.
///
/// Uses [`wax::Glob::partition`] to determine if the pattern contains variant
/// (non-literal) components. If it does, the pattern is a glob; otherwise exact.
fn build_name_pattern(name: &str) -> Result<PackageNamePattern, PackageFilterParseError> {
    let glob = wax::Glob::new(name)?.into_owned();
    if glob.clone().partition().1.is_some() {
        Ok(PackageNamePattern::Glob(Box::new(GlobPattern::new(glob, name.into()))))
    } else {
        Ok(PackageNamePattern::Exact { name: name.into(), exactly_one: false })
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
            PackageSelector::Name(PackageNamePattern::Exact { name: n, .. }) => {
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
                name: PackageNamePattern::Exact { name: n, .. },
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
                name: PackageNamePattern::Exact { name: n, .. },
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

    // ── Unbraced path selectors discard traversal (pnpm compat) ─────────

    #[test]
    fn unbraced_path_discards_trailing_dots() {
        // `./foo...` — `...` is stripped but traversal is discarded for unbraced paths.
        let cwd = abs("/workspace");
        let f = parse_filter("./foo...", cwd).unwrap();
        assert_directory(&f, abs("/workspace/foo"));
        assert_no_traversal(&f);
    }

    #[test]
    fn unbraced_dot_discards_trailing_dots() {
        // `....` = `.` (cwd) + `...` — traversal discarded.
        let cwd = abs("/workspace/packages/app");
        let f = parse_filter("....", cwd).unwrap();
        assert_directory(&f, abs("/workspace/packages/app"));
        assert_no_traversal(&f);
    }

    #[test]
    fn unbraced_dotdot_discards_leading_dots() {
        // `......` = `...` (dependents) + `...` (remaining = `...`)
        // After stripping both `...` markers, core = empty → error? No:
        // `...../foo` = `...` (dependents) + `../foo` — traversal discarded.
        let cwd = abs("/workspace/packages/app");
        let f = parse_filter("...../foo", cwd).unwrap();
        assert_directory(&f, abs("/workspace/packages/foo"));
        assert_no_traversal(&f);
    }

    #[test]
    fn braced_path_preserves_traversal() {
        // `{./foo}...` — braces make traversal work.
        let cwd = abs("/workspace");
        let f = parse_filter("{./foo}...", cwd).unwrap();
        assert_directory(&f, abs("/workspace/foo"));
        assert_traversal(&f, TraversalDirection::Dependencies, false);
    }

    // ── [<since>] changed-since selectors ──────────────────────────────────

    /// The `ChangedSince` parts of `filter`: revision, name, directory.
    fn changed_since(
        filter: &PackageFilter,
    ) -> (&str, Option<&PackageNamePattern>, Option<&DirectoryPattern>) {
        match &filter.selector {
            PackageSelector::ChangedSince { since, name, directory } => {
                (since.as_str(), name.as_ref(), directory.as_ref())
            }
            other => panic!("expected ChangedSince, got {other:?}"),
        }
    }

    fn exact(name: &str) -> PackageNamePattern {
        PackageNamePattern::Exact { name: name.into(), exactly_one: false }
    }

    #[test]
    fn since_alone() {
        let f = parse_filter("[origin/main]", abs("/workspace")).unwrap();
        assert!(!f.exclude);
        assert_eq!(changed_since(&f), ("origin/main", None, None));
        assert_no_traversal(&f);
    }

    #[test]
    fn since_with_dependents() {
        let f = parse_filter("...[HEAD~1]", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("HEAD~1", None, None));
        assert_traversal(&f, TraversalDirection::Dependents, false);
    }

    #[test]
    fn since_with_dependencies() {
        let f = parse_filter("[HEAD~1]...", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("HEAD~1", None, None));
        assert_traversal(&f, TraversalDirection::Dependencies, false);
    }

    #[test]
    fn since_with_both_directions() {
        let f = parse_filter("...[HEAD~1]...", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("HEAD~1", None, None));
        assert_traversal(&f, TraversalDirection::Both, false);
    }

    #[test]
    fn since_with_dependents_exclude_self() {
        let f = parse_filter("...^[main]", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("main", None, None));
        assert_traversal(&f, TraversalDirection::Dependents, true);
    }

    #[test]
    fn since_with_dependencies_exclude_self() {
        let f = parse_filter("[main]^...", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("main", None, None));
        assert_traversal(&f, TraversalDirection::Dependencies, true);
    }

    #[test]
    fn since_excluded() {
        let f = parse_filter("!...[main]", abs("/workspace")).unwrap();
        assert!(f.exclude);
        assert_eq!(changed_since(&f), ("main", None, None));
        assert_traversal(&f, TraversalDirection::Dependents, false);
    }

    #[test]
    fn since_with_name() {
        let f = parse_filter("@test/app[main]", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("main", Some(&exact("@test/app")), None));
    }

    #[test]
    fn since_with_name_glob() {
        let f = parse_filter("...@test/*[main]", abs("/workspace")).unwrap();
        let (since, name, directory) = changed_since(&f);
        assert_eq!(since, "main");
        assert!(
            matches!(name, Some(PackageNamePattern::Glob(g)) if g.to_string() == "@test/*"),
            "{name:?}"
        );
        assert!(directory.is_none());
        assert_traversal(&f, TraversalDirection::Dependents, false);
    }

    #[test]
    fn since_with_directory() {
        let f = parse_filter("{./packages/app}[main]", abs("/workspace")).unwrap();
        assert_eq!(
            changed_since(&f),
            ("main", None, Some(&DirectoryPattern::Exact(abs("/workspace/packages/app").into())))
        );
    }

    #[test]
    fn since_with_directory_glob_and_traversal() {
        let f = parse_filter("...{packages/**}[origin/main]...", abs("/workspace")).unwrap();
        let (since, name, directory) = changed_since(&f);
        assert_eq!(since, "origin/main");
        assert!(name.is_none());
        assert!(
            matches!(
                directory,
                Some(DirectoryPattern::Glob { base, pattern })
                    if base.as_ref() == abs("/workspace/packages") && pattern.to_string() == "**"
            ),
            "{directory:?}"
        );
        assert_traversal(&f, TraversalDirection::Both, false);
    }

    #[test]
    fn since_with_name_and_directory() {
        let f = parse_filter("app{./packages/app}[main]", abs("/workspace")).unwrap();
        assert_eq!(
            changed_since(&f),
            (
                "main",
                Some(&exact("app")),
                Some(&DirectoryPattern::Exact(abs("/workspace/packages/app").into()))
            )
        );
    }

    #[test]
    fn since_keeps_revision_syntax() {
        // `^`, `~`, `@{...}` and `..` belong to the revision, not to the selector.
        for since in ["HEAD^", "HEAD~2", "main@{1}", "v1.0.0", "a1b2c3d", "origin/feat/x"] {
            let f = parse_filter(&vt_str::format!("...[{since}]"), abs("/workspace")).unwrap();
            assert_eq!(changed_since(&f), (since, None, None));
        }
        let f = parse_filter("[HEAD^]...", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("HEAD^", None, None));
        assert_traversal(&f, TraversalDirection::Dependencies, false);
    }

    #[test]
    fn trailing_bracket_group_is_since_not_glob_class() {
        // Behavior change: `foo[ab]` was a glob matching `fooa` and `foob`.
        let f = parse_filter("foo[ab]", abs("/workspace")).unwrap();
        assert_eq!(changed_since(&f), ("ab", Some(&exact("foo")), None));
    }

    #[test]
    fn bracket_group_not_at_the_end_stays_glob() {
        let f = parse_filter("foo[ab]*", abs("/workspace")).unwrap();
        assert_glob_name(&f, "foo[ab]*");
        let f = parse_filter("[ab]*", abs("/workspace")).unwrap();
        assert_glob_name(&f, "[ab]*");
    }

    #[test]
    fn bracket_groups_pnpm_regex_rejects_stay_names() {
        // Two bracket groups: the name may not contain `]` after its first character.
        let f = parse_filter("[x][y]", abs("/workspace")).unwrap();
        assert_glob_name(&f, "[x][y]");
        // A `]` in the revision.
        let f = parse_filter("a[b]c]", abs("/workspace"));
        assert!(!matches!(
            f,
            Ok(PackageFilter { selector: PackageSelector::ChangedSince { .. }, .. })
        ));
    }

    #[test]
    fn empty_since_is_not_a_since_selector() {
        // pnpm's `\[[^\]]+\]` needs at least one character.
        let f = parse_filter("foo[]", abs("/workspace"));
        assert!(!matches!(
            f,
            Ok(PackageFilter { selector: PackageSelector::ChangedSince { .. }, .. })
        ));
    }

    #[test]
    fn unbraced_path_with_bracket_stays_a_path() {
        // As in pnpm, a `.`-prefixed selector is a location; use `{./dir}[<since>]`.
        let f = parse_filter("./foo[main]", abs("/workspace")).unwrap();
        assert!(matches!(&f.selector, PackageSelector::Directory(_)), "{:?}", f.selector);
    }

    // ── -w / --workspace-root flag ──────────────────────────────────────────

    #[test]
    fn workspace_root_produces_selector() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace/packages/app"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: true,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (query, _) = args.into_package_query(None, &cwd).unwrap();
        match &query.0 {
            crate::package_graph::PackageQueryKind::Filters(filters) => {
                assert_eq!(filters.len(), 1);
                assert!(!filters[0].exclude);
                assert!(
                    matches!(&filters[0].selector, PackageSelector::WorkspaceRoot),
                    "expected WorkspaceRoot, got {:?}",
                    filters[0].selector
                );
                assert_no_traversal(&filters[0]);
            }
            crate::package_graph::PackageQueryKind::All => panic!("expected Filters, got All"),
        }
    }

    #[test]
    fn workspace_root_with_recursive_returns_all() {
        // -w is redundant with -r (all packages already includes root).
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: true,
            transitive: false,
            workspace_root: true,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (query, _) = args.into_package_query(None, &cwd).unwrap();
        assert!(
            matches!(&query.0, crate::package_graph::PackageQueryKind::All),
            "expected All, got {:?}",
            query.0
        );
    }

    #[test]
    fn workspace_root_with_transitive() {
        // -w -t: workspace root with transitive dependencies.
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace/packages/app"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: true,
            workspace_root: true,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (query, _) = args.into_package_query(None, &cwd).unwrap();
        match &query.0 {
            crate::package_graph::PackageQueryKind::Filters(filters) => {
                assert_eq!(filters.len(), 1);
                assert!(
                    matches!(&filters[0].selector, PackageSelector::WorkspaceRoot),
                    "expected WorkspaceRoot, got {:?}",
                    filters[0].selector
                );
                assert_traversal(&filters[0], TraversalDirection::Dependencies, false);
            }
            crate::package_graph::PackageQueryKind::All => panic!("expected Filters, got All"),
        }
    }

    #[test]
    fn workspace_root_with_filter_unions() {
        // -w --filter foo: workspace root + parsed filter.
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: true,
            filters: vec![Str::from("foo")],
            fail_if_no_match: false,
        };
        let (query, _) = args.into_package_query(None, &cwd).unwrap();
        match &query.0 {
            crate::package_graph::PackageQueryKind::Filters(filters) => {
                assert_eq!(filters.len(), 2);
                assert_exact_name(&filters[0], "foo");
                assert!(
                    matches!(&filters[1].selector, PackageSelector::WorkspaceRoot),
                    "expected WorkspaceRoot, got {:?}",
                    filters[1].selector
                );
            }
            crate::package_graph::PackageQueryKind::All => panic!("expected Filters, got All"),
        }
    }

    #[test]
    fn workspace_root_conflicts_with_package_name() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: true,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        assert!(matches!(
            args.into_package_query(Some(Str::from("app")), &cwd),
            Err(PackageQueryError::PackageNameWithWorkspaceRoot { .. })
        ));
    }

    // ── source field ───────────────────────────────────────────────────────

    #[test]
    fn parse_filter_sets_source() {
        let cwd = abs("/workspace");
        let f = parse_filter("@test/app...", cwd).unwrap();
        assert_eq!(f.source.as_deref(), Some("@test/app..."));
    }

    #[test]
    fn filter_source_preserved_after_whitespace_split() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: vec![Str::from("a b")],
            fail_if_no_match: false,
        };
        let (query, _) = args.into_package_query(None, &cwd).unwrap();
        match &query.0 {
            crate::package_graph::PackageQueryKind::Filters(filters) => {
                assert_eq!(filters.len(), 2);
                assert_eq!(filters[0].source.as_deref(), Some("a"));
                assert_eq!(filters[1].source.as_deref(), Some("b"));
            }
            crate::package_graph::PackageQueryKind::All => panic!("expected Filters, got All"),
        }
    }

    #[test]
    fn synthetic_workspace_root_filter_has_no_source() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: true,
            filters: vec![Str::from("foo")],
            fail_if_no_match: false,
        };
        let (query, _) = args.into_package_query(None, &cwd).unwrap();
        match &query.0 {
            crate::package_graph::PackageQueryKind::Filters(filters) => {
                assert_eq!(filters.len(), 2);
                assert_eq!(filters[0].source.as_deref(), Some("foo"));
                assert!(filters[1].source.is_none());
            }
            crate::package_graph::PackageQueryKind::All => panic!("expected Filters, got All"),
        }
    }

    #[test]
    fn implicit_cwd_filter_has_no_source() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace/packages/app"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (query, _) = args.into_package_query(None, &cwd).unwrap();
        match &query.0 {
            crate::package_graph::PackageQueryKind::Filters(filters) => {
                assert_eq!(filters.len(), 1);
                assert!(filters[0].source.is_none());
            }
            crate::package_graph::PackageQueryKind::All => panic!("expected Filters, got All"),
        }
    }

    // ── empty filter validation ─────────────────────────────────────────────

    #[test]
    fn empty_filter_string_errors() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: vec![Str::from("")],
            fail_if_no_match: false,
        };
        assert!(matches!(args.into_package_query(None, &cwd), Err(PackageQueryError::EmptyFilter)));
    }

    #[test]
    fn whitespace_only_filter_errors() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: vec![Str::from("   ")],
            fail_if_no_match: false,
        };
        assert!(matches!(args.into_package_query(None, &cwd), Err(PackageQueryError::EmptyFilter)));
    }

    #[test]
    fn second_filter_empty_errors() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: vec![Str::from("foo"), Str::from("")],
            fail_if_no_match: false,
        };
        assert!(matches!(args.into_package_query(None, &cwd), Err(PackageQueryError::EmptyFilter)));
    }

    #[test]
    fn first_filter_empty_with_valid_second_errors() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: vec![Str::from(""), Str::from("foo")],
            fail_if_no_match: false,
        };
        assert!(matches!(args.into_package_query(None, &cwd), Err(PackageQueryError::EmptyFilter)));
    }

    #[test]
    fn valid_filter_with_whitespace_only_second_errors() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: vec![Str::from("foo"), Str::from("  \t  ")],
            fail_if_no_match: false,
        };
        assert!(matches!(args.into_package_query(None, &cwd), Err(PackageQueryError::EmptyFilter)));
    }

    // ── is_cwd_only flag ─────────────────────────────────────────────────────

    #[test]
    fn is_cwd_only_true_for_bare_invocation() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace/packages/app"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (_, is_cwd_only) = args.into_package_query(None, &cwd).unwrap();
        assert!(is_cwd_only);
    }

    #[test]
    fn is_cwd_only_false_with_package_name() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (_, is_cwd_only) = args.into_package_query(Some(Str::from("app")), &cwd).unwrap();
        assert!(!is_cwd_only);
    }

    #[test]
    fn is_cwd_only_false_with_transitive() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace/packages/app"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: true,
            workspace_root: false,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (_, is_cwd_only) = args.into_package_query(None, &cwd).unwrap();
        assert!(!is_cwd_only);
    }

    #[test]
    fn is_cwd_only_false_with_recursive() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: true,
            transitive: false,
            workspace_root: false,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (_, is_cwd_only) = args.into_package_query(None, &cwd).unwrap();
        assert!(!is_cwd_only);
    }

    #[test]
    fn is_cwd_only_false_with_filter() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: false,
            filters: vec![Str::from("foo")],
            fail_if_no_match: false,
        };
        let (_, is_cwd_only) = args.into_package_query(None, &cwd).unwrap();
        assert!(!is_cwd_only);
    }

    #[test]
    fn is_cwd_only_false_with_workspace_root() {
        let cwd: Arc<AbsolutePath> = Arc::from(abs("/workspace"));
        let args = PackageQueryArgs {
            recursive: false,
            transitive: false,
            workspace_root: true,
            filters: Vec::new(),
            fail_if_no_match: false,
        };
        let (_, is_cwd_only) = args.into_package_query(None, &cwd).unwrap();
        assert!(!is_cwd_only);
    }
}
