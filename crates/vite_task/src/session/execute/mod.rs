pub mod fingerprint;
pub mod glob_inputs;
mod hash;
pub mod spawn;

use std::{cell::RefCell, collections::BTreeMap, io::Write as _, process::Stdio, sync::Arc};

use futures_util::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use petgraph::Direction;
use rustc_hash::FxHashMap;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use vite_path::AbsolutePath;
use vite_task_plan::{
    ExecutionGraph, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind, SpawnCommand,
    SpawnExecution, execution_graph::ExecutionNodeIndex,
};

use self::{
    fingerprint::PostRunFingerprint,
    glob_inputs::compute_globbed_inputs,
    spawn::{SpawnResult, TrackedPathAccesses, spawn_with_tracking},
};
use super::{
    cache::{CacheEntryValue, ExecutionCache},
    event::{
        CacheDisabledReason, CacheErrorKind, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus,
        ExecutionError,
    },
    reporter::{
        ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder, LeafExecutionReporter,
        StdioSuggestion,
    },
};
use crate::{Session, collections::HashMap};

/// Outcome of a spawned execution.
///
/// Returned by [`execute_spawn`] to communicate what happened. Errors are
/// already reported through `LeafExecutionReporter::finish()` before this
/// value is returned — the caller does not need to handle error display.
pub enum SpawnOutcome {
    /// Cache hit — no process was spawned. Cached outputs were replayed.
    CacheHit,
    /// Process was spawned and exited with this status.
    Spawned(std::process::ExitStatus),
    /// An infrastructure error prevented the process from running
    /// (cache lookup failure or spawn failure).
    /// Already reported through the leaf reporter.
    Failed,
}

/// Holds shared references needed during graph execution.
///
/// The `reporter` field is wrapped in `RefCell` because concurrent futures
/// (via `FuturesUnordered`) need shared access to create leaf reporters.
/// Since all futures run on a single thread (no `tokio::spawn`), `RefCell`
/// is sufficient for interior mutability.
///
/// Cache fields are passed through to [`execute_spawn`] for cache-aware execution.
struct ExecutionContext<'a> {
    /// The graph-level reporter, used to create leaf reporters via `new_leaf_execution()`.
    /// Wrapped in `RefCell` for shared access from concurrent task futures.
    reporter: &'a RefCell<Box<dyn GraphExecutionReporter>>,
    /// The execution cache for looking up and storing cached results.
    cache: &'a ExecutionCache,
    /// Base path for resolving relative paths in cache entries.
    /// Typically the workspace root.
    cache_base_path: &'a Arc<AbsolutePath>,
    /// Token cancelled when a task fails. Kills in-flight child processes
    /// (via `start_kill` in spawn.rs), prevents scheduling new tasks, and
    /// prevents caching results of concurrently-running tasks.
    fast_fail_token: CancellationToken,
    /// Token cancelled by Ctrl-C. Unlike `fast_fail_token` (which kills
    /// children), this only prevents scheduling new tasks and caching
    /// results — running processes are left to handle SIGINT naturally.
    interrupt_token: CancellationToken,
}

