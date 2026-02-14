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
//! Two concrete implementations are provided:
//!
//! - [`PlainReporter`] — a standalone [`LeafExecutionReporter`] for single-leaf execution
//!   (e.g., `execute_synthetic`). Self-contained, no shared state, no summary.
//!
//! - [`LabeledReporterBuilder`] / [`LabeledGraphReporter`] / [`LabeledLeafReporter`] — a full
//!   graph-aware reporter family. Tracks stats across multiple leaf executions, prints command
//!   lines with cache status, and renders a summary at the end.

use std::{
    cell::RefCell,
    io::Write,
    process::ExitStatus as StdExitStatus,
    rc::Rc,
    sync::{Arc, LazyLock},
    time::Duration,
};

use bstr::BString;
use owo_colors::{Style, Styled};
use smallvec::SmallVec;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_plan::{
    ExecutionGraph, ExecutionItem, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind,
    execution_graph::ExecutionNodeIndex,
};

use super::{
    cache::{format_cache_status_inline, format_cache_status_summary},
    event::{CacheStatus, CacheUpdateStatus, OutputKind, exit_status_to_code},
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
// Stdin suggestion
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Suggestion from the reporter about what stdin mode to use for a spawned process.
///
/// The actual stdin mode is determined by [`execute_spawn`](super::execute::execute_spawn)
/// based on this suggestion AND whether caching is enabled for the task:
/// - `Inherited` is only used when the suggestion is `Inherited` AND caching is disabled.
///   This prevents non-deterministic user input from corrupting cached output.
/// - `Null` is always respected as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinSuggestion {
    /// Suggest connecting the child process's stdin to /dev/null (or NUL on Windows).
    /// Used when multiple tasks run in sequence and stdin should not be shared.
    Null,
    /// Suggest inheriting stdin from the parent process, allowing interactive input.
    /// Only effective when caching is disabled for the task.
    Inherited,
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

    /// Look up the [`ExecutionItemDisplay`] for the leaf identified by this path,
    /// traversing through nested `Expanded` graphs as needed.
    ///
    /// Returns `None` if the path is empty.
    ///
    /// # Panics
    ///
    /// Panics if an intermediate path element does not point to an `Expanded` item,
    /// which indicates a bug in path construction.
    fn resolve_display<'a>(
        &self,
        root_graph: &'a ExecutionGraph,
    ) -> Option<&'a ExecutionItemDisplay> {
        let mut current_graph = root_graph;
        for (depth, path_item) in self.0.iter().enumerate() {
            let item = path_item.resolve(current_graph);
            let is_last = depth == self.0.len() - 1;
            if is_last {
                // Last element — return the display info regardless of Leaf/Expanded
                return Some(&item.execution_item_display);
            }
            // Intermediate element — must be Expanded so we can descend into it
            match &item.kind {
                ExecutionItemKind::Expanded(nested_graph) => {
                    current_graph = nested_graph;
                }
                ExecutionItemKind::Leaf(_) => {
                    // A Leaf in the middle of the path is a bug in path construction.
                    // This should never happen if the execution engine builds paths correctly.
                    debug_assert!(
                        false,
                        "LeafExecutionPath: intermediate element is a Leaf, expected Expanded"
                    );
                    return None;
                }
            }
        }
        // Empty path
        None
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
    fn finish(self: Box<Self>) -> Result<(), ExitStatus>;
}

/// Reporter for a single leaf execution (one spawned process or in-process command).
///
/// Lifecycle: `start()` → zero or more `output()` → `finish()`.
///
/// `start()` may not be called before `finish()` if an error occurs before the cache
/// status is determined (e.g., cache lookup failure).
pub trait LeafExecutionReporter {
    /// Suggest which stdin mode to use for the spawned process.
    ///
    /// Called by [`execute_spawn`](super::execute::execute_spawn) before spawning to
    /// determine the child process's stdin configuration. The final decision also
    /// depends on whether caching is enabled — inherited stdin is only used when
    /// the suggestion is [`StdinSuggestion::Inherited`] AND the task has no cache
    /// metadata (caching disabled).
    fn stdin_suggestion(&self) -> StdinSuggestion;

