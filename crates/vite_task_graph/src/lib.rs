mod config;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use petgraph::prelude::StableDiGraph;
use vite_path::RelativePath;
use vite_str::Str;
use vite_workspace::WorkspaceRoot;

/// The type of a desk dependency, explaining why it's introduced.
#[derive(Debug, Clone, Copy)]
pub enum TaskDependencyType {
    /// The dependency is explicit defined by user in `dependsOn`.
    /// If a dependency is both explicit and topological, Explicit takes precedence.
    Explicit,
    /// The dependency is added due to topological ordering based on package dependencies.
    Topological,
}

/// Full task graph of a workspace.
///
/// It's immutable after created. The task nodes contain resolved task configurations and their dependencies.
/// External factors (e.g. additional args from cli, current working directory, environmental variables) are not stored here.
pub struct TaskGraph {
    // graph: StableDiGraph,
}

pub struct TaskNode {}

impl TaskGraph {
    pub fn load(workspace_root: WorkspaceRoot<'_>, load_user_config: ()) -> Self {
        todo!()
    }
}
