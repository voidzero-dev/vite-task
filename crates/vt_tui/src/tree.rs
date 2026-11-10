//! The task tree the caller hands to [`crate::Tui`], and the state of each node.

use std::time::Duration;

use vt_str::Str;

/// A node in the [`TaskTree`]. Returned by [`TaskTree::add`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a node shows, given when the node is added.
///
/// A node without children is a process: a task with one command, or one step
/// of a task. Its output can be opened in the task view. A node with children
/// is a group: a task with several steps, or a nested `vp run`.
#[derive(Debug, Clone)]
pub struct NodeSpec {
    /// `app#build` for a task, the command for a step.
    pub label: Str,
    /// The command of a task with a single command, shown as `label: command`.
    pub command: Option<Str>,
    /// Whether the task view may send typed lines to the process's stdin.
    pub accepts_input: bool,
}

impl NodeSpec {
    #[must_use]
    pub fn new(label: impl Into<Str>) -> Self {
        Self { label: label.into(), command: None, accepts_input: false }
    }

    #[must_use]
    pub fn command(mut self, command: impl Into<Str>) -> Self {
        self.command = Some(command.into());
        self
    }

    #[must_use]
    pub const fn accepts_input(mut self, accepts_input: bool) -> Self {
        self.accepts_input = accepts_input;
        self
    }
}

/// The state of a process node. Group states are derived from their children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskState {
    /// Waiting for the tasks it depends on.
    Waiting,
    /// Ready to start, waiting for a free concurrency slot.
    Queued,
    Running,
    /// A long-running task that reported it's ready, such as a dev server.
    Ready,
    Succeeded,
    CacheHit,
    Failed,
    /// Stopped by Ctrl-C or because another task failed.
    Cancelled,
    /// Never started because the run stopped first.
    NotRun,
}

impl TaskState {
    #[must_use]
    pub const fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::CacheHit | Self::Failed | Self::Cancelled | Self::NotRun
        )
    }

    #[must_use]
    pub const fn is_started(self) -> bool {
        !matches!(self, Self::Waiting | Self::Queued)
    }

    const fn is_success(self) -> bool {
        matches!(self, Self::Succeeded | Self::CacheHit)
    }
}

/// How a status text reads. A warning on a task that succeeded gives it the
/// warning icon, and puts its output in the report printed after the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tone {
    #[default]
    Normal,
    Warning,
    Error,
}

#[derive(Debug)]
pub struct Node {
    pub spec: NodeSpec,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub depth: u16,
    pub state: TaskState,
    pub status: Option<Str>,
    pub tone: Tone,
    pub started_at: Option<Duration>,
    pub finished_at: Option<Duration>,
    /// Set when the user folds or unfolds the group; otherwise the tree decides.
    pub expanded: Option<bool>,
    /// Set when the user unfolds the finished children the tree folded away.
    pub show_done_children: bool,
}

impl Node {
    pub const fn is_group(&self) -> bool {
        !self.children.is_empty()
    }
}

/// The tasks of a run, in the order they're listed.
///
/// Nested `vp run`s and the steps of a task each add a level. Tasks of the
/// same graph are siblings.
#[derive(Debug, Default)]
pub struct TaskTree {
    pub(crate) nodes: Vec<Node>,
    pub(crate) roots: Vec<NodeId>,
}

/// A node's state and tone after taking its children into account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub state: TaskState,
    pub tone: Tone,
}

