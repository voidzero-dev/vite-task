use std::sync::Arc;

use petgraph::graph::DiGraph;
use rustc_hash::FxHashMap;
use vec1::smallvec_v1::SmallVec1;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_workspace::{DependencyType, PackageInfo, PackageIx, PackageNodeIndex};

/// Package graph with additional hash maps for quick task lookup
#[derive(Debug)]
pub struct IndexedPackageGraph {
    graph: DiGraph<PackageInfo, DependencyType, PackageIx>,

    /// Grouping package indices by their package names.
    /// Due to rare but possible name conflicts in monorepos, we use `SmallVec1` to store multiple dirs for same name.
    indices_by_name: FxHashMap<Str, SmallVec1<[PackageNodeIndex; 1]>>,

    /// package indices by their absolute paths for quick lookup based on cwd
    indices_by_path: FxHashMap<Arc<AbsolutePath>, PackageNodeIndex>,
}

impl IndexedPackageGraph {
    pub fn index(package_graph: DiGraph<PackageInfo, DependencyType, PackageIx>) -> Self {
        // Index package indices by their absolute paths for quick lookup based on cwd
        let indices_by_path: FxHashMap<Arc<AbsolutePath>, PackageNodeIndex> = package_graph
            .node_indices()
            .map(|package_index| {
                let absolute_path: Arc<AbsolutePath> =
                    Arc::clone(&package_graph[package_index].absolute_path);
                (absolute_path, package_index)
            })
            .collect();

        // Grouping package indices by their package names.
        let mut indices_by_name: FxHashMap<Str, SmallVec1<[PackageNodeIndex; 1]>> =
            FxHashMap::default();
        for package_index in package_graph.node_indices() {
            let package = &package_graph[package_index];
            indices_by_name
                .entry(package.package_json.name.clone())
                .and_modify(|indices| indices.push(package_index))
                .or_insert_with(|| SmallVec1::new(package_index));
        }
        Self { graph: package_graph, indices_by_name, indices_by_path }
    }

    /// Get package index from a given current working directory by traversing up the directory tree.
    pub fn get_package_index_from_cwd(&self, cwd: &AbsolutePath) -> Option<PackageNodeIndex> {
        let mut cur_path = cwd;
        loop {
            if let Some(package_index) = self.indices_by_path.get(cur_path) {
                return Some(*package_index);
            }
            cur_path = cur_path.parent()?;
        }
    }

    /// Get package indices by package name.
    pub fn get_package_indices_by_name(
        &self,
        package_name: &Str,
    ) -> Option<&SmallVec1<[PackageNodeIndex; 1]>> {
        self.indices_by_name.get(package_name)
    }

    pub const fn package_graph(&self) -> &DiGraph<PackageInfo, DependencyType, PackageIx> {
        &self.graph
    }
}
