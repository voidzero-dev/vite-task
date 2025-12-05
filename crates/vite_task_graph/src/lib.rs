pub mod config;
pub mod loader;

use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use config::{ResolvedUserTaskConfig, UserConfigFile};
use petgraph::{
    graph::{DiGraph, NodeIndex},
    visit::depth_first_search,
};
use serde::Serialize;
use vec1::smallvec_v1::SmallVec1;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use vite_workspace::{
    DependencyType, PackageInfo, PackageIx, PackageNodeIndex, WorkspaceRoot, package,
};

/// The type of a desk dependency, explaining why it's introduced.
#[derive(Debug, Clone, Copy, Serialize)]
pub enum TaskDependencyType {
    /// The dependency is explicitly declared by user in `dependsOn`.
    /// If a dependency is both explicit and topological, `TaskDependencyType::Explicit` takes precedenc
    Explicit,
    /// The dependency is added due to topological ordering based on package dependencies.
    Topological,
}

/// Uniquely identifies a task, by its name and the path where it's defined.
#[derive(Debug, PartialEq, Eq, Hash, Clone, PartialOrd, Ord)]
pub struct TaskId {
    /// This is the index of the package where the task is defined.
    ///
    /// `package_index` is declared from `task_name` to make the `PartialOrd` implementation group tasks in same packages together.
    pub package_index: PackageNodeIndex,

    /// For user defined tasks, this is the name of the script or the entry in `vite-task.json`.
    ///
    /// For synthesized tasks, this is the program.
    pub task_name: Str,
}

/// A node in the task graph, representing a task with its resolved configuration.
#[derive(Debug)]
pub struct TaskNode {
    /// The unique id of this task
    pub task_id: TaskId,

    /// The name of the package where this task is defined.
    /// It's used for matching task specifiers ('packageName#taskName')
    ///
    /// If package.json doesn't have a name field, this will be "", so the task can be matched by `#taskName`.
    pub package_name: Str,

