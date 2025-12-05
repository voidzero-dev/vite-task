mod builder;
pub mod config;
pub mod loader;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use config::{ResolvedUserTaskConfig, UserConfigFile};
use petgraph::graph::DiGraph;
use vite_path::{AbsolutePath, RelativePath};
use vite_str::Str;
use vite_workspace::WorkspaceRoot;

/// The type of a desk dependency, explaining why it's introduced.
#[derive(Debug, Clone, Copy)]
pub enum TaskDependencyType {
    /// The dependency is explicitly declared by user in `dependsOn`.
    /// If a dependency is both explicit and topological, `TaskDependencyType::Explicit` takes precedenc
    Explicit,
    /// The dependency is added due to topological ordering based on package dependencies.
    Topological,
}

/// Uniquely identifies a task, by its name and the path where it's defined.
///
/// For user defined tasks, the path is where the package dir.
/// We don't use package names because multiple packages can have the same name in a monorepo.
///
/// For synthesized tasks, the path is the cwd where the command is run.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct TaskId {
    /// For user defined tasks, this is the name of the script or the entry in `vite-task.json`.
    ///
    /// For synthesized tasks, this is the program.
    pub task_name: Str,

    /// For user defined tasks, this is the path where the task is defined.
    ///
    /// For synthesized tasks, there's no config file. This value will be the cwd,
    /// so that same synthesized command running under different folders will be treated as different tasks,
    ///
    /// Note that this is not always the cwd where the command is run, which is stored in `ResolvedUserTaskConfig`.
    pub task_path: Arc<AbsolutePath>,
}

/// A node in the task graph, representing a task with its resolved configuration.
#[derive(Debug)]
pub struct TaskNode {
    /// The unique id of this task
    pub task_id: TaskId,

    /// The name of the package where this task is defined.
    /// It's used for matching task specifiers ('packageName#taskName')
    ///
    /// - If package.json doesn't have a name field, this will be Some("").
    /// - For synthesized tasks, this will be None, so that they won't be matched by any task specifiers.
    pub package_name: Option<Str>,

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
    // ConfigLoadError(loader::ConfigLoadError),
}

/// Full task graph of a workspace.
///
/// It's immutable after created. The task nodes contain resolved task configurations and their dependencies.
/// External factors (e.g. additional args from cli, current working directory, environmental variables) are not stored here.
pub struct TaskGraph {
    graph: DiGraph<TaskNode, TaskDependencyType>,
}

impl TaskGraph {
    /// Load the task graph from a discovered workspace using the provided config loader.
    pub fn load(
        workspace_root: WorkspaceRoot<'_>,
        config_loader: impl loader::UserConfigLoader,
    ) -> Result<Self, TaskGraphLoadError> {
        let package_graph = vite_workspace::load_package_graph(&workspace_root)?;

        for package in package_graph.node_weights() {}
        todo!()
    }
}
