//! Reporter traits and implementations for rendering execution events.
//!
//! This module provides a typestate-based reporter system with three phases:
//!
//! 1. [`GraphExecutionReporterBuilder`] — created before the execution graph is known.
//!    Transitions to [`GraphExecutionReporter`] when `build()` is called with the graph.
//!
//! 2. [`GraphExecutionReporter`] — knows the execution graph. Creates [`LeafExecutionReporter`]
//!    instances for individual leaf executions via `new_leaf_execution()`. Finalized with `finish()`.
//!
//! 3. [`LeafExecutionReporter`] — handles events for a single leaf execution (output streaming,
//!    cache status, errors). Finalized with `finish()`.
//!
//! Two concrete implementations are provided in child modules:
//!
//! - [`plain::PlainReporter`] — a standalone [`LeafExecutionReporter`] for single-leaf execution
//!   (e.g., `execute_synthetic`). Self-contained, no shared state, no summary.
//!
//! - [`labeled::LabeledReporterBuilder`] / [`labeled::LabeledGraphReporter`] /
//!   `LabeledLeafReporter` — a full graph-aware reporter family. Tracks stats across multiple
//!   leaf executions, prints command lines with cache status, and renders a summary at the end.

mod labeled;
mod plain;

// Re-export the concrete implementations so callers can use `reporter::PlainReporter`
// and `reporter::LabeledReporterBuilder` without navigating into child modules.
use std::{
    process::ExitStatus as StdExitStatus,
    sync::{Arc, LazyLock},
};

pub use labeled::LabeledReporterBuilder;
use owo_colors::{Style, Styled};
pub use plain::PlainReporter;
use smallvec::SmallVec;
use tokio::io::AsyncWrite;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_plan::{
    ExecutionGraph, ExecutionItem, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind,
};

use super::{
    cache::format_cache_status_inline,
    event::{CacheStatus, CacheUpdateStatus, ExecutionError},
};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Exit status type
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Exit status code for task execution.
///
/// Wraps a `u8` exit code. `0` means success, non-zero means failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus(pub u8);

impl ExitStatus {
    pub const FAILURE: Self = Self(1);
    pub const SUCCESS: Self = Self(0);
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Stdio suggestion and configuration
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Suggestion from the reporter about what stdio mode to use for a spawned process.
///
/// The actual stdio mode is determined by [`execute_spawn`](super::execute::execute_spawn)
/// based on this suggestion AND whether caching is enabled for the task:
/// - `Inherited` is only honoured when caching is disabled (`cache_metadata` is `None`).
///   With caching enabled, the execution engine overrides to `Piped` so that output can
///   be captured for the cache.
/// - `Piped` is always respected as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioSuggestion {
    /// stdin is `/dev/null`, stdout and stderr are piped into the reporter's
    /// [`AsyncWrite`] streams.  Used when multiple tasks run concurrently and
    /// stdio should not be shared.
    Piped,
    /// All three file descriptors (stdin, stdout, stderr) are inherited from the
    /// parent process, allowing interactive input and direct terminal output.
    /// Only effective when caching is disabled for the task.
    Inherited,
}

/// Stdio configuration returned by [`LeafExecutionReporter::start`].
///
/// Contains the reporter's suggestion for the stdio mode together with two
/// async writers that receive the child process's stdout and stderr when the
/// execution engine decides to use piped mode.  The writers are always provided
/// because the engine may override the suggestion (e.g. when caching forces
/// piped mode even though the reporter suggested inherited).
pub struct StdioConfig {
    /// The reporter's preferred stdio mode.
    pub suggestion: StdioSuggestion,
    /// Async writer for the child process's stdout (used in piped mode and cache replay).
    pub stdout_writer: Box<dyn AsyncWrite + Unpin>,
    /// Async writer for the child process's stderr (used in piped mode and cache replay).
    pub stderr_writer: Box<dyn AsyncWrite + Unpin>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Leaf execution path — identifies a leaf within a (potentially nested) execution graph
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// One step in a [`LeafExecutionPath`]: identifies a specific execution item
/// within a single level of the execution graph.
#[derive(Clone, Copy, Debug)]
struct ExecutionPathItem {
    /// The node (task) index in the execution graph at this level.
    graph_node_ix: ExecutionNodeIndex,
    /// The item index within the task's `items` vector.
    task_execution_item_index: usize,
}

use vite_task_plan::execution_graph::ExecutionNodeIndex;

impl ExecutionPathItem {
    /// Resolve this path item against a graph, returning the [`ExecutionItem`]
    /// at the identified position.
    fn resolve(self, graph: &ExecutionGraph) -> &ExecutionItem {
        &graph[self.graph_node_ix].items[self.task_execution_item_index]
    }
}

/// A path through a (potentially nested) execution graph that identifies a specific
/// leaf execution.
///
/// Each element in the path represents a step deeper into a nested `Expanded` execution
/// graph. The last element identifies the actual leaf item.
///
/// For example, a path of `[(node_0, item_1), (node_2, item_0)]` means:
/// - In the root graph, node 0, item 1 (which is an `Expanded` containing a nested graph)
/// - In that nested graph, node 2, item 0 (the actual leaf execution)
///
/// Uses `SmallVec` with inline capacity of 4 since most execution graphs are shallow
/// (typically 1-2 levels of nesting).
#[derive(Clone, Debug, Default)]
pub struct LeafExecutionPath(SmallVec<ExecutionPathItem, 4>);

impl LeafExecutionPath {
    /// Append a new step to this path, identifying an item at the given node and item indices.
    pub fn push(&mut self, graph_node_ix: ExecutionNodeIndex, task_execution_item_index: usize) {
        self.0.push(ExecutionPathItem { graph_node_ix, task_execution_item_index });
    }

