//! Package graph with indexed lookups and filter resolution.
//!
//! This module owns the `IndexedPackageGraph` (the read-only package dependency
//! graph enriched with hash-map indices for O(1) lookups) and the `PackageQuery`
//! type that expresses which packages a task query applies to.
//!
//! # Two-stage model
//!
//! Package selection (this module) is deliberately decoupled from task matching
//! (the task-query layer in `vt_graph`). `resolve_query` returns a *package
//! subgraph* — a `DiGraphMap<PackageNodeIndex, ()>` containing only the selected
//! packages and the original dependency edges between them. The task-query layer
//! then maps each selected package to its task node, reconnecting across
//! task-lacking nodes.

use std::sync::Arc;

use petgraph::{Direction, graph::DiGraph, prelude::DiGraphMap, visit::EdgeRef};
use rustc_hash::{FxHashMap, FxHashSet};
use vec1::{Vec1, smallvec_v1::SmallVec1};
use vt_path::AbsolutePath;
use vt_str::Str;

use crate::{
    DependencyType, PackageInfo, PackageIx, PackageNodeIndex,
    changed_since::{ChangedSinceError, uses_changed_catalog, workspace_changes},
    package_filter::{
        DirectoryPattern, GraphTraversal, PackageFilter, PackageNamePattern, PackageSelector,
        TraversalDirection,
    },
};

// ────────────────────────────────────────────────────────────────────────────
// PackageQuery
// ────────────────────────────────────────────────────────────────────────────

/// Specifies which packages a task query applies to.
///
/// This type is opaque — construct it via [`PackageQueryArgs::into_package_query`]
/// (from `package_filter`).
///
/// [`PackageQueryArgs::into_package_query`]: crate::package_filter::PackageQueryArgs::into_package_query
#[derive(Debug, PartialEq)]
pub struct PackageQuery(pub(crate) PackageQueryKind);

#[derive(Debug, PartialEq)]
pub(crate) enum PackageQueryKind {
    /// One or more `--filter` expressions.
    ///
    /// Inclusions are unioned; exclusions are subtracted from the union.
    /// If all filters are exclusions, the starting set is the full workspace
    /// pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L167-L168>
    Filters(Vec1<PackageFilter>),

    /// All packages in the workspace, in topological dependency order.
    ///
    /// Produced by `--recursive` / `-r`.
    All,
}

impl PackageQuery {
    /// All packages in the workspace.
    pub(crate) const fn all() -> Self {
        Self(PackageQueryKind::All)
    }

    /// One or more filter expressions.
    pub(crate) const fn filters(filters: Vec1<PackageFilter>) -> Self {
        Self(PackageQueryKind::Filters(filters))
    }

