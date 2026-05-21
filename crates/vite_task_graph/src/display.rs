//! Structs for printing packages and tasks in a human-readable way. It's used in error messages and CLI outputs.

use std::{fmt::Display, sync::Arc};

use serde::Serialize;
use vite_path::AbsolutePath;
use vite_str::Str;

use crate::{IndexedTaskGraph, TaskNodeIndex};

/// struct for printing a task in a human-readable way.
#[derive(Debug, Clone, Serialize)]
pub struct TaskDisplay {
    pub package_name: Str,
    pub task_name: Str,
    pub package_path: Arc<AbsolutePath>,
}

impl Display for TaskDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only include package name and # separator if package name is not empty
        if self.package_name.is_empty() {
            write!(f, "{}", self.task_name)
        } else {
            write!(f, "{}#{}", self.package_name, self.task_name)
        }
    }
}

/// A task with its display info and command, for listing purposes.
#[derive(Debug)]
pub struct TaskListEntry {
    pub task_display: TaskDisplay,
    pub command: Str,
}

impl IndexedTaskGraph {
    /// Get human-readable display for a task node.
    #[must_use]
    pub fn display_task(&self, task_index: TaskNodeIndex) -> TaskDisplay {
        self.task_graph()[task_index].task_display.clone()
    }

    /// Returns all tasks as a flat list.
    #[must_use]
    pub fn list_tasks(&self) -> Vec<TaskListEntry> {
        self.task_graph()
            .node_indices()
            .map(|idx| {
                let node = &self.task_graph()[idx];
                TaskListEntry {
                    task_display: node.task_display.clone(),
                    command: format_command_for_task_list(&node.resolved_config.commands),
                }
            })
            .collect()
    }
}

// Display-only formatting for task list/selector descriptions. Execution planning keeps
// command arrays structured and must not depend on this joined string.
fn format_command_for_task_list(commands: &Arc<[Str]>) -> Str {
    if commands.len() == 1 {
        commands[0].clone()
    } else {
        let mut display = Str::default();
        for (index, command) in commands.iter().enumerate() {
            if index > 0 {
                display.push_str(" && ");
            }
            display.push_str(command.as_str());
        }
        display
    }
}