impl ExecutionContext<'_> {
    /// Returns true if execution has been cancelled, either by a task
    /// failure (fast-fail) or by Ctrl-C (interrupt).
    fn cancelled(&self) -> bool {
        self.fast_fail_token.is_cancelled() || self.interrupt_token.is_cancelled()
    }

    /// Execute all tasks in an execution graph concurrently, respecting dependencies.
    ///
    /// Uses a DAG scheduler: tasks whose dependencies have all completed are scheduled
    /// onto a `FuturesUnordered`, bounded by a per-graph `Semaphore` with
    /// `concurrency_limit` permits. Each recursive `Expanded` graph creates its own
    /// semaphore, so nested graphs have independent concurrency limits.
    ///
    /// Fast-fail: if any task fails, `execute_leaf` cancels the `fast_fail_token`
    /// (killing in-flight child processes). Ctrl-C cancels the `interrupt_token`.
    /// Either cancellation causes this method to close the semaphore, drain
    /// remaining futures, and return.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute_expanded_graph(&self, graph: &ExecutionGraph) {
        if graph.graph.node_count() == 0 {
            return;
        }

        let semaphore =
            Arc::new(Semaphore::new(graph.concurrency_limit.min(Semaphore::MAX_PERMITS)));

        // Compute dependency count for each node.
        // Edge A→B means "A depends on B", so A's dependency count = outgoing edge count.
        let mut dep_count: FxHashMap<ExecutionNodeIndex, usize> = FxHashMap::default();
        for node_ix in graph.graph.node_indices() {
            dep_count.insert(node_ix, graph.graph.neighbors(node_ix).count());
        }

        let mut futures = FuturesUnordered::new();

        // Schedule initially ready nodes (no dependencies).
        for (&node_ix, &count) in &dep_count {
            if count == 0 {
                futures.push(self.spawn_node(graph, node_ix, &semaphore));
            }
        }

        // Process completions and schedule newly ready dependents.
        // On failure, `execute_leaf` cancels the token — we detect it here, close
        // the semaphore (so pending acquires fail immediately), and drain.
        while let Some(completed_ix) = futures.next().await {
            if self.cancelled() {
                semaphore.close();
                while futures.next().await.is_some() {}
                return;
            }

            // Find dependents of the completed node (nodes that depend on it).
            // Edge X→completed means "X depends on completed", so X is a predecessor
            // in graph direction = neighbor in Incoming direction.
            for dependent in graph.graph.neighbors_directed(completed_ix, Direction::Incoming) {
                let count = dep_count.get_mut(&dependent).expect("all nodes are in dep_count");
                *count -= 1;
                if *count == 0 {
                    futures.push(self.spawn_node(graph, dependent, &semaphore));
                }
            }
        }
    }

    /// Create a future that acquires a semaphore permit, then executes a graph node.
    ///
    /// On failure, `execute_node` cancels the `fast_fail_token` — the caller
    /// detects this after the future completes. On semaphore closure or prior
    /// cancellation, the node is skipped.
    fn spawn_node<'a>(
        &'a self,
        graph: &'a ExecutionGraph,
        node_ix: ExecutionNodeIndex,
        semaphore: &Arc<Semaphore>,
    ) -> LocalBoxFuture<'a, ExecutionNodeIndex> {
        let sem = semaphore.clone();
        async move {
            if let Ok(_permit) = sem.acquire_owned().await
                && !self.cancelled()
            {
                self.execute_node(graph, node_ix).await;
            }
            node_ix
        }
        .boxed_local()
    }

    /// Execute a single node's items sequentially.
    ///
    /// A node may have multiple items (from `&&`-split commands). Items are executed
    /// in order; if any item fails, `execute_leaf` cancels the `fast_fail_token`
    /// and remaining items are skipped (preserving `&&` semantics).
    async fn execute_node(&self, graph: &ExecutionGraph, node_ix: ExecutionNodeIndex) {
        let task_execution = &graph.graph[node_ix];

        for item in &task_execution.items {
            if self.cancelled() {
                return;
            }
            match &item.kind {
                ExecutionItemKind::Leaf(leaf_kind) => {
                    self.execute_leaf(&item.execution_item_display, leaf_kind).boxed_local().await;
                }
                ExecutionItemKind::Expanded(nested_graph) => {
                    self.execute_expanded_graph(nested_graph).boxed_local().await;
                }
            }
        }
    }

    /// Execute a single leaf item (in-process command or spawned process).
    ///
    /// Creates a [`LeafExecutionReporter`] from the graph reporter and delegates
    /// to the appropriate execution method. On failure (non-zero exit or
    /// infrastructure error), cancels the `fast_fail_token`.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute_leaf(&self, display: &ExecutionItemDisplay, leaf_kind: &LeafExecutionKind) {
        // Borrow the reporter briefly to create the leaf reporter, then drop
        // the RefCell guard before any `.await` point.
        let mut leaf_reporter = self.reporter.borrow_mut().new_leaf_execution(display, leaf_kind);

        let failed = match leaf_kind {
            LeafExecutionKind::InProcess(in_process_execution) => {
                // In-process (built-in) commands: caching is disabled, execute synchronously
                let mut stdio_config = leaf_reporter
                    .start(CacheStatus::Disabled(CacheDisabledReason::InProcessExecution));

                let execution_output = in_process_execution.execute();
                // Write output to the stdout writer from StdioConfig
                let _ = stdio_config.stdout_writer.write_all(&execution_output.stdout);
                let _ = stdio_config.stdout_writer.flush();

                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    None,
                );
                false
            }
            LeafExecutionKind::Spawn(spawn_execution) => {
                #[expect(
                    clippy::large_futures,
                    reason = "spawn execution with cache management creates large futures"
                )]
                let outcome = execute_spawn(
                    leaf_reporter,
                    spawn_execution,
                    self.cache,
                    self.cache_base_path,
                    self.fast_fail_token.clone(),
                    self.interrupt_token.clone(),
                )
                .await;
                match outcome {
                    SpawnOutcome::CacheHit => false,
                    SpawnOutcome::Spawned(status) => !status.success(),
                    SpawnOutcome::Failed => true,
                }
            }
        };
        if failed {
            self.fast_fail_token.cancel();
        }
    }
}