    /// The resolved configuration of this task.
    ///
    /// This contains information affecting how the task is spawn,
    /// whereas `task_id` and `package_name` are for looking up the task.
    ///
    /// However, it does not contain external factors like additional args from cli and env vars.
    pub resolved_config: ResolvedUserTaskConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskGraphLoadError {
    #[error("Failed to load package graph: {0}")]
    PackageGraphLoadError(#[from] vite_workspace::Error),

    #[error("Failed to load task config file for package at {package_path:?}: {error}")]
    ConfigLoadError {
        #[source]
        error: anyhow::Error,
        package_path: Arc<AbsolutePath>,
    },

    #[error("Failed to resolve task config for task {0}#{1}: {2}", package_name, task_name, error)]
    ResolveConfigError {
        #[source]
        error: crate::config::ResolveTaskError,
        package_name: Str,
        task_name: Str,
    },

    #[error("Failed to lookup dependency '{specifier}' of task {0} at {1:?}: {error}", origin_task_id.task_name, origin_task_id.task_name)]
    DependencySpecifierLookupError {
        #[source]
        error: SpecifierLookupError,
        specifier: Str,
        // Where the dependency specifier is defined
        origin_task_id: TaskId,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SpecifierLookupError {
    #[error("Package '{package_name}' is ambiguous among multiple packages: {package_paths:?}")]
    AmbiguousPackageName { package_name: Str, package_paths: Box<[RelativePathBuf]> },

    #[error("Package '{package_name}' not found")]
    PackageNameNotFound { package_name: Str },

    #[error("Task '{task_name}' not found in package {package_name}")]
    TaskNameNotFound { package_name: Str, task_name: Str },
}

/// Full task graph of a workspace.
///
/// It's immutable after created. The task nodes contain resolved task configurations and their dependencies.
/// External factors (e.g. additional args from cli, current working directory, environmental variables) are not stored here.
pub struct TaskGraph {
    graph: DiGraph<TaskNode, TaskDependencyType>,

    package_graph: Arc<DiGraph<PackageInfo, DependencyType, PackageIx>>,

    /// Grouping package indices by their package names.
    /// Due to rare but possible name conflicts in monorepos, we use `SmallVec1` to store multiple dirs for same name.
    package_indices_by_name: HashMap<Str, SmallVec1<[PackageNodeIndex; 1]>>,

    /// task indices by task id for quick lookup
    node_indices_by_task_id: HashMap<TaskId, NodeIndex>,
}

impl TaskGraph {
    /// Load the task graph from a discovered workspace using the provided config loader.
    pub async fn load(
        workspace_root: WorkspaceRoot<'_>,
        config_loader: impl loader::UserConfigLoader,
    ) -> Result<Self, TaskGraphLoadError> {
        let mut task_graph = DiGraph::<TaskNode, TaskDependencyType>::new();

        let package_graph = vite_workspace::load_package_graph(&workspace_root)?;

        // Record dependency specifiers for each task node to add explicit dependencies later
        let mut dependency_specifiers_with_task_node_indices: Vec<(Arc<[Str]>, NodeIndex)> =
            Vec::new();

        // Load task nodes into `task_graph`
        for package_index in package_graph.node_indices() {
            let package = &package_graph[package_index];
            let package_dir: Arc<AbsolutePath> = workspace_root.path.join(&package.path).into();

            // Collect package.json scripts into a mutable map for draining lookup.
            let mut package_json_scripts: HashMap<&str, &str> = package
                .package_json
                .scripts
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();

            // Load vite.config.* for the package
            let user_config: UserConfigFile =
                config_loader.load_user_config_file(&package_dir).await.map_err(|error| {
                    TaskGraphLoadError::ConfigLoadError { error, package_path: package_dir.clone() }
                })?;

            for (task_name, task_user_config) in user_config.tasks {
                // For each task defined in vite.config.*, look up the corresponding package.json script (if any)
                let package_json_script = package_json_scripts.remove(task_name.as_str());

                let task_id = TaskId { task_name: task_name.clone(), package_index };

                let dependency_specifiers = Arc::clone(&task_user_config.depends_on);

                // Resolve the task configuration combining vite.config.* and package.json script
                let resolved_config = ResolvedUserTaskConfig::resolve(
                    task_user_config,
                    &package_dir,
                    package_json_script,
                )
                .map_err(|err| TaskGraphLoadError::ResolveConfigError {
                    error: err,
                    package_name: package.package_json.name.clone(),
                    task_name: task_name.clone(),
                })?;

                let task_node = TaskNode {
                    task_id,
                    package_name: package.package_json.name.clone(),
                    resolved_config,
                };

                let node_index = task_graph.add_node(task_node);
                dependency_specifiers_with_task_node_indices
                    .push((dependency_specifiers, node_index));
            }

            // For remaining package.json scripts not defined in vite.config.*, create tasks with default config
            for (script_name, package_json_script) in package_json_scripts.drain() {
                let task_id = TaskId { task_name: Str::from(script_name), package_index };
                let resolved_config = ResolvedUserTaskConfig::resolve_package_json_script(
                    &package_dir,
                    package_json_script,
                );
                task_graph.add_node(TaskNode {
                    task_id,
                    package_name: package.package_json.name.clone(),
                    resolved_config,
                });
            }
        }

        // index tasks by ids
        let mut node_indices_by_task_id: HashMap<TaskId, NodeIndex> =
            HashMap::with_capacity(task_graph.node_count());
        for node_index in task_graph.node_indices() {
            let task_node = &task_graph[node_index];

            let existing_entry =
                node_indices_by_task_id.insert(task_node.task_id.clone(), node_index);
            if existing_entry.is_some() {
                // This should never happen as we enforce unique task ids when adding nodes.
                panic!("Duplicate task id found: {:?}", task_node.task_id);
            }
        }

        // Grouping package dirs by their package names.
        let mut package_dirs_by_name: HashMap<Str, SmallVec1<[PackageNodeIndex; 1]>> =
            HashMap::new();
        for package_index in package_graph.node_indices() {
            let package = &package_graph[package_index];
            match package_dirs_by_name.entry(package.package_json.name.clone()) {
                Entry::Vacant(vacant) => {
                    vacant.insert(SmallVec1::new(package_index));
                }
                Entry::Occupied(occupied) => {
                    occupied.into_mut().push(package_index);
                }
            }
        }

        let package_graph = Arc::new(package_graph);
        // Construct `Self` with task_graph with all task nodes ready and indexed, but no edges.
        let mut me = Self {
            graph: task_graph,
            package_graph: Arc::clone(&package_graph),
            node_indices_by_task_id,
            package_indices_by_name: package_dirs_by_name,
        };

        // Add explicit dependencies
        for (dependency_specifiers, from_node_index) in dependency_specifiers_with_task_node_indices
        {
            let from_task_id = me.graph[from_node_index].task_id.clone();

            for specifier in dependency_specifiers.iter().cloned() {
                let to_node_index = me
                    .get_task_index_by_specifier(&specifier, from_task_id.package_index)
                    .map_err(|error| TaskGraphLoadError::DependencySpecifierLookupError {
                        error,
                        specifier,
                        origin_task_id: from_task_id.clone(),
                    })?;
                me.graph.update_edge(from_node_index, to_node_index, TaskDependencyType::Explicit);
            }
        }

        // Add topological dependencies based on package dependencies
        // for (package_index, task_node_index) in package_indices_with_task_node_indices {
        //     // For every task,
        //     // DFS starting from the containing package
        //     depth_first_search(&package_graph, Some(package_index), |event| {});
        // }

        Ok(me)
    }

    /// Lookup the node index of a task by its specifier.
    ///
    /// The specifier can be either 'packageName#taskName' or just 'taskName' (in which case the task in the origin package is looked up).
    fn get_task_index_by_specifier(
        &self,
        specifier: &str,
        package_origin: PackageNodeIndex,
    ) -> Result<NodeIndex, SpecifierLookupError> {
        let (package_index, task_name): (PackageNodeIndex, Str) =
            if let Some((package_name, task_name)) = specifier.rsplit_once('#') {
                // Lookup package path by the package name from '#'
                let Some(package_indices) = self.package_indices_by_name.get(package_name) else {
                    return Err(SpecifierLookupError::PackageNameNotFound {
                        package_name: package_name.into(),
                    });
                };
                if package_indices.len() > 1 {
                    return Err(SpecifierLookupError::AmbiguousPackageName {
                        package_name: package_name.into(),
                        package_paths: package_indices
                            .iter()
                            .map(|package_index| self.package_graph[*package_index].path.clone())
                            .collect(),
                    });
                };
                (*package_indices.first(), task_name.into())
            } else {
                // No '#', so the specifier only contains task name, look up in the origin path package
                (package_origin, specifier.into())
            };
        let task_id = TaskId { task_name, package_index };
        let Some(node_index) = self.node_indices_by_task_id.get(&task_id) else {
            return Err(SpecifierLookupError::TaskNameNotFound {
                package_name: self.package_graph[package_index].package_json.name.clone(),
                task_name: task_id.task_name.clone(),
            });
        };
        Ok(*node_index)
    }

    /// Create a stable json representation of the task graph for snapshot testing.
    ///
    /// All paths are relative to `base_dir`.
    pub fn snapshot(&self, base_dir: &AbsolutePath) -> serde_json::Value {
        use vite_path::RelativePathBuf;

        #[derive(serde::Serialize, PartialEq, PartialOrd, Eq, Ord)]
        struct TaskIdSnapshot {
            package_dir: RelativePathBuf,
            task_name: Str,
        }
        impl TaskIdSnapshot {
            fn from_task_id(
                task_id: &TaskId,
                package_graph: &DiGraph<PackageInfo, DependencyType, PackageIx>,
            ) -> Self {
                Self {
                    task_name: task_id.task_name.clone(),
                    package_dir: package_graph[task_id.package_index].path.clone(),
                }
            }
        }

        #[derive(serde::Serialize)]
        struct TaskNodeSnapshot {
            id: TaskIdSnapshot,
            command: Str,
            cwd: RelativePathBuf,
            depends_on: Vec<(TaskIdSnapshot, TaskDependencyType)>,
        }

        let mut node_snapshots = Vec::<TaskNodeSnapshot>::with_capacity(self.graph.node_count());
        for a in self.graph.node_indices() {
            let node = &self.graph[a];
            let mut depends_on: Vec<(TaskIdSnapshot, TaskDependencyType)> = self
                .graph
                .edges_directed(a, petgraph::Direction::Outgoing)
                .map(|edge| {
                    use petgraph::visit::EdgeRef as _;
                    let target_node = &self.graph[edge.target()];
                    (
                        TaskIdSnapshot::from_task_id(&target_node.task_id, &self.package_graph),
                        *edge.weight(),
                    )
                })
                .collect();
            depends_on.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            node_snapshots.push(TaskNodeSnapshot {
                id: TaskIdSnapshot::from_task_id(&node.task_id, &self.package_graph),
                command: node.resolved_config.command.clone(),
                cwd: node.resolved_config.cwd.strip_prefix(base_dir).unwrap().unwrap(),
                depends_on,
            });
        }
        node_snapshots.sort_unstable_by(|a, b| a.id.cmp(&b.id));

        serde_json::to_value(&node_snapshots).unwrap()
    }
}