    /// Resolve this path against a root execution graph, returning the final
    /// [`ExecutionItem`] the path points to.
    ///
    /// This is the shared traversal logic that walks through nested `Expanded`
    /// graphs. Used by:
    /// - `Index<&LeafExecutionPath> for ExecutionGraph` — extracts `&LeafExecutionKind`
    /// - `new_leaf_execution` in `labeled.rs` — extracts `ExecutionItemDisplay`
    ///
    /// # Panics
    ///
    /// - If the path is empty (indicates a bug in path construction).
    /// - If an intermediate path element points to a `Leaf` item instead of
    ///   `Expanded` (only `Expanded` items contain nested graphs to descend into).
    fn resolve_item<'a>(&self, root_graph: &'a ExecutionGraph) -> &'a ExecutionItem {
        let mut current_graph = root_graph;
        let last_depth = self.0.len() - 1;
        for (depth, path_item) in self.0.iter().enumerate() {
            let item = path_item.resolve(current_graph);
            if depth == last_depth {
                return item;
            }
            match &item.kind {
                ExecutionItemKind::Expanded(nested_graph) => {
                    current_graph = nested_graph;
                }
                ExecutionItemKind::Leaf(_) => {
                    unreachable!(
                        "LeafExecutionPath: intermediate element at depth {depth} is a Leaf, expected Expanded"
                    )
                }
            }
        }
        unreachable!("LeafExecutionPath: empty path")
    }

    /// Return true when this leaf is contained in a chain of execution graphs where
    /// every graph (root + all nested `Expanded` ancestors) has exactly one node.
    ///
    /// This is used by the labeled reporter to determine whether inherited stdio can
    /// be suggested for spawned processes.
    ///
    /// # Panics
    ///
    /// - If the path is empty (indicates a bug in path construction).
    /// - If an intermediate path element points to a `Leaf` item instead of
    ///   `Expanded`.
    fn all_containing_graphs_single_node(&self, root_graph: &ExecutionGraph) -> bool {
        let Some((last_path_item, parent_path_items)) = self.0.split_last() else {
            unreachable!("LeafExecutionPath: empty path")
        };

        let mut current_graph = root_graph;
        if current_graph.node_count() != 1 {
            return false;
        }

        for (depth, path_item) in parent_path_items.iter().enumerate() {
            let item = path_item.resolve(current_graph);
            match &item.kind {
                ExecutionItemKind::Expanded(nested_graph) => {
                    current_graph = nested_graph;
                    if current_graph.node_count() != 1 {
                        return false;
                    }
                }
                ExecutionItemKind::Leaf(_) => {
                    unreachable!(
                        "LeafExecutionPath: intermediate element at depth {depth} is a Leaf, expected Expanded"
                    )
                }
            }
        }

        // Validate that the final path item resolves in the containing graph.
        let _ = last_path_item.resolve(current_graph);
        true
    }
}

impl std::ops::Index<&LeafExecutionPath> for ExecutionGraph {
    type Output = LeafExecutionKind;