/// Execute a spawned process with cache-aware lifecycle.
///
/// This is a free function (not tied to `ExecutionContext`) so it can be reused
/// from both graph-based execution and standalone synthetic execution.
///
/// The full lifecycle is:
/// 1. Cache lookup (determines cache status)
/// 2. `leaf_reporter.start(cache_status)` → `StdioConfig`
/// 3. If cache hit: replay cached outputs via `StdioConfig` writers → finish
/// 4. If `Inherited` suggestion AND caching disabled: `spawn_inherited()` → finish
/// 5. Else (piped): `spawn_with_tracking()` with writers → cache update → finish
///
/// Errors (cache lookup failure, spawn failure, cache update failure) are reported
/// through `leaf_reporter.finish()` and do not abort the caller.
#[tracing::instrument(level = "debug", skip_all)]
#[expect(
    clippy::too_many_lines,
    reason = "sequential cache check, execute, and update steps are clearer in one function"
)]
pub async fn execute_spawn(
    mut leaf_reporter: Box<dyn LeafExecutionReporter>,
    spawn_execution: &SpawnExecution,
    cache: &ExecutionCache,
    cache_base_path: &Arc<AbsolutePath>,
    fast_fail_token: CancellationToken,
    interrupt_token: CancellationToken,
) -> SpawnOutcome {
    let cache_metadata = spawn_execution.cache_metadata.as_ref();

    // 1. Determine cache status FIRST by trying cache hit.
    //    We need to know the status before calling start() so the reporter
    //    can display cache status immediately when execution begins.
    let (cache_status, cached_value, globbed_inputs) = if let Some(cache_metadata) = cache_metadata
    {
        // Compute globbed inputs from positive globs at execution time
        // Globs are already workspace-root-relative (resolved at task graph stage)
        let globbed_inputs = match compute_globbed_inputs(
            cache_base_path,
            &cache_metadata.input_config.positive_globs,
            &cache_metadata.input_config.negative_globs,
        ) {
            Ok(inputs) => inputs,
            Err(err) => {
                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    Some(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }),
                );
                return SpawnOutcome::Failed;
            }
        };

        match cache.try_hit(cache_metadata, &globbed_inputs, cache_base_path).await {
            Ok(Ok(cached)) => (
                // Cache hit — we can replay the cached outputs
                CacheStatus::Hit { replayed_duration: cached.duration },
                Some(cached),
                globbed_inputs,
            ),
            Ok(Err(cache_miss)) => (
                // Cache miss — includes detailed reason (NotFound or FingerprintMismatch)
                CacheStatus::Miss(cache_miss),
                None,
                globbed_inputs,
            ),
            Err(err) => {
                // Cache lookup error — report through finish.
                // Note: start() is NOT called because we don't have a valid cache status.
                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    Some(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }),
                );
                return SpawnOutcome::Failed;
            }
        }
    } else {
        // No cache metadata provided — caching is disabled for this task
        (CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata), None, BTreeMap::new())
    };

    // 2. Report execution start with the determined cache status.
    //    Returns StdioConfig with the reporter's suggestion and writers.
    let mut stdio_config = leaf_reporter.start(cache_status);

    // 3. If cache hit, replay outputs via the StdioConfig writers and finish early.
    //    No need to actually execute the command — just replay what was cached.
    if let Some(cached) = cached_value {
        for output in cached.std_outputs.iter() {
            let writer: &mut dyn std::io::Write = match output.kind {
                spawn::OutputKind::StdOut => &mut stdio_config.stdout_writer,
                spawn::OutputKind::StdErr => &mut stdio_config.stderr_writer,
            };
            let _ = writer.write_all(&output.content);
            let _ = writer.flush();
        }
        leaf_reporter.finish(
            None,
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheHit),
            None,
        );
        return SpawnOutcome::CacheHit;
    }

    // 4. Determine actual stdio mode based on the suggestion AND cache state.
    //    Inherited stdio is only used when the reporter suggests it AND caching is
    //    completely disabled (no cache_metadata). If caching is enabled but missed,
    //    we still need piped mode to capture output for the cache update.
    let use_inherited =
        stdio_config.suggestion == StdioSuggestion::Inherited && cache_metadata.is_none();

    if use_inherited {
        // Inherited mode: all three stdio FDs (stdin, stdout, stderr) are inherited
        // from the parent process. No fspy tracking, no output capture.
        // Drop the StdioConfig writers before spawning to avoid holding std::io::Stdout
        // while the child also writes to the same FD.
        drop(stdio_config);

        match spawn_inherited(&spawn_execution.spawn_command, fast_fail_token).await {
            Ok(result) => {
                leaf_reporter.finish(
                    Some(result.exit_status),
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    None,
                );
                return SpawnOutcome::Spawned(result.exit_status);
            }
            Err(err) => {
                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    Some(ExecutionError::Spawn(err)),
                );
                return SpawnOutcome::Failed;
            }
        }
    }

    // 5. Piped mode: execute spawn with tracking, streaming output to writers.
    //    - std_outputs: always captured when caching is enabled (for cache replay)
    //    - path_accesses: only tracked when includes_auto is true (fspy inference)
    let (mut std_outputs, mut path_accesses, cache_metadata_and_inputs) =
        cache_metadata.map_or((None, None, None), |cache_metadata| {
            // On musl targets, LD_PRELOAD-based tracking is unavailable but seccomp
            // unotify provides equivalent file access tracing.
            let path_accesses = if cache_metadata.input_config.includes_auto {
                Some(TrackedPathAccesses::default())
            } else {
                None // Skip fspy when inference is disabled or unavailable
            };
            (Some(Vec::new()), path_accesses, Some((cache_metadata, globbed_inputs)))
        });

    // Build negative globs for fspy path filtering (already workspace-root-relative)
    let resolved_negatives: Vec<wax::Glob<'static>> =
        if let Some((cache_metadata, _)) = &cache_metadata_and_inputs {
            match cache_metadata
                .input_config
                .negative_globs
                .iter()
                .map(|p| Ok(wax::Glob::new(p.as_str())?.into_owned()))
                .collect::<anyhow::Result<Vec<_>>>()
            {
                Ok(negs) => negs,
                Err(err) => {
                    leaf_reporter.finish(
                        None,
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                        Some(ExecutionError::PostRunFingerprint(err)),
                    );
                    return SpawnOutcome::Failed;
                }
            }
        } else {
            Vec::new()
        };

    #[expect(
        clippy::large_futures,
        reason = "spawn_with_tracking manages process I/O and creates a large future"
    )]
    let result = match spawn_with_tracking(
        &spawn_execution.spawn_command,
        cache_base_path,
        &mut *stdio_config.stdout_writer,
        &mut *stdio_config.stderr_writer,
        std_outputs.as_mut(),
        path_accesses.as_mut(),
        &resolved_negatives,
        fast_fail_token.clone(),
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            leaf_reporter.finish(
                None,
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Spawn(err)),
            );
            return SpawnOutcome::Failed;
        }
    };

    // 6. Update cache if successful and determine cache update status.
    //    Errors during cache update are terminal (reported through finish).
    let (cache_update_status, cache_error) = if let Some((cache_metadata, globbed_inputs)) =
        cache_metadata_and_inputs
    {
        let cancelled = fast_fail_token.is_cancelled() || interrupt_token.is_cancelled();
        if cancelled {
            // Cancelled (Ctrl-C or sibling failure) — result is untrustworthy
            (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::Cancelled), None)
        } else if result.exit_status.success() {
            // Check for read-write overlap: if the task wrote to any file it also
            // read, the inputs were modified during execution — don't cache.
            // Note: this only checks fspy-inferred reads, not globbed_inputs keys.
            // A task that writes to a glob-matched file without reading it causes
            // perpetual cache misses (glob detects the hash change) but not a
            // correctness bug, so we don't handle that case here.
            if let Some(path) = path_accesses
                .as_ref()
                .and_then(|pa| pa.path_reads.keys().find(|p| pa.path_writes.contains(*p)))
            {
                (
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::InputModified {
                        path: path.clone(),
                    }),
                    None,
                )
            } else {
                // path_reads is empty when inference is disabled (path_accesses is None)
                let empty_path_reads = HashMap::default();
                let path_reads =
                    path_accesses.as_ref().map_or(&empty_path_reads, |pa| &pa.path_reads);

                // Execution succeeded — attempt to create fingerprint and update cache.
                // Paths already in globbed_inputs are skipped: Rule 1 (above) guarantees
                // no input modification, so the prerun hash is the correct post-exec hash.
                match PostRunFingerprint::create(path_reads, cache_base_path, &globbed_inputs) {
                    Ok(post_run_fingerprint) => {
                        let new_cache_value = CacheEntryValue {
                            post_run_fingerprint,
                            std_outputs: std_outputs.unwrap_or_default().into(),
                            duration: result.duration,
                            globbed_inputs,
                        };
                        match cache.update(cache_metadata, new_cache_value).await {
                            Ok(()) => (CacheUpdateStatus::Updated, None),
                            Err(err) => (
                                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                                Some(ExecutionError::Cache {
                                    kind: CacheErrorKind::Update,
                                    source: err,
                                }),
                            ),
                        }
                    }
                    Err(err) => (
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                        Some(ExecutionError::PostRunFingerprint(err)),
                    ),
                }
            }
        } else {
            // Execution failed with non-zero exit status — don't update cache
            (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::NonZeroExitStatus), None)
        }
    } else {
        // Caching was disabled for this task
        (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled), None)
    };

    // 7. Finish the leaf execution with the result and optional cache error.
    //    Cache update/fingerprint failures are reported but do not affect the outcome —
    //    the process ran, so we return its actual exit status.
    leaf_reporter.finish(Some(result.exit_status), cache_update_status, cache_error);

    SpawnOutcome::Spawned(result.exit_status)
}