    /// Select the single package whose root is `path`.
    ///
    /// Used by the interactive task selector to match by filesystem path
    /// instead of package name.
    #[must_use]
    pub fn containing_package(path: Arc<AbsolutePath>) -> Self {
        Self(PackageQueryKind::Filters(Vec1::new(PackageFilter {
            exclude: false,
            selector: PackageSelector::ContainingPackage(path),
            traversal: None,
            source: None,
        })))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// FilterResolution
// ────────────────────────────────────────────────────────────────────────────

/// The result of resolving a [`PackageQuery`] against the workspace.
pub struct FilterResolution {
    /// Induced package subgraph: nodes = selected packages, edges = original dependency
    /// ordering edges between them.
    ///
    /// All original edges between selected packages are preserved regardless of how each
    /// package was selected (traversal or cherry-pick). A cherry-picked package still
    /// respects its dependencies if they happen to be in the selected set.
    ///
    /// Future `--filter-prod` support would skip `DependencyType::Dev` edges at this
    /// stage (construction time), keeping all downstream code edge-type-agnostic.
    pub package_subgraph: DiGraphMap<PackageNodeIndex, ()>,

    /// Original `--filter` strings for inclusion filters that contributed no packages
    /// to the final selected set (either the core selector matched nothing, or the
    /// selector matched but the graph traversal — e.g. `^...` — collapsed it to empty).
    ///
    /// Omits synthetic filters (implicit cwd, `-w`) since the user didn't type them.
    /// Empty when `PackageQuery::All` is used.
    pub unmatched_selectors: Vec<Str>,
}

// ────────────────────────────────────────────────────────────────────────────
// PackageQueryResolveError
// ────────────────────────────────────────────────────────────────────────────

/// Errors that can occur when resolving a [`PackageQuery`] against the workspace.
#[derive(Debug, thiserror::Error)]
pub enum PackageQueryResolveError {
    #[error(
        "Package name '{package_name}' is ambiguous; found in multiple locations: {}",
        package_paths.iter().map(|p| p.as_path().display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    AmbiguousPackageName { package_name: Str, package_paths: Box<[Arc<AbsolutePath>]> },

    #[error("Package '{package_name}' not found in the workspace")]
    PackageNotFound { package_name: Str },

    #[error("Failed to find the packages changed since '{since}'")]
    ChangedSince {
        since: Str,
        #[source]
        error: ChangedSinceError,
    },
}

/// Packages changed since each `[<since>]` revision, so filters that share a
/// revision run `git` once.
type ChangedSinceCache = FxHashMap<Str, FxHashSet<PackageNodeIndex>>;

// ────────────────────────────────────────────────────────────────────────────
// IndexedPackageGraph
// ────────────────────────────────────────────────────────────────────────────

/// Package graph with additional hash maps for quick task lookup.
#[derive(Debug)]
pub struct IndexedPackageGraph {
    graph: DiGraph<PackageInfo, DependencyType, PackageIx>,

    /// Package indices grouped by name.
    ///
    /// `SmallVec1` avoids a heap allocation in the common case (one package per name)
    /// while still supporting the rare monorepo name-collision scenario.
    indices_by_name: FxHashMap<Str, SmallVec1<[PackageNodeIndex; 1]>>,

    /// Package indices by absolute path, for O(1) lookup given a cwd.
    indices_by_path: FxHashMap<Arc<AbsolutePath>, PackageNodeIndex>,
}

impl IndexedPackageGraph {
    /// Build the index from a raw package graph.
    #[must_use]
    pub fn index(package_graph: DiGraph<PackageInfo, DependencyType, PackageIx>) -> Self {
        let indices_by_path: FxHashMap<Arc<AbsolutePath>, PackageNodeIndex> = package_graph
            .node_indices()
            .map(|idx| (Arc::clone(&package_graph[idx].absolute_path), idx))
            .collect();

        let mut indices_by_name: FxHashMap<Str, SmallVec1<[PackageNodeIndex; 1]>> =
            FxHashMap::default();
        for idx in package_graph.node_indices() {
            let name = package_graph[idx].package_json.name.clone();
            indices_by_name
                .entry(name)
                .and_modify(|v| v.push(idx))
                .or_insert_with(|| SmallVec1::new(idx));
        }

        Self { graph: package_graph, indices_by_name, indices_by_path }
    }

    // ── Public accessors ─────────────────────────────────────────────────────

    /// Reference to the underlying package graph.
    #[must_use]
    pub const fn package_graph(&self) -> &DiGraph<PackageInfo, DependencyType, PackageIx> {
        &self.graph
    }

    /// Walk up the directory tree from `cwd` to find the nearest enclosing package.
    ///
    /// Returns `None` if no package root is found anywhere above `cwd`.
    #[must_use]
    pub fn get_package_index_from_cwd(&self, cwd: &AbsolutePath) -> Option<PackageNodeIndex> {
        let mut cur = cwd;
        loop {
            if let Some(&idx) = self.indices_by_path.get(cur) {
                return Some(idx);
            }
            cur = cur.parent()?;
        }
    }

    /// Look up all package indices with the given name (exact, case-sensitive).
    #[must_use]
    pub fn get_package_indices_by_name(
        &self,
        name: &Str,
    ) -> Option<&SmallVec1<[PackageNodeIndex; 1]>> {
        self.indices_by_name.get(name)
    }

    // ── Query resolution ─────────────────────────────────────────────────────

    /// Resolve a [`PackageQuery`] into a [`FilterResolution`].
    ///
    /// For `All`, returns the full induced subgraph (every package, every edge).
    /// For `Filters`, applies the filter algorithm and returns the induced subgraph
    /// of the matching packages.
    ///
    /// # Errors
    ///
    /// Returns [`PackageQueryResolveError::AmbiguousPackageName`] when an exact
    /// package name (from a `pkg#task` specifier) matches multiple packages, and
    /// [`PackageQueryResolveError::PackageNotFound`] when it matches none.
    /// Returns [`PackageQueryResolveError::ChangedSince`] when the changes for
    /// a `[<since>]` selector can't be read from git.
    pub fn resolve_query(
        &self,
        query: &PackageQuery,
    ) -> Result<FilterResolution, PackageQueryResolveError> {
        match &query.0 {
            PackageQueryKind::All => Ok(FilterResolution {
                package_subgraph: self.full_subgraph(),
                unmatched_selectors: Vec::new(),
            }),
            PackageQueryKind::Filters(filters) => self.resolve_filters(filters.as_slice()),
        }
    }

    /// Build the full induced subgraph: every package node and every dependency edge.
    ///
    /// Used by `PackageQuery::All` (`--recursive`).
    fn full_subgraph(&self) -> DiGraphMap<PackageNodeIndex, ()> {
        let mut subgraph = DiGraphMap::new();
        for idx in self.graph.node_indices() {
            subgraph.add_node(idx);
        }
        for edge in self.graph.edge_references() {
            subgraph.add_edge(edge.source(), edge.target(), ());
        }
        subgraph
    }

    /// Resolve a slice of package filters into a `FilterResolution`.
    ///
    /// # Algorithm (follows pnpm's [`filterWorkspacePackages`])
    ///
    /// [`filterWorkspacePackages`]: https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L149-L178
    ///
    /// 1. Partition filters into inclusions (`!exclude = false`) and exclusions.
    /// 2. If there are no inclusions, start from ALL packages (exclude-only mode).
    /// 3. For each inclusion: resolve its selector to entry packages, expand via
    ///    graph traversal if requested, and union into `selected`.
    /// 4. For each exclusion: resolve + expand, then subtract from `selected`.
    /// 5. Build the induced subgraph: every original edge whose both endpoints are
    ///    in `selected` is preserved, regardless of how each endpoint was selected.
    fn resolve_filters(
        &self,
        filters: &[PackageFilter],
    ) -> Result<FilterResolution, PackageQueryResolveError> {
        let mut unmatched_selectors = Vec::new();
        let mut changed_since_cache = ChangedSinceCache::default();

        let (inclusions, exclusions): (Vec<_>, Vec<_>) = filters.iter().partition(|f| !f.exclude);

        // Start from all packages when there are no inclusions (exclude-only mode).
        let mut selected: FxHashSet<PackageNodeIndex> = if inclusions.is_empty() {
            self.graph.node_indices().collect()
        } else {
            FxHashSet::default()
        };

        // Apply inclusions: union each filter's resolved set into `selected`.
        for filter in &inclusions {
            let matched =
                self.resolve_selector_entries(&filter.selector, &mut changed_since_cache)?;
            let expanded = self.expand_traversal(matched, filter.traversal.as_ref());
            // Track filters that contribute nothing (typo, no-match glob, or a
            // traversal like `^...` that collapsed an otherwise-matching seed
            // down to zero). Synthetic filters (implicit cwd, `-w`) have no
            // user-typed source and are skipped — they aren't typos.
            if expanded.is_empty()
                && let Some(source) = &filter.source
            {
                unmatched_selectors.push(source.clone());
            }
            selected.extend(expanded);
        }

        // Apply exclusions: subtract each filter's resolved set from `selected`.
        for filter in &exclusions {
            let matched =
                self.resolve_selector_entries(&filter.selector, &mut changed_since_cache)?;
            let to_remove = self.expand_traversal(matched, filter.traversal.as_ref());
            for pkg in to_remove {
                selected.remove(&pkg);
            }
        }

        let package_subgraph = self.build_induced_subgraph(&selected);
        Ok(FilterResolution { package_subgraph, unmatched_selectors })
    }

    /// Resolve a `PackageSelector` to the set of directly matched packages
    /// (before any graph traversal expansion).
    fn resolve_selector_entries(
        &self,
        selector: &PackageSelector,
        changed_since_cache: &mut ChangedSinceCache,
    ) -> Result<FxHashSet<PackageNodeIndex>, PackageQueryResolveError> {
        let mut matched = FxHashSet::default();

        match selector {
            PackageSelector::Name(pattern) => {
                self.match_by_name_pattern(pattern, &mut matched)?;
            }

            PackageSelector::Directory(dir_pattern) => {
                self.match_by_directory_pattern(dir_pattern, &mut matched);
            }

            PackageSelector::ContainingPackage(path) => {
                // Walk up the directory tree to find the enclosing package.
                if let Some(idx) = self.get_package_index_from_cwd(path) {
                    matched.insert(idx);
                }
            }

            PackageSelector::NameAndDirectory { name, directory } => {
                // Intersection: packages satisfying both name AND directory.
                let mut by_name = FxHashSet::default();
                self.match_by_name_pattern(name, &mut by_name)?;
                let mut by_dir = FxHashSet::default();
                self.match_by_directory_pattern(directory, &mut by_dir);
                matched.extend(by_name.intersection(&by_dir));
            }

            PackageSelector::WorkspaceRoot => {
                matched.extend(self.workspace_root_index());
            }

            PackageSelector::ChangedSince { since, name, directory } => {
                if !changed_since_cache.contains_key(since) {
                    let changed = self.changed_since(since).map_err(|error| {
                        PackageQueryResolveError::ChangedSince { since: since.clone(), error }
                    })?;
                    changed_since_cache.insert(since.clone(), changed);
                }
                matched.clone_from(&changed_since_cache[since]);
                if let Some(name) = name {
                    let mut by_name = FxHashSet::default();
                    self.match_by_name_pattern(name, &mut by_name)?;
                    matched.retain(|idx| by_name.contains(idx));
                }
                if let Some(directory) = directory {
                    let mut by_dir = FxHashSet::default();
                    self.match_by_directory_pattern(directory, &mut by_dir);
                    matched.retain(|idx| by_dir.contains(idx));
                }
            }
        }

        Ok(matched)
    }

    /// The workspace root package, the one with an empty relative path.
    fn workspace_root_index(&self) -> Option<PackageNodeIndex> {
        self.graph.node_indices().find(|&idx| self.graph[idx].path.as_str().is_empty())
    }

    /// The packages with files changed since the git revision `since`.
    ///
    /// Follows pnpm's `get_changed_projects`: a changed file belongs to the
    /// deepest package that contains it, which is the workspace root package
    /// for files outside every other package. When `pnpm-workspace.yaml`
    /// changed, packages that take a changed catalog entry (`catalog:`,
    /// `catalog:<name>`) count as changed too.
    fn changed_since(&self, since: &str) -> Result<FxHashSet<PackageNodeIndex>, ChangedSinceError> {
        let mut changed = FxHashSet::default();
        // Every workspace has a root package (see `load_package_graph`).
        let Some(root) = self.workspace_root_index() else {
            return Ok(changed);
        };
        let workspace = workspace_changes(&self.graph[root].absolute_path, since)?;

        let indices_by_relative_path: FxHashMap<&str, PackageNodeIndex> =
            self.graph.node_indices().map(|idx| (self.graph[idx].path.as_str(), idx)).collect();
        for file in &workspace.files {
            let mut dir = file.as_str();
            let owner = loop {
                dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
                if let Some(&idx) = indices_by_relative_path.get(dir) {
                    break idx;
                }
                if dir.is_empty() {
                    break root;
                }
            };
            changed.insert(owner);
        }

        if !workspace.catalogs.is_empty() {
            changed.extend(self.graph.node_indices().filter(|&idx| {
                uses_changed_catalog(&self.graph[idx].package_json, &workspace.catalogs)
            }));
        }
        Ok(changed)
    }

    /// Match packages by a name pattern, inserting into `out`.
    ///
    /// For `Exact` names, scoped auto-completion applies
    /// (pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L303-L306>):
    /// if `"bar"` has no exact match but exactly one `@*/bar` package exists,
    /// that package is used instead. An `exactly_one` name that still matches
    /// no package, or several, is an error.
    fn match_by_name_pattern(
        &self,
        pattern: &PackageNamePattern,
        out: &mut FxHashSet<PackageNodeIndex>,
    ) -> Result<(), PackageQueryResolveError> {
        match pattern {
            PackageNamePattern::Exact { name, exactly_one } => {
                if let Some(indices) = self.get_package_indices_by_name(name) {
                    if *exactly_one && indices.len() > 1 {
                        return Err(self.ambiguous_package_name(name, indices));
                    }
                    out.extend(indices.iter().copied());
                } else {
                    // Scoped auto-completion: `"bar"` → `"@scope/bar"` if exactly one match.
                    let scoped_suffix = vt_str::format!("/{}", name);
                    let mut scoped: Vec<_> = self
                        .indices_by_name
                        .iter()
                        .filter(|(pkg_name, _)| {
                            pkg_name.starts_with('@') && pkg_name.ends_with(scoped_suffix.as_str())
                        })
                        .flat_map(|(_, indices)| indices.iter().copied())
                        .collect();
                    if scoped.len() == 1 {
                        out.insert(scoped[0]);
                    } else if *exactly_one {
                        if scoped.is_empty() {
                            return Err(PackageQueryResolveError::PackageNotFound {
                                package_name: name.clone(),
                            });
                        }
                        // Sorted so the listed paths don't depend on hash order.
                        scoped.sort_unstable();
                        return Err(self.ambiguous_package_name(name, &scoped));
                    }
                    // For `--filter`:
                    // 0 matches: nothing to insert; the caller warns about the filter.
                    // >1 matches: ambiguous; skip rather than guessing wrong.
                }
            }

            PackageNamePattern::Glob(glob) => {
                use wax::Program as _;
                for (pkg_name, indices) in &self.indices_by_name {
                    if glob.is_match(pkg_name.as_str()) {
                        out.extend(indices.iter().copied());
                    }
                }
            }
        }
        Ok(())
    }

    /// The error for a `pkg#task` name that matches several packages.
    fn ambiguous_package_name(
        &self,
        name: &Str,
        indices: &[PackageNodeIndex],
    ) -> PackageQueryResolveError {
        PackageQueryResolveError::AmbiguousPackageName {
            package_name: name.clone(),
            package_paths: indices
                .iter()
                .map(|i| Arc::clone(&self.graph[*i].absolute_path))
                .collect(),
        }
    }

    /// Match packages by a directory pattern, inserting into `out`.
    ///
    /// pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L317-L324>
    fn match_by_directory_pattern(
        &self,
        pattern: &DirectoryPattern,
        out: &mut FxHashSet<PackageNodeIndex>,
    ) {
        match pattern {
            DirectoryPattern::Exact(dir) => {
                // O(1) exact lookup by path.
                if let Some(&idx) = self.indices_by_path.get(dir) {
                    out.insert(idx);
                }
            }
            DirectoryPattern::Glob { base, pattern } => {
                use wax::Program as _;
                for idx in self.graph.node_indices() {
                    let pkg_path = &self.graph[idx].absolute_path;
                    if let Ok(remainder) = pkg_path.as_path().strip_prefix(base.as_path())
                        && pattern.is_match(remainder)
                    {
                        out.insert(idx);
                    }
                }
            }
        }
    }

    /// Expand a seed set of packages according to a graph traversal specification.
    ///
    /// Returns the final set, including or excluding the original seeds depending on
    /// `traversal.exclude_self`. `None` traversal → seeds returned unchanged.
    fn expand_traversal(
        &self,
        seeds: FxHashSet<PackageNodeIndex>,
        traversal: Option<&GraphTraversal>,
    ) -> FxHashSet<PackageNodeIndex> {
        let Some(traversal) = traversal else {
            return seeds;
        };

        let mut reachable = FxHashSet::default();

        match traversal.direction {
            TraversalDirection::Dependencies => {
                self.bfs_outgoing(&seeds, &mut reachable);
            }
            TraversalDirection::Dependents => {
                self.bfs_incoming(&seeds, &mut reachable);
            }
            TraversalDirection::Both => {
                // Walk dependents first, then walk dependencies of ALL dependents found
                // (including the original seeds).
                // pnpm ref: <https://github.com/pnpm/pnpm/blob/491a84fb26fa716408bf6bd361680f6a450c61fc/workspace/filter-workspace-packages/src/index.ts#L265-L267>
                let mut dependents = FxHashSet::default();
                self.bfs_incoming(&seeds, &mut dependents);
                let all_dep_seeds: FxHashSet<_> =
                    seeds.iter().chain(dependents.iter()).copied().collect();
                self.bfs_outgoing(&all_dep_seeds, &mut reachable);
                reachable.extend(dependents);
            }
        }

        if traversal.exclude_self {
            for seed in &seeds {
                reachable.remove(seed);
            }
        } else {
            reachable.extend(seeds);
        }

        reachable
    }

    /// BFS along outgoing (dependency) edges from `seeds`, collecting all reachable nodes.
    ///
    /// Seeds are NOT added to `out`; the caller decides inclusion based on `exclude_self`.
    fn bfs_outgoing(
        &self,
        seeds: &FxHashSet<PackageNodeIndex>,
        out: &mut FxHashSet<PackageNodeIndex>,
    ) {
        let mut queue: Vec<PackageNodeIndex> = seeds.iter().copied().collect();
        while let Some(node) = queue.pop() {
            for edge in self.graph.edges(node) {
                let dep = edge.target();
                if out.insert(dep) {
                    queue.push(dep);
                }
            }
        }
    }

    /// BFS along incoming (dependent) edges from `seeds`, collecting all reachable nodes.
    ///
    /// Seeds are NOT added to `out`.
    fn bfs_incoming(
        &self,
        seeds: &FxHashSet<PackageNodeIndex>,
        out: &mut FxHashSet<PackageNodeIndex>,
    ) {
        let mut queue: Vec<PackageNodeIndex> = seeds.iter().copied().collect();
        while let Some(node) = queue.pop() {
            for edge in self.graph.edges_directed(node, Direction::Incoming) {
                let dependent = edge.source();
                if out.insert(dependent) {
                    queue.push(dependent);
                }
            }
        }
    }

    /// Build the induced subgraph of `selected` packages.
    ///
    /// Includes every node in `selected` and every original edge `(a, b)` where
    /// both `a` and `b` are in `selected`. Isolated nodes are also included.
    fn build_induced_subgraph(
        &self,
        selected: &FxHashSet<PackageNodeIndex>,
    ) -> DiGraphMap<PackageNodeIndex, ()> {
        let mut subgraph = DiGraphMap::new();
        for &pkg in selected {
            subgraph.add_node(pkg);
        }
        for edge in self.graph.edge_references() {
            let src = edge.source();
            let dst = edge.target();
            if selected.contains(&src) && selected.contains(&dst) {
                subgraph.add_edge(src, dst, ());
            }
        }
        subgraph
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use tempfile::TempDir;
    use vt_path::AbsolutePathBuf;

    use super::*;
    use crate::{discover_package_graph, package_filter::parse_filter};

    fn git(cwd: &AbsolutePath, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args)
            .current_dir(cwd.as_path())
            .output()
            .unwrap()
    }

    fn select(
        workspace: &AbsolutePath,
        filters: &[&str],
    ) -> Result<Vec<Str>, PackageQueryResolveError> {
        let graph = IndexedPackageGraph::index(discover_package_graph(workspace).unwrap());
        let filters = filters
            .iter()
            .map(|filter| parse_filter(filter, workspace).unwrap())
            .collect::<Vec<_>>();
        let query = PackageQuery::filters(Vec1::try_from_vec(filters).unwrap());
        let resolution = graph.resolve_query(&query)?;
        let mut names: Vec<Str> = resolution
            .package_subgraph
            .nodes()
            .map(|idx| graph.package_graph()[idx].package_json.name.clone())
            .collect();
        names.sort_unstable();
        Ok(names)
    }

    fn changed_since_error(workspace: &AbsolutePath, filter: &str) -> ChangedSinceError {
        match select(workspace, &[filter]) {
            Err(PackageQueryResolveError::ChangedSince { error, .. }) => error,
            other => panic!("expected a ChangedSince error, got {other:?}"),
        }
    }

    /// A git repository holding a pnpm workspace in `workspace_dir` (relative
    /// to the repository root): root `monorepo`, and `app` → `lib` → `utils`
    /// under `packages/`. `app` takes `react` from the default catalog.
    struct Repo {
        _temp_dir: TempDir,
        root: AbsolutePathBuf,
        workspace: AbsolutePathBuf,
    }

    impl Repo {
        fn new() -> Self {
            Self::with_workspace_dir("")
        }

        fn with_workspace_dir(workspace_dir: &str) -> Self {
            let temp_dir = TempDir::new().unwrap();
            let root = AbsolutePathBuf::new(temp_dir.path().canonicalize().unwrap()).unwrap();
            let workspace = root.join(workspace_dir);
            let repo = Self { _temp_dir: temp_dir, root, workspace };
            repo.git(&["-c", "init.defaultBranch=main", "init", "-q"]);
            repo.write(
                "pnpm-workspace.yaml",
                "packages:\n  - packages/*\ncatalog:\n  react: ^18.0.0\n  lodash: ^4.0.0\n",
            );
            repo.write("package.json", r#"{ "name": "monorepo" }"#);
            for (name, deps) in [
                ("app", r#""lib": "workspace:*", "react": "catalog:""#),
                ("lib", r#""utils": "workspace:*""#),
                ("utils", ""),
            ] {
                repo.write(
                    &vt_str::format!("packages/{name}/package.json"),
                    &vt_str::format!(r#"{{ "name": "{name}", "dependencies": {{ {deps} }} }}"#),
                );
                repo.write(&vt_str::format!("packages/{name}/src/index.js"), "export {}\n");
            }
            repo.commit("init");
            repo
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.workspace.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn git(&self, args: &[&str]) {
            let output = git(&self.root, args);
            assert!(output.status.success(), "git {args:?} failed: {output:?}");
        }

        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }

        fn select(&self, filters: &[&str]) -> Vec<Str> {
            select(&self.workspace, filters).unwrap()
        }
    }

    #[test]
    fn leaf_change_selects_leaf_and_dependents() {
        let repo = Repo::new();
        repo.write("packages/utils/src/index.js", "export const x = 1\n");
        repo.commit("change utils");

        assert_eq!(repo.select(&["[HEAD~1]"]), ["utils"]);
        assert_eq!(repo.select(&["...[HEAD~1]"]), ["app", "lib", "utils"]);
        assert_eq!(repo.select(&["...^[HEAD~1]"]), ["app", "lib"]);
        assert_eq!(repo.select(&["[HEAD~1]..."]), ["utils"]);
        assert!(repo.select(&["[HEAD]"]).is_empty());
    }

    #[test]
    fn middle_change_with_dependencies() {
        let repo = Repo::new();
        repo.write("packages/lib/src/index.js", "export const x = 1\n");
        repo.commit("change lib");

        assert_eq!(repo.select(&["[HEAD~1]..."]), ["lib", "utils"]);
        assert_eq!(repo.select(&["[HEAD~1]^..."]), ["utils"]);
        assert_eq!(repo.select(&["...[HEAD~1]..."]), ["app", "lib", "utils"]);
    }

    #[test]
    fn files_outside_every_package_belong_to_the_root() {
        let repo = Repo::new();
        repo.write("README.md", "hi\n");
        repo.write("packages/README.md", "hi\n");
        repo.commit("docs");

        assert_eq!(repo.select(&["[HEAD~1]"]), ["monorepo"]);
    }

    #[test]
    fn uncommitted_tracked_changes_count_and_untracked_files_do_not() {
        let repo = Repo::new();
        repo.write("packages/lib/src/index.js", "export const changed = 1\n");
        repo.write("packages/utils/src/untracked.js", "export {}\n");

        assert_eq!(repo.select(&["[HEAD]"]), ["lib"]);
    }

    #[test]
    fn moved_file_changes_both_packages() {
        let repo = Repo::new();
        repo.git(&["mv", "packages/lib/src/index.js", "packages/utils/src/moved.js"]);
        repo.commit("move");

        assert_eq!(repo.select(&["[HEAD~1]"]), ["lib", "utils"]);
    }

    #[test]
    fn compares_with_the_merge_base() {
        let repo = Repo::new();
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("packages/lib/src/index.js", "export const feature = 1\n");
        repo.commit("feature");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("packages/utils/src/index.js", "export const main = 1\n");
        repo.commit("main moved on");
        repo.git(&["checkout", "-q", "feature"]);

        // `utils` changed only on `main`, after `feature` forked from it.
        assert_eq!(repo.select(&["[main]"]), ["lib"]);
    }

    #[test]
    fn name_and_directory_narrow_the_changed_packages() {
        let repo = Repo::new();
        repo.write("packages/lib/src/index.js", "export const x = 1\n");
        repo.write("packages/utils/src/index.js", "export const x = 1\n");
        repo.commit("change lib and utils");

        assert_eq!(repo.select(&["lib[HEAD~1]"]), ["lib"]);
        assert!(repo.select(&["app[HEAD~1]"]).is_empty());
        assert_eq!(repo.select(&["{./packages/utils}[HEAD~1]"]), ["utils"]);
        assert_eq!(repo.select(&["{packages/*}[HEAD~1]"]), ["lib", "utils"]);
        assert_eq!(repo.select(&["...utils{packages/*}[HEAD~1]"]), ["app", "lib", "utils"]);
    }

    #[test]
    fn exclusion_removes_changed_packages() {
        let repo = Repo::new();
        repo.write("packages/utils/src/index.js", "export const x = 1\n");
        repo.commit("change utils");

        assert_eq!(repo.select(&["!...[HEAD~1]"]), ["monorepo"]);
        assert_eq!(repo.select(&["...[HEAD~1]", "!lib"]), ["app", "utils"]);
    }

    #[test]
    fn catalog_change_selects_the_packages_that_use_it() {
        let repo = Repo::new();
        repo.write(
            "pnpm-workspace.yaml",
            "packages:\n  - packages/*\ncatalog:\n  react: ^19.0.0\n  lodash: ^4.0.0\n",
        );
        repo.commit("bump react");

        // The root owns `pnpm-workspace.yaml`; `app` takes `react: catalog:`.
        assert_eq!(repo.select(&["[HEAD~1]"]), ["app", "monorepo"]);
        assert_eq!(repo.select(&["...{packages/*}[HEAD~1]"]), ["app"]);
    }

    #[test]
    fn catalog_change_to_an_unused_entry_selects_only_the_root() {
        let repo = Repo::new();
        repo.write(
            "pnpm-workspace.yaml",
            "packages:\n  - packages/*\ncatalog:\n  react: ^18.0.0\n  lodash: ^4.17.0\n",
        );
        repo.commit("bump lodash");

        assert_eq!(repo.select(&["[HEAD~1]"]), ["monorepo"]);
    }

    #[test]
    fn workspace_in_a_repository_subdirectory() {
        let repo = Repo::with_workspace_dir("js");
        fs::write(repo.root.join("outside.txt"), "outside the workspace\n").unwrap();
        repo.write("packages/utils/src/index.js", "export const x = 1\n");
        repo.write(
            "pnpm-workspace.yaml",
            "packages:\n  - packages/*\ncatalog:\n  react: ^19.0.0\n  lodash: ^4.0.0\n",
        );
        repo.commit("change");

        assert_eq!(repo.select(&["[HEAD~1]"]), ["app", "monorepo", "utils"]);

        fs::write(repo.root.join("outside.txt"), "changed again\n").unwrap();
        repo.commit("change outside the workspace");
        assert!(repo.select(&["[HEAD~1]"]).is_empty());
    }

    #[test]
    fn unknown_revision_is_an_error() {
        let repo = Repo::new();
        let error = changed_since_error(&repo.workspace, "...[no-such-branch]");
        assert!(
            matches!(&error, ChangedSinceError::UnknownRevision { since } if since == "no-such-branch"),
            "{error:?}"
        );
    }

    #[test]
    fn option_like_revision_is_an_error() {
        let repo = Repo::new();
        let error = changed_since_error(&repo.workspace, "[--output=x]");
        assert!(matches!(&error, ChangedSinceError::InvalidRevision { .. }), "{error:?}");
    }

    #[test]
    fn unrelated_history_is_an_error() {
        let repo = Repo::new();
        repo.git(&["checkout", "-q", "--orphan", "unrelated"]);
        repo.commit("unrelated root");

        let error = changed_since_error(&repo.workspace, "[main]");
        assert!(
            matches!(&error, ChangedSinceError::NoMergeBase { since, shallow: false } if since == "main"),
            "{error:?}"
        );
    }

    #[test]
    fn not_a_repository_is_an_error() {
        let temp_dir = TempDir::new().unwrap();
        let workspace = AbsolutePathBuf::new(temp_dir.path().canonicalize().unwrap()).unwrap();
        assert!(
            !git(&workspace, &["rev-parse", "--git-dir"]).status.success(),
            "the system temp dir must not be inside a git repository"
        );
        fs::write(workspace.join("package.json"), r#"{ "name": "solo" }"#).unwrap();

        let error = changed_since_error(&workspace, "[HEAD~1]");
        assert!(matches!(&error, ChangedSinceError::NotARepository { .. }), "{error:?}");
    }
}