    /// Report that execution is starting with the given cache status.
    ///
    /// Called after the cache lookup completes, before any output is produced.
    fn start(&mut self, cache_status: CacheStatus);

    /// Report a chunk of output (stdout or stderr) from the executing process.
    fn output(&mut self, kind: OutputKind, content: BString);

    /// Finalize this leaf execution.
    ///
    /// - `status`: The process exit status, or `None` for cache hits and in-process commands.
    /// - `cache_update_status`: Whether the cache was updated after execution.
    /// - `error`: If `Some`, an error occurred during this leaf's execution (cache lookup
    ///   failure, spawn failure, fingerprint creation failure, cache update failure).
    ///
    /// This method consumes the reporter — no further calls are possible after `finish()`.
    fn finish(
        self: Box<Self>,
        status: Option<StdExitStatus>,
        cache_update_status: CacheUpdateStatus,
        error: Option<Str>,
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

/// Write the command line with optional inline cache status to the writer.
///
/// This is called during `start()` to show the user what command is being executed
/// and its cache status.
fn write_command_with_cache_status(
    writer: &mut impl Write,
    display: &ExecutionItemDisplay,
    workspace_path: &AbsolutePath,
    cache_status: &CacheStatus,
) {
    let command_str = format_command_display(display, workspace_path);
    if let Some(inline_status) = format_cache_status_inline(cache_status) {
        // Apply styling based on cache status type
        let styled_status = match cache_status {
            CacheStatus::Hit { .. } => inline_status.style(Style::new().green().dimmed()),
            CacheStatus::Miss(_) => inline_status.style(CACHE_MISS_STYLE.dimmed()),
            CacheStatus::Disabled(_) => inline_status.style(Style::new().bright_black()),
        };
        let _ = writeln!(writer, "{} {}", command_str.style(COMMAND_STYLE), styled_status);
    } else {
        let _ = writeln!(writer, "{}", command_str.style(COMMAND_STYLE));
    }
}

/// Write an error message in red with an error icon.
fn write_error_message(writer: &mut impl Write, message: &str) {
    let _ = writeln!(
        writer,
        "{} {}",
        "✗".style(Style::new().red().bold()),
        message.style(Style::new().red())
    );
}

/// Write the "cache hit, logs replayed" message for synthetic executions without display info.
fn write_cache_hit_message(writer: &mut impl Write) {
    let _ =
        writeln!(writer, "{}", "✓ cache hit, logs replayed".style(Style::new().green().dimmed()));
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// PlainReporter — standalone LeafExecutionReporter for single-leaf execution
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// A self-contained [`LeafExecutionReporter`] for single-leaf executions
/// (e.g., `execute_synthetic`).
///
/// This reporter:
/// - Owns its writer directly (no `Rc<RefCell>` shared state)
/// - Has no display info (synthetic executions have no task display)
/// - Does not track stats or print summaries
/// - Supports `silent_if_cache_hit` to suppress output for cached executions
///
/// The exit status is determined by the caller from the `execute_spawn` return value,
/// not from the reporter.
pub struct PlainReporter<W: Write> {
    writer: W,
    /// When true, suppresses all output (command line, process output, cache hit message)
    /// for executions that are cache hits.
    silent_if_cache_hit: bool,
    /// Whether the current execution is a cache hit, set by `start()`.
    is_cache_hit: bool,
}

impl<W: Write> PlainReporter<W> {
    /// Create a new plain reporter.
    ///
    /// - `writer`: The output stream (typically `std::io::stdout()`).
    /// - `silent_if_cache_hit`: If true, suppress all output when the execution is a cache hit.
    pub const fn new(writer: W, silent_if_cache_hit: bool) -> Self {
        Self { writer, silent_if_cache_hit, is_cache_hit: false }
    }

    /// Returns true if output should be suppressed for this execution.
    const fn is_silent(&self) -> bool {
        self.silent_if_cache_hit && self.is_cache_hit
    }
}

impl<W: Write> LeafExecutionReporter for PlainReporter<W> {
    fn stdin_suggestion(&self) -> StdinSuggestion {
        // PlainReporter is used for single-leaf synthetic executions (e.g., auto-install).
        // Always suggest inherited stdin so the spawned process can be interactive.
        StdinSuggestion::Inherited
    }

    fn start(&mut self, cache_status: CacheStatus) {
        self.is_cache_hit = matches!(cache_status, CacheStatus::Hit { .. });
        // PlainReporter has no display info (synthetic executions),
        // so there's no command line to print at start.
    }

    fn output(&mut self, _kind: OutputKind, content: BString) {
        if self.is_silent() {
            return;
        }
        let _ = self.writer.write_all(&content);
        let _ = self.writer.flush();
    }

    fn finish(
        mut self: Box<Self>,
        _status: Option<StdExitStatus>,
        _cache_update_status: CacheUpdateStatus,
        error: Option<Str>,
    ) {
        // Handle errors — print inline error message
        if let Some(ref message) = error {
            write_error_message(&mut self.writer, message);
            return;
        }

        // For cache hits, print the "cache hit" message (unless silent)
        if self.is_cache_hit && !self.is_silent() {
            write_cache_hit_message(&mut self.writer);
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// LabeledReporter family — graph-aware reporter with aggregation and summary
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Information tracked for each leaf execution, used in the final summary.
#[derive(Debug)]
struct ExecutionInfo {
    /// Display info for this execution. `None` for displayless executions
    /// (e.g., synthetics reached via nested expansion).
    display: Option<ExecutionItemDisplay>,
    /// Cache status, determined at `start()`.
    cache_status: CacheStatus,
    /// Exit status from the process. `None` means no process was spawned (cache hit or in-process).
    exit_status: Option<StdExitStatus>,
    /// Error message, set on error.
    error_message: Option<Str>,
}

/// Running statistics updated as leaf executions complete.
#[derive(Default)]
struct ExecutionStats {
    cache_hits: usize,
    cache_misses: usize,
    cache_disabled: usize,
    failed: usize,
}

/// Mutable state shared between [`LabeledGraphReporter`] and its [`LabeledLeafReporter`] instances
/// via `Rc<RefCell<...>>`.
///
/// This is safe because execution is single-threaded and sequential — only one leaf
/// reporter is active at a time.
struct SharedReporterState<W: Write> {
    writer: W,
    executions: Vec<ExecutionInfo>,
    stats: ExecutionStats,
    /// Total number of spawned leaf executions in the graph (including nested `Expanded`
    /// subgraphs). Computed once at build time and used to determine the stdin suggestion:
    /// inherited stdin is only suggested when there is exactly one spawn leaf.
    spawn_leaf_count: usize,
}

/// Count the total number of spawned leaf executions in an execution graph,
/// recursing into nested `Expanded` subgraphs.
///
/// In-process executions are not counted because they don't spawn child processes
/// and thus don't need stdin.
fn count_spawn_leaves(graph: &ExecutionGraph) -> usize {
    graph
        .node_weights()
        .flat_map(|task| task.items.iter())
        .map(|item| match &item.kind {
            ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(_)) => 1,
            ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(_)) => 0,
            ExecutionItemKind::Expanded(nested_graph) => count_spawn_leaves(nested_graph),
        })
        .sum()
}

/// Builder for the labeled graph reporter.
///
/// Created by the caller before execution, then transitioned to [`LabeledGraphReporter`]
/// by calling `build()` with the execution graph.
///
/// # Output Modes
///
/// ## Normal Mode (default)
/// - Prints command lines with cache status indicators during execution
/// - Shows full summary with Statistics and Task Details at the end
///
/// ## Simplified Summary for Single Tasks
/// - When a single task with display info is executed:
///   - Skips full summary (no Statistics/Task Details sections)
///   - Shows only cache status inline
///   - Results in clean output showing just the command's stdout/stderr
pub struct LabeledReporterBuilder<W: Write> {
    writer: W,
    workspace_path: Arc<AbsolutePath>,
}

impl<W: Write> LabeledReporterBuilder<W> {
    /// Create a new labeled reporter builder.
    ///
    /// - `writer`: The output stream (typically `std::io::stdout()`).
    /// - `workspace_path`: The workspace root, used to compute relative cwds in display.
    pub const fn new(writer: W, workspace_path: Arc<AbsolutePath>) -> Self {
        Self { writer, workspace_path }
    }
}

impl<W: Write + 'static> GraphExecutionReporterBuilder for LabeledReporterBuilder<W> {
    fn build(self: Box<Self>, graph: &Arc<ExecutionGraph>) -> Box<dyn GraphExecutionReporter> {
        let spawn_leaf_count = count_spawn_leaves(graph);
        Box::new(LabeledGraphReporter {
            shared: Rc::new(RefCell::new(SharedReporterState {
                writer: self.writer,
                executions: Vec::new(),
                stats: ExecutionStats::default(),
                spawn_leaf_count,
            })),
            graph: Arc::clone(graph),
            workspace_path: self.workspace_path,
        })
    }
}

/// Graph-level reporter that tracks multiple leaf executions and prints a summary.
///
/// Creates [`LabeledLeafReporter`] instances for each leaf execution. The leaf reporters
/// share mutable state with this reporter via `Rc<RefCell<SharedReporterState>>`.
pub struct LabeledGraphReporter<W: Write> {
    shared: Rc<RefCell<SharedReporterState<W>>>,
    graph: Arc<ExecutionGraph>,
    workspace_path: Arc<AbsolutePath>,
}

impl<W: Write + 'static> GraphExecutionReporter for LabeledGraphReporter<W> {
    fn new_leaf_execution(&mut self, path: &LeafExecutionPath) -> Box<dyn LeafExecutionReporter> {
        // Look up display info from the graph using the path
        let display = path.resolve_display(&self.graph).cloned();
        Box::new(LabeledLeafReporter {
            shared: Rc::clone(&self.shared),
            display,
            workspace_path: Arc::clone(&self.workspace_path),
            started: false,
            is_cache_hit: false,
        })
    }

    fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        let mut shared = self.shared.borrow_mut();

        // Print summary.
        // Special case: single execution without display info (e.g., synthetic via nested expansion)
        // → skip summary since there's nothing meaningful to show.
        let is_single_displayless =
            shared.executions.len() == 1 && shared.executions[0].display.is_none();
        if !is_single_displayless {
            // Destructure to get simultaneous mutable access to writer and immutable
            // access to executions/stats, satisfying the borrow checker.
            let SharedReporterState { ref mut writer, ref executions, ref stats, .. } = *shared;
            print_summary(writer, executions, stats, &self.workspace_path);
        }

        // Determine exit code based on failed tasks and infrastructure errors:
        // - Infrastructure errors (cache lookup, spawn failure) have error_message set
        //   but no meaningful exit_status.
        // - Process failures have a non-zero exit_status.
        //
        // Rules:
        // 1. No failures at all → Ok(())
        // 2. Exactly one process failure, no infra errors → use that task's exit code
        // 3. Any infra errors, or multiple failures → Err(1)
        let has_infra_errors = shared.executions.iter().any(|exec| exec.error_message.is_some());

        let failed_exit_codes: Vec<i32> = shared
            .executions
            .iter()
            .filter_map(|exec| exec.exit_status.as_ref())
            .filter(|status| !status.success())
            .map(|status| exit_status_to_code(*status))
            .collect();

        match (has_infra_errors, failed_exit_codes.as_slice()) {
            (false, []) => Ok(()),
            (false, [code]) => {
                // Return the single failed task's exit code (clamped to u8 range)
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "value is clamped to 1..=255, always positive"
                )]
                Err(ExitStatus((*code).clamp(1, 255) as u8))
            }
            _ => Err(ExitStatus::FAILURE),
        }
    }
}