    fn index(&self, path: &LeafExecutionPath) -> &Self::Output {
        match &path.resolve_item(self).kind {
            ExecutionItemKind::Leaf(kind) => kind,
            ExecutionItemKind::Expanded(_) => {
                unreachable!("LeafExecutionPath: final element is Expanded, expected Leaf")
            }
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Typestate traits
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Builder for creating a [`GraphExecutionReporter`].
///
/// This is the initial state before the execution graph is known. The `build` method
/// transitions to the next state by providing the graph.
pub trait GraphExecutionReporterBuilder {
    /// Create a [`GraphExecutionReporter`] for the given execution graph.
    ///
    /// The reporter may clone the `Arc` to retain a reference to the graph
    /// for looking up display information during execution.
    fn build(self: Box<Self>, graph: &Arc<ExecutionGraph>) -> Box<dyn GraphExecutionReporter>;
}

/// Reporter for an entire graph execution session.
///
/// Creates [`LeafExecutionReporter`] instances for individual leaf executions
/// and finalizes the session with `finish()`.
#[async_trait::async_trait(?Send)]
pub trait GraphExecutionReporter {
    /// Create a new leaf execution reporter for the leaf identified by `path`.
    ///
    /// The reporter implementation can look up display info (task name, command, cwd)
    /// from the execution graph using the path.
    fn new_leaf_execution(&mut self, path: &LeafExecutionPath) -> Box<dyn LeafExecutionReporter>;

    /// Finalize the graph execution session.
    ///
    /// Leaf-level errors are already tracked internally by the reporter via the
    /// leaf reporter's `finish()` method. Graph-level errors (cycle detection) are
    /// now caught at plan time and never reach the reporter.
    ///
    /// Returns `Ok(())` if all tasks succeeded, or `Err(ExitStatus)` to indicate the
    /// process should exit with the given status code.
    async fn finish(self: Box<Self>) -> Result<(), ExitStatus>;
}

/// Reporter for a single leaf execution (one spawned process or in-process command).
///
/// Lifecycle: `start()` → `finish()`.
///
/// `start()` may not be called before `finish()` if an error occurs before the cache
/// status is determined (e.g., cache lookup failure).
#[async_trait::async_trait(?Send)]
pub trait LeafExecutionReporter {
    /// Report that execution is starting with the given cache status.
    ///
    /// Called after the cache lookup completes, before any output is produced.
    /// Returns a [`StdioConfig`] containing:
    /// - The reporter's stdio mode suggestion (inherited or piped).
    /// - Two [`AsyncWrite`] streams for receiving the child's stdout and stderr
    ///   (used when the execution engine decides on piped mode, or for cache replay).
    ///
    /// The execution engine decides the actual stdio mode based on the suggestion
    /// AND whether caching is enabled — inherited stdio is only used when the
    /// suggestion is [`StdioSuggestion::Inherited`] AND the task has no cache
    /// metadata (caching disabled).
    async fn start(&mut self, cache_status: CacheStatus) -> StdioConfig;

    /// Finalize this leaf execution.
    ///
    /// - `status`: The process exit status, or `None` for cache hits and in-process commands.
    /// - `cache_update_status`: Whether the cache was updated after execution.
    /// - `error`: If `Some`, an error occurred during this leaf's execution (cache lookup
    ///   failure, spawn failure, fingerprint creation failure, cache update failure).
    ///
    /// This method consumes the reporter — no further calls are possible after `finish()`.
    async fn finish(
        self: Box<Self>,
        status: Option<StdExitStatus>,
        cache_update_status: CacheUpdateStatus,
        error: Option<ExecutionError>,
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Shared display helpers
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Wrap of `OwoColorize` that ignores style if `NO_COLOR` is set.
trait ColorizeExt {
    fn style(&self, style: Style) -> Styled<&Self>;
}

impl<T: owo_colors::OwoColorize> ColorizeExt for T {
    fn style(&self, style: Style) -> Styled<&Self> {
        static NO_COLOR: LazyLock<bool> =
            LazyLock::new(|| std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()));
        owo_colors::OwoColorize::style(self, if *NO_COLOR { Style::new() } else { style })
    }
}

const COMMAND_STYLE: Style = Style::new().cyan();
const CACHE_MISS_STYLE: Style = Style::new().purple();

/// Format the display's cwd as a string relative to the workspace root.
/// Returns an empty string if the cwd equals the workspace root.
fn format_cwd_relative(display: &ExecutionItemDisplay, workspace_path: &AbsolutePath) -> Str {
    let cwd_relative = if let Ok(Some(rel)) = display.cwd.strip_prefix(workspace_path) {
        Str::from(rel.as_str())
    } else {
        Str::default()
    };
    if cwd_relative.is_empty() { Str::default() } else { vite_str::format!("~/{cwd_relative}") }
}

/// Format the command string with cwd prefix for display (e.g., `~/packages/lib$ vitest run`).
fn format_command_display(display: &ExecutionItemDisplay, workspace_path: &AbsolutePath) -> Str {
    let cwd_str = format_cwd_relative(display, workspace_path);
    vite_str::format!("{cwd_str}$ {}", display.command)
}

/// Format the command line with optional inline cache status.
///
/// This is called during `start()` to show the user what command is being executed
/// and its cache status. The caller writes the returned string to the async writer.
fn format_command_with_cache_status(
    display: &ExecutionItemDisplay,
    workspace_path: &AbsolutePath,
    cache_status: &CacheStatus,
) -> Str {
    let command_str = format_command_display(display, workspace_path);
    format_cache_status_inline(cache_status).map_or_else(
        || vite_str::format!("{}\n", command_str.style(COMMAND_STYLE)),
        |inline_status| {
            // Apply styling based on cache status type
            let styled_status = match cache_status {
                CacheStatus::Hit { .. } => inline_status.style(Style::new().green().dimmed()),
                CacheStatus::Miss(_) => inline_status.style(CACHE_MISS_STYLE.dimmed()),
                CacheStatus::Disabled(_) => inline_status.style(Style::new().bright_black()),
            };
            vite_str::format!("{} {}\n", command_str.style(COMMAND_STYLE), styled_status)
        },
    )
}

/// Format an error message in red with an error icon.
fn format_error_message(message: &str) -> Str {
    vite_str::format!(
        "{} {}\n",
        "✗".style(Style::new().red().bold()),
        message.style(Style::new().red())
    )
}

/// Format the "cache hit, logs replayed" message for synthetic executions without display info.
fn format_cache_hit_message() -> Str {
    vite_str::format!("{}\n", "✓ cache hit, logs replayed".style(Style::new().green().dimmed()))
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Test fixtures (shared by child module tests)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
pub mod test_fixtures {
    use std::{collections::BTreeMap, sync::Arc};

    use vite_path::AbsolutePath;
    use vite_task_graph::display::TaskDisplay;
    use vite_task_plan::{
        ExecutionGraph, ExecutionItem, ExecutionItemDisplay, ExecutionItemKind, InProcessExecution,
        LeafExecutionKind, SpawnCommand, SpawnExecution, TaskExecution,
    };

    /// Create a dummy `AbsolutePath` for test fixtures.
    pub fn test_path() -> Arc<AbsolutePath> {
        #[cfg(unix)]
        {
            Arc::from(AbsolutePath::new("/test").unwrap())
        }
        #[cfg(windows)]
        {
            Arc::from(AbsolutePath::new("C:\\test").unwrap())
        }
    }

    /// Create a dummy `TaskDisplay` for test fixtures.
    pub fn test_task_display(name: &str) -> TaskDisplay {
        TaskDisplay {
            package_name: "pkg".into(),
            task_name: name.into(),
            package_path: test_path(),
        }
    }

    /// Create a dummy `ExecutionItemDisplay` for test fixtures.
    pub fn test_display(name: &str) -> ExecutionItemDisplay {
        ExecutionItemDisplay {
            task_display: test_task_display(name),
            command: name.into(),
            and_item_index: None,
            cwd: test_path(),
        }
    }

    /// Create a `TaskExecution` with a single spawn leaf.
    pub fn spawn_task(name: &str) -> TaskExecution {
        TaskExecution {
            task_display: test_task_display(name),
            items: vec![ExecutionItem {
                execution_item_display: test_display(name),
                kind: ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(SpawnExecution {
                    cache_metadata: None,
                    spawn_command: SpawnCommand {
                        program_path: test_path(),
                        args: Arc::from([]),
                        all_envs: Arc::new(BTreeMap::new()),
                        cwd: test_path(),
                    },
                })),
            }],
        }
    }

    /// Create a `TaskExecution` with a single in-process leaf (echo).
    pub fn in_process_task(name: &str) -> TaskExecution {
        TaskExecution {
            task_display: test_task_display(name),
            items: vec![ExecutionItem {
                execution_item_display: test_display(name),
                kind: ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(
                    InProcessExecution::get_builtin_execution(
                        "echo",
                        ["hello"].iter(),
                        &test_path(),
                    )
                    .unwrap(),
                )),
            }],
        }
    }

    /// Create a `TaskExecution` with an expanded (nested) subgraph as its item.
    pub fn expanded_task(name: &str, nested_graph: ExecutionGraph) -> TaskExecution {
        TaskExecution {
            task_display: test_task_display(name),
            items: vec![ExecutionItem {
                execution_item_display: test_display(name),
                kind: ExecutionItemKind::Expanded(nested_graph),
            }],
        }
    }
}
