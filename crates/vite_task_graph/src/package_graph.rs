use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use petgraph::graph::DiGraph;
use vec1::smallvec_v1::SmallVec1;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_workspace::{DependencyType, PackageInfo, PackageIx, PackageNodeIndex};

/// Package graph with additional HashMaps for quick task lookup
#[derive(Debug)]
pub struct IndexedPackageGraph {
    package_graph: DiGraph<PackageInfo, DependencyType, PackageIx>,

    /// Grouping package indices by their package names.
    /// Due to rare but possible name conflicts in monorepos, we use `SmallVec1` to store multiple dirs for same name.
    package_indices_by_name: HashMap<Str, SmallVec1<[PackageNodeIndex; 1]>>,

    /// package indices by their absolute paths for quick lookup based on cwd
    package_indices_by_paths: HashMap<Arc<AbsolutePath>, PackageNodeIndex>,
}

impl IndexedPackageGraph {
    pub fn index(package_graph: DiGraph<PackageInfo, DependencyType, PackageIx>) -> Self {
        // Index package indices by their absolute paths for quick lookup based on cwd
        let package_indices_by_paths = package_graph
            .node_indices()
            .map(|package_index| {
                let absolute_path: Arc<AbsolutePath> =
                    Arc::clone(&package_graph[package_index].absolute_path);
                (absolute_path, package_index)
            })
            .collect::<HashMap<Arc<AbsolutePath>, PackageNodeIndex>>();

        // Grouping package indices by their package names.
        let mut package_indices_by_name: HashMap<Str, SmallVec1<[PackageNodeIndex; 1]>> =
            HashMap::new();
        for package_index in package_graph.node_indices() {
            let package = &package_graph[package_index];
            match package_indices_by_name.entry(package.package_json.name.clone()) {
                Entry::Vacant(vacant) => {
                    vacant.insert(SmallVec1::new(package_index));
                }
                Entry::Occupied(occupied) => {
                    occupied.into_mut().push(package_index);
                }
            }
        }
        Self { package_graph, package_indices_by_name, package_indices_by_paths }
    }

    /// Get package index from a given current working directory by traversing up the directory tree.
    pub fn get_package_index_from_cwd(&self, cwd: &AbsolutePath) -> Option<PackageNodeIndex> {
        let mut cur_path = cwd;
        loop {
            if let Some(package_index) = self.package_indices_by_paths.get(cur_path) {
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
        self.package_indices_by_name.get(package_name)
    }

    pub fn package_graph(&self) -> &DiGraph<PackageInfo, DependencyType, PackageIx> {
        &self.package_graph
    }
}
