use std::{
    collections::HashMap, env::JoinPathsError, ffi::OsStr, fmt::Display, ops::Range, sync::Arc,
};

use vite_path::AbsolutePath;
use vite_task_graph::{IndexedTaskGraph, TaskNodeIndex, display::TaskDispay};

use crate::{PlanCallbacks, path_env::prepend_path_env};

#[derive(Debug, thiserror::Error)]
#[error(
    "Detected a cycle in task call stack, from the {0}th frame to the end", cycle_start + 1
)]
pub struct TaskCycleError {
    /// The index in `task_call_stack` where the cycle starts
    ///
    /// The cycle ends at the end of `task_call_stack`.
    cycle_start: usize,
}

/// The context for planning an execution from a task.
#[derive(Debug)]
pub struct PlanContext<'a> {
    /// The current working directory.
    cwd: Arc<AbsolutePath>,

    /// The environment variables for the current execution context.
    envs: HashMap<Arc<OsStr>, Arc<OsStr>>,

    /// The callbacks for loading task graphs and parsing commands.
    callbacks: &'a mut (dyn PlanCallbacks + 'a),

    /// The current call stack of task index nodes being planned.
    task_call_stack: Vec<(TaskNodeIndex, Range<usize>)>,

    indexed_task_graph: &'a IndexedTaskGraph,
}

/// A human-readable frame in the task call stack.
#[derive(Debug, Clone)]
pub struct TaskCallStackFrameDisplay {
    pub task_display: TaskDispay,
    pub command_span: Range<usize>,
}

impl Display for TaskCallStackFrameDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: display command_span
        write!(f, "{}", self.task_display)
    }
}

/// A human-readable display of the task call stack.
#[derive(Debug, Clone)]
pub struct TaskCallStackDisplay {
    frames: Arc<[TaskCallStackFrameDisplay]>,
}

impl Display for TaskCallStackDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, frame) in self.frames.iter().enumerate() {
            if i > 0 {
                write!(f, " -> ")?;
            }
            write!(f, "{}", frame)?;
        }
        Ok(())
    }
}

pub struct TaskCallFrame {
    pub task_index: TaskNodeIndex,
}

impl<'a> PlanContext<'a> {
    pub fn cwd(&self) -> &Arc<AbsolutePath> {
        &self.cwd
    }

    pub fn envs(&self) -> &HashMap<Arc<OsStr>, Arc<OsStr>> {
        &self.envs
    }

    /// Get a human-readable display of the current task call stack.
    pub fn display_call_stack(&self) -> TaskCallStackDisplay {
        TaskCallStackDisplay {
            frames: self
                .task_call_stack
                .iter()
                .map(|(idx, span)| TaskCallStackFrameDisplay {
                    task_display: self.indexed_task_graph.display_task(*idx),
                    command_span: span.clone(),
                })
                .collect(),
        }
    }

    /// Check if adding the given task node index would create a cycle in the call stack.
    pub fn check_cycle(&self, task_node_index: TaskNodeIndex) -> Result<(), TaskCycleError> {
        if let Some(cycle_start) =
            self.task_call_stack.iter().position(|(idx, _)| *idx == task_node_index)
        {
            return Err(TaskCycleError { cycle_start });
        }
        Ok(())
    }

    pub fn indexed_task_graph(&self) -> &'a IndexedTaskGraph {
        self.indexed_task_graph
    }

    /// Push a new frame onto the task call stack.
    pub fn push_stack_frame(&mut self, task_node_index: TaskNodeIndex, command_span: Range<usize>) {
        self.task_call_stack.push((task_node_index, command_span));
    }

    pub fn callbacks(&mut self) -> &mut (dyn PlanCallbacks + '_) {
        self.callbacks
    }

    pub fn prepend_path(&mut self, path_to_prepend: &AbsolutePath) -> Result<(), JoinPathsError> {
        prepend_path_env(&mut self.envs, path_to_prepend)
    }

    pub fn add_envs(
        &mut self,
        new_envs: impl Iterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
    ) {
        for (key, value) in new_envs {
            self.envs.insert(Arc::from(key.as_ref()), Arc::from(value.as_ref()));
        }
    }

    pub fn duplicate(&mut self) -> PlanContext<'_> {
        PlanContext {
            cwd: Arc::clone(&self.cwd),
            envs: self.envs.clone(),
            callbacks: self.callbacks,
            task_call_stack: self.task_call_stack.clone(),
            indexed_task_graph: self.indexed_task_graph,
        }
    }
}
//     pub fn enter_package(&mut self, package_path: Arc<AbsolutePath>) -> Result<PlanContext<'_>, PackageCycleError> {
//         Ok(PlanContext {
//             cwd: package_path,
//             envs: Arc::clone(&self.envs),
//             callbacks: self.callbacks,
//             stack: self.stack,
//         })
//     }

//     /// Create a new context with new frame.
//     ///
//     /// Returns `None` if the new frame already exists in the stack (to prevent infinite recursion).
//     pub fn with_new_frame<R>(
//         &mut self,
//         new_frame: PlanStackFrame,
//         envs: impl Iterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
//         cwd: Arc<AbsolutePath>,
//         f: impl FnOnce(PlanContext<'_>) -> R,
//     ) -> Option<R> {
//         // IndexSet::insert returns `false` and doesn't touch the set if the item already exists.
//         if !self.stack.insert(new_frame) {
//             return None;
//         }
//         // Merge envs
//         let mut new_envs: Option<HashMap<Arc<OsStr>, Arc<OsStr>>> = None;
//         for (key, value) in envs {
//             // Clone on write
//             new_envs
//                 .get_or_insert_with(|| self.envs.as_ref().clone())
//                 .insert(Arc::from(key.as_ref()), Arc::from(value.as_ref()));
//         }

//         let ret = f(PlanContext {
//             cwd,
//             envs: if let Some(new_envs) = new_envs {
//                 Arc::new(new_envs)
//             } else {
//                 Arc::clone(&self.envs)
//             },
//             callbacks: self.callbacks,
//             stack: self.stack,
//         });
//         self.stack.pop().expect("stack pop should succeed");
//         Some(ret)
//     }
// }
