use std::{
    collections::HashMap, env::JoinPathsError, ffi::OsStr, fmt::Display, ops::Range, sync::Arc,
};

use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskNodeIndex, display::TaskDisplay};

use crate::{PlanRequestParser, path_env::prepend_path_env};

#[derive(Debug, thiserror::Error)]
#[error(
    "Detected a recursion in task call stack: the last frame calls the {0}th frame", recursion_point + 1
)]
pub struct TaskRecursionError {
    /// The index in `task_call_stack` where the last frame recurses to.
    recursion_point: usize,
}

/// The context for planning an execution from a task.
#[derive(Debug)]
pub struct PlanContext<'a> {
    /// The root path of the workspace.
    workspace_path: &'a Arc<AbsolutePath>,

    /// The current working directory.
    cwd: Arc<AbsolutePath>,

    /// The environment variables for the current execution context.
    envs: HashMap<Arc<OsStr>, Arc<OsStr>>,

    /// The callbacks for loading task graphs and parsing commands.
    callbacks: &'a mut (dyn PlanRequestParser + 'a),

    /// The current call stack of task index nodes being planned.
    task_call_stack: Vec<(TaskNodeIndex, Range<usize>)>,

    /// The extra args (`vite run task [extra_arg...]`).
    /// It may come from real cli args, or commands in task scripts.
    extra_args: Arc<[Str]>,

    indexed_task_graph: &'a IndexedTaskGraph,
}

/// A human-readable frame in the task call stack.
#[derive(Debug, Clone)]
pub struct TaskCallStackFrameDisplay {
    pub task_display: TaskDisplay,

    #[expect(dead_code)] // To be used in terminal error display
    pub command_span: Range<usize>,
}

impl Display for TaskCallStackFrameDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: display command_span
        write!(f, "{}", self.task_display)
    }
}

/// A human-readable display of the task call stack.
#[derive(Default, Debug, Clone)]
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

impl<'a> PlanContext<'a> {
    pub fn new(
        workspace_path: &'a Arc<AbsolutePath>,
        cwd: Arc<AbsolutePath>,
        envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
        callbacks: &'a mut (dyn PlanRequestParser + 'a),
        indexed_task_graph: &'a IndexedTaskGraph,
    ) -> Self {
        Self {
            workspace_path,
            cwd,
            envs,
            callbacks,
            task_call_stack: Vec::new(),
            indexed_task_graph,
            extra_args: Default::default(),
        }
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

    /// Check if adding the given task node index would create a recursion in the call stack.
    pub fn check_recursion(
        &self,
        task_node_index: TaskNodeIndex,
    ) -> Result<(), TaskRecursionError> {
        if let Some(recursion_start) =
            self.task_call_stack.iter().position(|(idx, _)| *idx == task_node_index)
        {
            return Err(TaskRecursionError { recursion_point: recursion_start });
        }
        Ok(())
    }

    pub fn indexed_task_graph(&self) -> &'a IndexedTaskGraph {
        self.indexed_task_graph
    }

    pub fn workspace_path(&self) -> &Arc<AbsolutePath> {
        self.workspace_path
    }

    /// Push a new frame onto the task call stack.
    pub fn push_stack_frame(&mut self, task_node_index: TaskNodeIndex, command_span: Range<usize>) {
        self.task_call_stack.push((task_node_index, command_span));
    }

    pub fn callbacks(&mut self) -> &mut (dyn PlanRequestParser + '_) {
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

    pub fn extra_args(&self) -> &Arc<[Str]> {
        &self.extra_args
    }

    pub fn set_extra_args(&mut self, extra_args: Arc<[Str]>) {
        self.extra_args = extra_args;
    }

    pub fn duplicate(&mut self) -> PlanContext<'_> {
        PlanContext {
            workspace_path: self.workspace_path,
            cwd: Arc::clone(&self.cwd),
            envs: self.envs.clone(),
            callbacks: self.callbacks,
            task_call_stack: self.task_call_stack.clone(),
            indexed_task_graph: self.indexed_task_graph,
            extra_args: Arc::clone(&self.extra_args),
        }
    }
}
