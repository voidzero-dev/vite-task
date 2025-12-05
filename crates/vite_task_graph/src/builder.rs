use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use petgraph::{
    graph::DiGraph,
    stable_graph::{NodeIndex, StableDiGraph},
};
use smallvec::SmallVec;
use vite_path::AbsolutePath;
use vite_str::Str;

use crate::{ResolvedUserTaskConfig, TaskDependencyType, TaskId, TaskNode};

#[derive(Debug)]
struct TaskConfigWithDependencies {
    task_config: ResolvedUserTaskConfig,
    dependency_specifiers: Arc<[Str]>,
}

#[derive(Default, Debug)]
pub struct TaskGraphBuilder {
    /// Grouping task configs and dependency specifiers by their TaskId
    resolved_config_by_task_id: HashMap<TaskId, TaskConfigWithDependencies>,

    /// Grouping package dirs by their package names.
    /// Due to rare but possible name conflicts in monorepos, we use `SmallVec` to store multiple dirs for same name.
    package_dirs_by_name: HashMap<Str, SmallVec<Arc<AbsolutePath>, 1>>,
}

pub struct TaskDependencyNotFound {}

/// The built and indexed task graph.
pub struct IndexedTaskGraph {
    pub task_graph: StableDiGraph<TaskNode, TaskDependencyType>,
    /// Grouping package dirs by their package names.
    /// Due to rare but possible name conflicts in monorepos, we use `SmallVec` to store multiple dirs for same name.
    pub package_dirs_by_name: HashMap<Str, SmallVec<Arc<AbsolutePath>, 1>>,
}

impl TaskGraphBuilder {
    /// Add a task to the builder.
    ///
    /// # Panics
    /// Panics if a task node with the same `TaskId` was already added in the builder.
    pub fn add_task(&mut self, task_node: TaskNode, dependency_specifiers: &Arc<[Str]>) {
        match self.resolved_config_by_task_id.entry(task_node.task_id) {
            Entry::Vacant(vacant) => {
                vacant.insert(TaskConfigWithDependencies {
                    task_config: task_node.resolved_config,
                    dependency_specifiers: Arc::clone(&dependency_specifiers),
                });
            }
            Entry::Occupied(occupied) => {
                panic!("Task with id {:?} was already added: {:?}", occupied.key(), occupied.get(),);
            }
        }
        // self.package_dirs_by_name
        //     .entry(task_node.package_name.clone())
        //     .or_default()
        //     .push(Arc::clone(&task_node.package_dir));
    }

    /// Build the complete task graph with tasks connected to their explict dependencies, and return it along with package_dirs_by_name.
    pub(crate) fn build(
        self,
    ) -> Result<DiGraph<TaskNode, TaskDependencyType>, TaskDependencyNotFound> {
        todo!()
        // let mut task_graph = DiGraph::<TaskNode, TaskDependencyType>::new();

        // let mut node_indices_by_task_ids = HashMap::<TaskId, NodeIndex>::new();

        // // Add all tasks to the graph
        // for (task_id, task_node) in self.resolved_config_by_task_id {
        //     let node_index = task_graph.add_node(task_node.task.clone());
        //     node_indices_by_task_ids.insert(task_id.clone(), node_index);
        // }

        // // Add edges from explicit dependencies
        // for (task_id, task_node) in self.resolved_config_by_task_id {
        //     let current_task_index = node_indices_by_task_ids[&task_id];
        //     for (dep_id, dep_type) in task_node.dependeny_types_by_task_id {
        //         let Some(&dep_index) = node_indices_by_task_ids.get(&dep_id) else {
        //             return Err(Error::TaskDependencyNotFound {
        //                 name: dep_id.task_group_id.task_group_name.clone(),
        //                 package_path: dep_id.task_group_id.config_path.clone(),
        //             });
        //         };
        //         task_graph.add_edge(current_task_index, dep_index, dep_type);
        //     }
        // }

        // Ok(task_graph)
    }
}