/// Leaf-level reporter created by [`LabeledGraphReporter::new_leaf_execution`].
///
/// Writes output in real-time to the shared writer and updates shared stats/errors
/// via `Rc<RefCell<SharedReporterState>>`.
struct LabeledLeafReporter<W: Write> {
    shared: Rc<RefCell<SharedReporterState<W>>>,
    /// Display info for this execution, looked up from the graph via the path.
    display: Option<ExecutionItemDisplay>,
    workspace_path: Arc<AbsolutePath>,
    /// Whether `start()` has been called. Used to determine if stats should be updated
    /// in `finish()` and whether to push an `ExecutionInfo` entry.
    started: bool,
    /// Whether the current execution is a cache hit, set by `start()`.
    is_cache_hit: bool,
}

impl<W: Write> LeafExecutionReporter for LabeledLeafReporter<W> {
    fn stdin_suggestion(&self) -> StdinSuggestion {
        // Only suggest inherited stdin when the graph has exactly one spawned leaf
        // execution. With multiple spawned tasks, stdin should not be shared — each
        // task gets /dev/null to avoid contention.
        let shared = self.shared.borrow();
        if shared.spawn_leaf_count == 1 {
            StdinSuggestion::Inherited
        } else {
            StdinSuggestion::Null
        }
    }