impl TaskTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a node under `parent`, or at the top level, after its siblings.
    ///
    /// # Panics
    ///
    /// Panics if `parent` isn't a node of this tree, or if the tree has more
    /// than `u32::MAX` nodes.
    pub fn add(&mut self, parent: Option<NodeId>, spec: NodeSpec) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("too many nodes"));
        let depth = parent.map_or(0, |parent| self.nodes[parent.index()].depth + 1);
        self.nodes.push(Node {
            spec,
            parent,
            children: Vec::new(),
            depth,
            state: TaskState::Waiting,
            status: None,
            tone: Tone::Normal,
            started_at: None,
            finished_at: None,
            expanded: None,
            show_done_children: false,
        });
        match parent {
            Some(parent) => self.nodes[parent.index()].children.push(id),
            None => self.roots.push(id),
        }
        id
    }

    /// What the node was added with.
    #[must_use]
    pub fn spec(&self, id: NodeId) -> &NodeSpec {
        &self.node(id).spec
    }

    /// The node's parent, or `None` at the top level.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    pub(crate) fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.nodes.len()).map(|index| NodeId(u32::try_from(index).unwrap_or(u32::MAX)))
    }

    /// Process nodes in tree order.
    pub(crate) fn leaves(&self) -> Vec<NodeId> {
        let mut leaves = Vec::new();
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let node = self.node(id);
            if node.is_group() {
                stack.extend(node.children.iter().rev());
            } else {
                leaves.push(id);
            }
        }
        leaves
    }

    pub(crate) fn effective(&self, id: NodeId) -> Effective {
        let node = self.node(id);
        if !node.is_group() {
            let tone = if node.state == TaskState::Failed { Tone::Error } else { node.tone };
            return Effective { state: node.state, tone };
        }
        let mut counts = Counts::default();
        for &child in &node.children {
            counts.add(self.effective(child));
        }
        counts.result()
    }

    /// Whether a group's children are listed. The user's choice wins;
    /// otherwise groups that finished cleanly or never ran are folded.
    pub(crate) fn is_expanded(&self, id: NodeId) -> bool {
        let node = self.node(id);
        if let Some(expanded) = node.expanded {
            return expanded;
        }
        let effective = self.effective(id);
        !matches!(
            (effective.state, effective.tone),
            (TaskState::Succeeded | TaskState::CacheHit, Tone::Normal) | (TaskState::NotRun, _)
        )
    }

    /// Whether the node finished cleanly, which lets the tree fold it away
    /// when the screen is short.
    pub(crate) fn is_clean_success(&self, id: NodeId) -> bool {
        let effective = self.effective(id);
        effective.state.is_success() && effective.tone == Tone::Normal
    }

    /// The first start and last finish below a node.
    pub(crate) fn span(&self, id: NodeId) -> (Option<Duration>, Option<Duration>) {
        let node = self.node(id);
        if !node.is_group() {
            return (node.started_at, node.finished_at);
        }
        let mut start: Option<Duration> = None;
        let mut finish: Option<Duration> = None;
        for &child in &node.children {
            let (child_start, child_finish) = self.span(child);
            if let Some(child_start) = child_start {
                start = Some(start.map_or(child_start, |start| start.min(child_start)));
            }
            if let Some(child_finish) = child_finish {
                finish = Some(finish.map_or(child_finish, |finish| finish.max(child_finish)));
            }
        }
        (start, finish)
    }

    /// How many processes below a node have finished, and how many there are.
    pub(crate) fn progress(&self, id: NodeId) -> (usize, usize) {
        let node = self.node(id);
        if !node.is_group() {
            return (usize::from(node.state.is_finished()), 1);
        }
        node.children.iter().fold((0, 0), |(done, total), &child| {
            let (child_done, child_total) = self.progress(child);
            (done + child_done, total + child_total)
        })
    }
}

#[derive(Default)]
#[expect(clippy::struct_excessive_bools, reason = "one flag per state a child can be in")]
struct Counts {
    failed: bool,
    running: bool,
    ready: bool,
    waiting: bool,
    started: bool,
    cancelled: bool,
    not_run: bool,
    succeeded: bool,
    cache_hit: bool,
    warning: bool,
}

impl Counts {
    const fn add(&mut self, effective: Effective) {
        match effective.state {
            TaskState::Failed => self.failed = true,
            TaskState::Running => self.running = true,
            TaskState::Ready => self.ready = true,
            TaskState::Waiting | TaskState::Queued => self.waiting = true,
            TaskState::Cancelled => self.cancelled = true,
            TaskState::NotRun => self.not_run = true,
            TaskState::Succeeded => self.succeeded = true,
            TaskState::CacheHit => self.cache_hit = true,
        }
        if effective.state.is_started() && !matches!(effective.state, TaskState::NotRun) {
            self.started = true;
        }
        if matches!(effective.tone, Tone::Warning) {
            self.warning = true;
        }
    }

    const fn result(&self) -> Effective {
        let tone = if self.failed {
            Tone::Error
        } else if self.warning {
            Tone::Warning
        } else {
            Tone::Normal
        };
        let state = if self.failed {
            TaskState::Failed
        } else if self.running || (self.waiting && self.started) {
            TaskState::Running
        } else if self.waiting {
            TaskState::Waiting
        } else if self.ready {
            TaskState::Ready
        } else if self.cancelled {
            TaskState::Cancelled
        } else if self.not_run {
            TaskState::NotRun
        } else if self.cache_hit && !self.succeeded {
            TaskState::CacheHit
        } else {
            TaskState::Succeeded
        };
        Effective { state, tone }
    }
}
