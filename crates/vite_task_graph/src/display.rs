//! Structs for printing packages and tasks in a human-readable way. It's used in error messages and CLI outputs.

use std::{fmt::Display, sync::Arc};

use vite_path::AbsolutePath;
use vite_str::Str;

use crate::{IndexedTaskGraph, TaskNodeIndex};

/// struct for printing a task in a human-readable way.
#[derive(Debug, Clone)]
pub struct TaskDisplay {
    pub package_name: Str,
    pub task_name: Str,
    pub package_path: Arc<AbsolutePath>,
}

impl Display for TaskDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: give an option to display package path as well
        write!(f, "{}#{}", self.package_name, self.task_name,)
    }
}

impl IndexedTaskGraph {
    /// Get human-readable display for a task node.
    pub fn display_task(&self, task_index: TaskNodeIndex) -> TaskDisplay {
        let task_node = &self.task_graph()[task_index];
        let package = &self.indexed_package_graph.package_graph()[task_node.task_id.package_index];
        TaskDisplay {
            package_name: package.package_json.name.clone(),
            task_name: task_node.task_id.task_name.clone(),
            package_path: Arc::clone(&package.absolute_path),
        }
    }
}