    fn start(&mut self, cache_status: CacheStatus) {
        self.started = true;
        self.is_cache_hit = matches!(cache_status, CacheStatus::Hit { .. });

        let mut shared = self.shared.borrow_mut();

        // Update statistics based on cache status
        match &cache_status {
            CacheStatus::Hit { .. } => shared.stats.cache_hits += 1,
            CacheStatus::Miss(_) => shared.stats.cache_misses += 1,
            CacheStatus::Disabled(_) => shared.stats.cache_disabled += 1,
        }

        // Print command line with cache status (if display info is available)
        if let Some(ref display) = self.display {
            write_command_with_cache_status(
                &mut shared.writer,
                display,
                &self.workspace_path,
                &cache_status,
            );
        }

        // Store execution info for the summary
        shared.executions.push(ExecutionInfo {
            display: self.display.clone(),
            cache_status,
            exit_status: None,
            error_message: None,
        });
    }

    fn output(&mut self, _kind: OutputKind, content: BString) {
        let mut shared = self.shared.borrow_mut();
        let _ = shared.writer.write_all(&content);
        let _ = shared.writer.flush();
    }

    fn finish(
        self: Box<Self>,
        status: Option<StdExitStatus>,
        _cache_update_status: CacheUpdateStatus,
        error: Option<Str>,
    ) {
        let mut shared = self.shared.borrow_mut();

        // Handle errors
        if let Some(ref message) = error {
            write_error_message(&mut shared.writer, message);

            // Update the execution info if start() was called (an entry was pushed).
            // Without the `self.started` guard, `last_mut()` would return a
            // *different* execution's entry, corrupting its error_message.
            if self.started
                && let Some(exec) = shared.executions.last_mut()
            {
                exec.error_message = Some(message.clone());
            }

            shared.stats.failed += 1;
        }

        // Update failure statistics for non-zero exit status (not an error, just a failed task)
        // None means success (cache hit or in-process), Some checks the actual exit status
        if error.is_none() && status.is_some_and(|s| !s.success()) {
            shared.stats.failed += 1;
        }

        // Update execution info with exit status (if start() was called and an entry exists)
        if self.started
            && let Some(exec) = shared.executions.last_mut()
        {
            exec.exit_status = status;
        }

        // For executions without display info (synthetics via nested expansion) that are
        // cache hits, print the cache hit message
        if self.started && self.display.is_none() && self.is_cache_hit {
            write_cache_hit_message(&mut shared.writer);
        }

        // Add a trailing newline after each task's output for readability.
        // Skip if start() was never called (e.g. cache lookup failure) — there's
        // no task output to separate.
        if self.started {
            let _ = writeln!(shared.writer);
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Summary printing
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Print the full execution summary with statistics, performance, and per-task details.
///
/// Called by [`LabeledGraphReporter::finish`] after all tasks have executed.
/// Infrastructure errors and task failures are included in the summary.
#[expect(
    clippy::too_many_lines,
    reason = "summary formatting is inherently verbose with many write calls"
)]
fn print_summary(
    writer: &mut impl Write,
    executions: &[ExecutionInfo],
    stats: &ExecutionStats,
    workspace_path: &AbsolutePath,
) {
    let total = executions.len();
    let cache_hits = stats.cache_hits;
    let cache_misses = stats.cache_misses;
    let cache_disabled = stats.cache_disabled;
    let failed = stats.failed;

    // Print summary header with decorative line
    // Note: leaf finish already adds a trailing newline after each task's output
    // Add an extra blank line before the summary for visual separation
    let _ = writeln!(writer);
    let _ = writeln!(
        writer,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(
        writer,
        "{}",
        "    Vite+ Task Runner • Execution Summary".style(Style::new().bold().bright_white())
    );
    let _ = writeln!(
        writer,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(writer);

    // Print statistics
    let cache_disabled_str = if cache_disabled > 0 {
        Str::from(
            vite_str::format!("• {cache_disabled} cache disabled")
                .style(Style::new().bright_black())
                .to_string(),
        )
    } else {
        Str::default()
    };

    let failed_str = if failed > 0 {
        Str::from(vite_str::format!("• {failed} failed").style(Style::new().red()).to_string())
    } else {
        Str::default()
    };

    // Build statistics line, only including non-empty parts
    // Note: trailing space after "cache misses" is intentional for consistent formatting
    let _ = write!(
        writer,
        "{}  {} {} {} ",
        "Statistics:".style(Style::new().bold()),
        vite_str::format!(" {total} tasks").style(Style::new().bright_white()),
        vite_str::format!("• {cache_hits} cache hits").style(Style::new().green()),
        vite_str::format!("• {cache_misses} cache misses").style(CACHE_MISS_STYLE),
    );
    if !cache_disabled_str.is_empty() {
        let _ = write!(writer, "{cache_disabled_str} ");
    }
    if !failed_str.is_empty() {
        let _ = write!(writer, "{failed_str} ");
    }
    let _ = writeln!(writer);

    // Calculate cache hit rate
    let cache_rate = if total > 0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "percentage is always 0..=100, fits in u32"
        )]
        #[expect(clippy::cast_sign_loss, reason = "percentage is always non-negative")]
        #[expect(
            clippy::cast_precision_loss,
            reason = "acceptable precision loss for display percentage"
        )]
        {
            (f64::from(cache_hits as u32) / total as f64 * 100.0) as u32
        }
    } else {
        0
    };

    // Calculate total time saved from cache hits
    let total_saved: Duration = executions
        .iter()
        .filter_map(|exec| {
            if let CacheStatus::Hit { replayed_duration } = &exec.cache_status {
                Some(*replayed_duration)
            } else {
                None
            }
        })
        .sum();

    let _ = write!(
        writer,
        "{}  {} cache hit rate",
        "Performance:".style(Style::new().bold()),
        format_args!("{cache_rate}%").style(if cache_rate >= 75 {
            Style::new().green().bold()
        } else if cache_rate >= 50 {
            CACHE_MISS_STYLE
        } else {
            Style::new().red()
        })
    );

    if total_saved > Duration::ZERO {
        let _ = write!(
            writer,
            ", {:.2?} saved in total",
            total_saved.style(Style::new().green().bold())
        );
    }
    let _ = writeln!(writer);
    let _ = writeln!(writer);

    // Detailed task results
    let _ = writeln!(writer, "{}", "Task Details:".style(Style::new().bold()));
    let _ = writeln!(
        writer,
        "{}",
        "────────────────────────────────────────────────".style(Style::new().bright_black())
    );

    for (idx, exec) in executions.iter().enumerate() {
        // Skip executions without display info (they have nothing to show in the summary)
        let Some(ref display) = exec.display else {
            continue;
        };

        let task_display = &display.task_display;

        // Task name and index
        let _ = write!(
            writer,
            "  {} {}",
            vite_str::format!("[{}]", idx + 1).style(Style::new().bright_black()),
            task_display.to_string().style(Style::new().bright_white().bold())
        );

        // Command with cwd prefix
        let command_display = format_command_display(display, workspace_path);
        let _ = write!(writer, ": {}", command_display.style(COMMAND_STYLE));

        // Execution result icon
        // None means success (cache hit or in-process), Some checks actual status
        match &exec.exit_status {
            None => {
                let _ = write!(writer, " {}", "✓".style(Style::new().green().bold()));
            }
            Some(exit_status) if exit_status.success() => {
                let _ = write!(writer, " {}", "✓".style(Style::new().green().bold()));
            }
            Some(exit_status) => {
                let code = exit_status_to_code(*exit_status);
                let _ = write!(
                    writer,
                    " {} {}",
                    "✗".style(Style::new().red().bold()),
                    vite_str::format!("(exit code: {code})").style(Style::new().red())
                );
            }
        }
        let _ = writeln!(writer);

        // Cache status details — use display module for plain text, apply styling here
        let cache_summary = format_cache_status_summary(&exec.cache_status);
        let styled_summary = match &exec.cache_status {
            CacheStatus::Hit { .. } => cache_summary.style(Style::new().green()),
            CacheStatus::Miss(_) => cache_summary.style(CACHE_MISS_STYLE),
            CacheStatus::Disabled(_) => cache_summary.style(Style::new().bright_black()),
        };
        let _ = writeln!(writer, "      {styled_summary}");

        // Error message if present
        if let Some(ref error_msg) = exec.error_message {
            let _ = writeln!(
                writer,
                "      {} {}",
                "✗ Error:".style(Style::new().red().bold()),
                error_msg.style(Style::new().red())
            );
        }

        // Add spacing between tasks except for the last one
        if idx < executions.len() - 1 {
            let _ = writeln!(
                writer,
                "  {}",
                "·······················································"
                    .style(Style::new().bright_black())
            );
        }
    }

    let _ = writeln!(
        writer,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Tests
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use vite_task_graph::display::TaskDisplay;
    use vite_task_plan::{
        ExecutionItem, ExecutionItemDisplay, ExecutionItemKind, InProcessExecution,
        LeafExecutionKind, SpawnCommand, SpawnExecution, TaskExecution,
    };

    use super::*;

    /// Create a dummy `AbsolutePath` for test fixtures.
    fn test_path() -> Arc<AbsolutePath> {
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
    fn test_task_display(name: &str) -> TaskDisplay {
        TaskDisplay {
            package_name: "pkg".into(),
            task_name: name.into(),
            package_path: test_path(),
        }
    }

    /// Create a dummy `ExecutionItemDisplay` for test fixtures.
    fn test_display(name: &str) -> ExecutionItemDisplay {
        ExecutionItemDisplay {
            task_display: test_task_display(name),
            command: name.into(),
            and_item_index: None,
            cwd: test_path(),
        }
    }

    /// Create a `TaskExecution` with a single spawn leaf.
    fn spawn_task(name: &str) -> TaskExecution {
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
    fn in_process_task(name: &str) -> TaskExecution {
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
    fn expanded_task(name: &str, nested_graph: ExecutionGraph) -> TaskExecution {
        TaskExecution {
            task_display: test_task_display(name),
            items: vec![ExecutionItem {
                execution_item_display: test_display(name),
                kind: ExecutionItemKind::Expanded(nested_graph),
            }],
        }
    }

    // ────────────────────────────────────────────────────────────
    // count_spawn_leaves tests
    // ────────────────────────────────────────────────────────────

    #[test]
    fn count_spawn_leaves_empty_graph() {
        let graph = ExecutionGraph::default();
        assert_eq!(count_spawn_leaves(&graph), 0);
    }

    #[test]
    fn count_spawn_leaves_single_spawn() {
        let graph = ExecutionGraph::from_node_list([spawn_task("build")]);
        assert_eq!(count_spawn_leaves(&graph), 1);
    }

    #[test]
    fn count_spawn_leaves_multiple_spawns() {
        let graph = ExecutionGraph::from_node_list([
            spawn_task("build"),
            spawn_task("test"),
            spawn_task("lint"),
        ]);
        assert_eq!(count_spawn_leaves(&graph), 3);
    }

    #[test]
    fn count_spawn_leaves_in_process_not_counted() {
        let graph = ExecutionGraph::from_node_list([in_process_task("echo")]);
        assert_eq!(count_spawn_leaves(&graph), 0);
    }

    #[test]
    fn count_spawn_leaves_mixed_spawn_and_in_process() {
        let graph = ExecutionGraph::from_node_list([spawn_task("build"), in_process_task("echo")]);
        assert_eq!(count_spawn_leaves(&graph), 1);
    }

    #[test]
    fn count_spawn_leaves_nested_expanded() {
        // Build a nested graph containing two spawns
        let nested =
            ExecutionGraph::from_node_list([spawn_task("nested-build"), spawn_task("nested-test")]);

        // Outer graph has one expanded item pointing to the nested graph
        let graph = ExecutionGraph::from_node_list([expanded_task("expand", nested)]);
        assert_eq!(count_spawn_leaves(&graph), 2);
    }

    #[test]
    fn count_spawn_leaves_nested_with_top_level() {
        // Nested graph with one spawn
        let nested = ExecutionGraph::from_node_list([spawn_task("nested-lint")]);

        // Top-level graph has one spawn + one expanded
        let graph =
            ExecutionGraph::from_node_list([spawn_task("build"), expanded_task("expand", nested)]);
        assert_eq!(count_spawn_leaves(&graph), 2);
    }

    // ────────────────────────────────────────────────────────────
    // PlainReporter stdin_suggestion tests
    // ────────────────────────────────────────────────────────────

    #[test]
    fn plain_reporter_always_suggests_inherited() {
        let reporter = PlainReporter::new(Vec::<u8>::new(), false);
        assert_eq!(reporter.stdin_suggestion(), StdinSuggestion::Inherited);
    }

    #[test]
    fn plain_reporter_suggests_inherited_even_when_silent() {
        let reporter = PlainReporter::new(Vec::<u8>::new(), true);
        assert_eq!(reporter.stdin_suggestion(), StdinSuggestion::Inherited);
    }

    // ────────────────────────────────────────────────────────────
    // LabeledLeafReporter stdin_suggestion tests
    // ────────────────────────────────────────────────────────────

    /// Build a `LabeledGraphReporter` for the given graph and return a leaf reporter
    /// for the first node's first item.
    fn build_labeled_leaf(graph: ExecutionGraph) -> Box<dyn LeafExecutionReporter> {
        let graph_arc = Arc::new(graph);
        let builder = Box::new(LabeledReporterBuilder::new(Vec::<u8>::new(), test_path()));
        let mut reporter = builder.build(&graph_arc);

        // Create a leaf reporter for the first node
        let path = LeafExecutionPath::default();
        reporter.new_leaf_execution(&path)
    }

    #[test]
    fn labeled_reporter_single_spawn_suggests_inherited() {
        let graph = ExecutionGraph::from_node_list([spawn_task("build")]);
        let leaf = build_labeled_leaf(graph);
        assert_eq!(leaf.stdin_suggestion(), StdinSuggestion::Inherited);
    }

    #[test]
    fn labeled_reporter_multiple_spawns_suggests_null() {
        let graph = ExecutionGraph::from_node_list([spawn_task("build"), spawn_task("test")]);
        let leaf = build_labeled_leaf(graph);
        assert_eq!(leaf.stdin_suggestion(), StdinSuggestion::Null);
    }

    #[test]
    fn labeled_reporter_single_in_process_suggests_inherited() {
        // Zero spawn leaves → spawn_leaf_count == 0, so not == 1 → Null
        // This is correct: in-process executions don't spawn child processes,
        // so stdin suggestion doesn't apply to them.
        let graph = ExecutionGraph::from_node_list([in_process_task("echo")]);
        let leaf = build_labeled_leaf(graph);
        assert_eq!(leaf.stdin_suggestion(), StdinSuggestion::Null);
    }

    #[test]
    fn labeled_reporter_one_spawn_one_in_process_suggests_inherited() {
        // One spawn leaf + one in-process → spawn_leaf_count == 1 → Inherited
        let graph = ExecutionGraph::from_node_list([spawn_task("build"), in_process_task("echo")]);
        let leaf = build_labeled_leaf(graph);
        assert_eq!(leaf.stdin_suggestion(), StdinSuggestion::Inherited);
    }

    #[test]
    fn labeled_reporter_nested_single_spawn_suggests_inherited() {
        // Nested graph with exactly one spawn
        let nested = ExecutionGraph::from_node_list([spawn_task("nested-build")]);

        let graph = ExecutionGraph::from_node_list([expanded_task("expand", nested)]);
        let leaf = build_labeled_leaf(graph);
        assert_eq!(leaf.stdin_suggestion(), StdinSuggestion::Inherited);
    }

    #[test]
    fn labeled_reporter_nested_multiple_spawns_suggests_null() {
        // Nested graph with two spawns
        let nested =
            ExecutionGraph::from_node_list([spawn_task("nested-a"), spawn_task("nested-b")]);

        let graph = ExecutionGraph::from_node_list([expanded_task("expand", nested)]);
        let leaf = build_labeled_leaf(graph);
        assert_eq!(leaf.stdin_suggestion(), StdinSuggestion::Null);
    }
}