/// Spawn a command with all three stdio file descriptors inherited from the parent.
///
/// Used when the reporter suggests inherited stdio AND caching is disabled.
/// All three FDs (stdin, stdout, stderr) are inherited, allowing interactive input
/// and direct terminal output. No fspy tracking is performed since there's no
/// cache to update.
///
/// The child process will see `is_terminal() == true` for stdout/stderr when the
/// parent is running in a terminal. This is expected behavior.
#[tracing::instrument(level = "debug", skip_all)]
async fn spawn_inherited(
    spawn_command: &SpawnCommand,
    fast_fail_token: CancellationToken,
) -> anyhow::Result<SpawnResult> {
    let mut cmd = fspy::Command::new(spawn_command.program_path.as_path());
    cmd.args(spawn_command.args.iter().map(vite_str::Str::as_str));
    cmd.envs(spawn_command.all_envs.iter());
    cmd.current_dir(&*spawn_command.cwd);
    cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());

    let start = std::time::Instant::now();
    let mut tokio_cmd = cmd.into_tokio_command();

    // Clear FD_CLOEXEC on stdio fds before exec. libuv (used by Node.js) marks
    // stdin/stdout/stderr as close-on-exec, which causes them to be closed when
    // the child process calls exec(). Without this fix, the child's fds 0-2 are
    // closed after exec and Node.js reopens them as /dev/null, losing all output.
    // See: https://github.com/libuv/libuv/issues/2062
    // SAFETY: The pre_exec closure only performs fcntl operations to clear
    // FD_CLOEXEC flags on stdio fds, which is safe in a post-fork context.
    #[cfg(unix)]
    unsafe {
        tokio_cmd.pre_exec(|| {
            use std::os::fd::BorrowedFd;

            use nix::{
                fcntl::{FcntlArg, FdFlag, fcntl},
                libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO},
            };
            for fd in [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO] {
                // SAFETY: fds 0-2 are always valid in a post-fork context
                let borrowed = BorrowedFd::borrow_raw(fd);
                if let Ok(flags) = fcntl(borrowed, FcntlArg::F_GETFD) {
                    let mut fd_flags = FdFlag::from_bits_retain(flags);
                    if fd_flags.contains(FdFlag::FD_CLOEXEC) {
                        fd_flags.remove(FdFlag::FD_CLOEXEC);
                        let _ = fcntl(borrowed, FcntlArg::F_SETFD(fd_flags));
                    }
                }
            }
            Ok(())
        });
    }

    let mut child = tokio_cmd.spawn()?;

    // On Windows, assign the child to a Job Object with KILL_ON_JOB_CLOSE so that
    // all descendant processes (e.g., node.exe spawned by a .cmd shim) are killed
    // when the job handle is dropped. Without this, TerminateProcess only kills the
    // direct child, leaving grandchildren alive.
    #[cfg(windows)]
    let _job = {
        use std::os::windows::io::{AsRawHandle, BorrowedHandle};
        // Duplicate the process handle so the job outlives tokio's handle.
        // SAFETY: The child was just spawned, so its raw handle is valid.
        let borrowed = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) };
        let owned = borrowed.try_clone_to_owned()?;
        win_job::assign_to_kill_on_close_job(owned.as_raw_handle())?
    };

    let exit_status = tokio::select! {
        status = child.wait() => status?,
        () = fast_fail_token.cancelled() => {
            child.start_kill()?;
            child.wait().await?
        }
    };

    Ok(SpawnResult { exit_status, duration: start.elapsed() })
}

