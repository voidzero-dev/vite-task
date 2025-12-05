use bincode::{Decode, Encode};
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use serde::Serialize;
use vite_path::RelativePathBuf;
use vite_str::Str;

use super::ResolvedTask;
use crate::{
    Error,
    collections::{HashMap, HashSet},
};

#[derive(Debug, Clone, Copy)]
pub enum TaskDependencyType {
    /// The dependency is explicit defined by user in `dependsOn`.
    Explicit,
    /// The dependency is added due to topological ordering based on package dependencies.
    #[expect(unused)]
    Topological,
}

/// Uniquely identifies a task group, which is a script in `package.json`, or an entry in `vite-task.json`.
///
/// A task group can be parsed into one task or multiple tasks split by `&&`
#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Clone, Encode, Decode, Serialize)]
pub struct TaskGroupId {
    /// For user defined task, this is the name of the script or the entry in `vite-task.json`.
    /// For built-in tasks, this is the command name.
    pub task_group_name: Str,

    /// Whether this is a built-in task like `vite lint`.
    pub is_builtin: bool,

    /// The path to the config file that defines this task group, relative to the workspace root.
    ///
    /// For built-in tasks, there's no config file. This value will be the cwd,
    /// so that same built-in command running under different folders will be treated as different tasks.
    pub config_path: RelativePathBuf,
}

/// Uniquely identifies a task.
///
/// Similar to `TaskName` but replaces `package_name` with `config_path` to ensure uniqueness.
#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Clone, Encode, Decode, Serialize)]
pub struct TaskId {
    pub task_group_id: TaskGroupId,

    /// The index of the subcommand in a parsed command (`echo A && echo B`).
    /// None if the task is the last command. Only the last command can be filtered out by user task requests.
    pub subcommand_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct TaskGraphNode {
    task: ResolvedTask,
    dependeny_types_by_task_id: HashMap<TaskId, TaskDependencyType>,
}

#[derive(Default, Debug, Clone)]
pub struct TaskGraphBuilder {
    pub(crate) task_nodes_by_id: HashMap<TaskId, TaskGraphNode>,
}

impl TaskGraphBuilder {
    pub(crate) fn add_task_with_explicit_deps(
        &mut self,
        task: ResolvedTask,
        dep_ids: HashSet<TaskId>,
    ) -> Result<(), Error> {
        let task_node = TaskGraphNode {
            task,
            dependeny_types_by_task_id: dep_ids
                .into_iter()
                .map(|dep_id| (dep_id, TaskDependencyType::Explicit))
                .collect(),
        };
        if let Some(old_task_node) = self.task_nodes_by_id.insert(task_node.task.id(), task_node) {
            return Err(Error::DuplicatedTask(old_task_node.task.display_name()));
        }
        Ok(())
    }

    /// Build the complete task graph including all tasks and their dependencies
    pub(crate) fn build_complete_graph(
        self,
    ) -> Result<StableDiGraph<ResolvedTask, TaskDependencyType>, Error> {
        let mut task_graph = StableDiGraph::<ResolvedTask, TaskDependencyType>::new();
        let mut node_indices_by_task_ids = HashMap::<TaskId, NodeIndex>::new();

        // Add all tasks to the graph
        for (task_id, task_node) in &self.task_nodes_by_id {
            let node_index = task_graph.add_node(task_node.task.clone()); // TODO(perf): remove clone here
            node_indices_by_task_ids.insert(task_id.clone(), node_index);
        }

        // Add edges from explicit dependencies
        for (task_id, task_node) in self.task_nodes_by_id {
            let current_task_index = node_indices_by_task_ids[&task_id];
            for (dep_id, dep_type) in task_node.dependeny_types_by_task_id {
                let Some(&dep_index) = node_indices_by_task_ids.get(&dep_id) else {
                    return Err(Error::TaskDependencyNotFound {
                        name: dep_id.task_group_id.task_group_name.clone(),
                        package_path: dep_id.task_group_id.config_path.clone(),
                    });
                };
                task_graph.add_edge(current_task_index, dep_index, dep_type);
            }
        }

        Ok(task_graph)
    }
}