/// Win32 Job Object utilities for process tree management.
///
/// On Windows, `TerminateProcess` only kills the direct child process, not its
/// descendants. This module creates a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
/// which automatically terminates all processes in the job when the handle is dropped.
#[cfg(windows)]
mod win_job {
    use std::{io, os::windows::io::RawHandle};

    use winapi::{
        shared::minwindef::FALSE,
        um::{
            handleapi::CloseHandle,
            jobapi2::{
                AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
                TerminateJobObject,
            },
            winnt::{
                HANDLE, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            },
        },
    };

    /// RAII wrapper around a Win32 Job Object `HANDLE` that closes it on drop.
    pub(super) struct OwnedJobHandle(HANDLE);

    impl OwnedJobHandle {
        /// Immediately terminate all processes in the job.
        ///
        /// This is needed when pipes to a grandchild process must be closed before
        /// the job handle is dropped (e.g., to unblock pipe reads in `spawn_with_tracking`).
        pub(super) fn terminate(&self) {
            // SAFETY: self.0 is a valid job handle from CreateJobObjectW.
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for OwnedJobHandle {
        fn drop(&mut self) {
            // SAFETY: self.0 is a valid handle obtained from CreateJobObjectW.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Create a Job Object with `KILL_ON_JOB_CLOSE` and assign a process to it.
    ///
    /// Returns the job handle wrapped in an RAII guard. When dropped, all processes
    /// in the job (the child and its descendants) are terminated.
    pub(super) fn assign_to_kill_on_close_job(
        process_handle: RawHandle,
    ) -> io::Result<OwnedJobHandle> {
        // SAFETY: Creating an anonymous job object with no security attributes.
        let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedJobHandle(job);

        // Configure the job to kill all processes when the handle is closed.
        // SAFETY: JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a plain C struct (no pointers
        // in the zeroed fields). Zeroing then setting LimitFlags is the standard pattern.
        let mut info = unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            info
        };

        // SAFETY: info is a valid JOBOBJECT_EXTENDED_LIMIT_INFORMATION, job.0 is a valid handle.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                // JobObjectExtendedLimitInformation = 9
                9,
                std::ptr::from_mut(&mut info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>().try_into().unwrap(),
            )
        };
        if ok == FALSE {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: Both handles are valid — job from CreateJobObjectW, process handle
        // from the caller.
        let ok = unsafe { AssignProcessToJobObject(job.0, process_handle as HANDLE) };
        if ok == FALSE {
            return Err(io::Error::last_os_error());
        }

        Ok(job)
    }
}

impl Session<'_> {
    /// Execute an execution graph, reporting events through the provided reporter builder.
    ///
    /// Cache is initialized only if any leaf execution needs it. The reporter is built
    /// after cache initialization, so cache errors are reported directly to stderr
    /// without involving the reporter at all.
    ///
    /// Returns `Err(ExitStatus)` to indicate the caller should exit with the given status code.
    /// Returns `Ok(())` when all tasks succeeded.
    #[tracing::instrument(level = "debug", skip_all)]
    pub(crate) async fn execute_graph(
        &self,
        execution_graph: ExecutionGraph,
        builder: Box<dyn GraphExecutionReporterBuilder>,
        interrupt_token: CancellationToken,
    ) -> Result<(), ExitStatus> {
        // Initialize cache before building the reporter. Cache errors are reported
        // directly to stderr and cause an early exit, keeping the reporter flow clean
        // (the reporter's `finish()` no longer accepts graph-level error messages).
        let cache = match self.cache() {
            Ok(cache) => cache,
            #[expect(clippy::print_stderr, reason = "cache init errors bypass the reporter")]
            Err(err) => {
                eprintln!("Failed to initialize cache: {err}");
                return Err(ExitStatus::FAILURE);
            }
        };

        let reporter = RefCell::new(builder.build());

        let execution_context = ExecutionContext {
            reporter: &reporter,
            cache,
            cache_base_path: &self.workspace_path,
            fast_fail_token: CancellationToken::new(),
            interrupt_token,
        };

        // Execute the graph with fast-fail: if any task fails, remaining tasks
        // are skipped. Leaf-level errors are reported through the reporter.
        execution_context.execute_expanded_graph(&execution_graph).await;

        // Leaf-level errors and non-zero exit statuses are tracked internally
        // by the reporter.
        reporter.into_inner().finish()
    }
}
