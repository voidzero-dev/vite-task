pub mod fingerprint;
pub mod spawn;

use std::sync::Arc;

use futures_util::FutureExt;
use petgraph::{algo::toposort, stable_graph::StableGraph};
use vite_path::AbsolutePath;
use vite_task_plan::{
    ExecutionItemKind, ExecutionPlan, LeafExecutionKind, SpawnExecution, TaskExecution,
    execution_graph::ExecutionIx,
};

use self::{
    fingerprint::PostRunFingerprint,
    spawn::{OutputKind as SpawnOutputKind, spawn_with_tracking},
};
use super::{
    cache::{CommandCacheValue, ExecutionCache},
    event::{
        CacheDisabledReason, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus, ExecutionEvent,
        ExecutionEventKind, ExecutionId, ExecutionItemDisplay, OutputKind,
    },
    reporter::{ExitStatus, Reporter},
};
use crate::{Session, session::execute::spawn::SpawnTrackResult};

/// Internal error type used to abort execution when errors occur.
/// This error is swallowed in Session::execute and never exposed externally.
#[derive(Debug)]
struct ExecutionAborted;

struct ExecutionContext<'a> {
    event_handler: &'a mut dyn Reporter,
    current_execution_id: ExecutionId,
    cache: &'a ExecutionCache,
    /// All relative paths in cache are relative to this base path
    cache_base_path: &'a Arc<AbsolutePath>,
}

impl ExecutionContext<'_> {
    async fn execute_item_kind(
        &mut self,
        display: Option<&ExecutionItemDisplay>,
        item_kind: &ExecutionItemKind,
    ) -> Result<(), ExecutionAborted> {
        match item_kind {
            ExecutionItemKind::Expanded(graph) => {
                // Use StableGraph to preserve node indices during removal
                let mut graph: StableGraph<&TaskExecution, (), _, ExecutionIx> =
                    graph.map(|_, task_execution| task_execution, |_, ()| ()).into();

                // To be consistent with the package graph in vite_package_manager and the dependency graph definition in Wikipedia
                // https://en.wikipedia.org/wiki/Dependency_graph, we construct the graph with edges from dependents to dependencies
                // e.g. A -> B means A depends on B
                //
                // For execution we need to reverse the edges first before topological sorting,
                // so that tasks without dependencies are executed first
                graph.reverse(); // Run tasks without dependencies first

                // Always use topological sort to ensure the correct order of execution
                // or the task dependencies declaration is meaningless
                let node_indices = match toposort(&graph, None) {
                    Ok(ok) => ok,
                    Err(cycle) => {
                        // Follow standard error pattern: Start event, then Error event
                        let execution_id = self.current_execution_id;
                        self.current_execution_id = self.current_execution_id.next();

                        // Emit Start event for cycle detection error
                        // display is None for top-level execution (no parent task)
                        // display is Some for nested execution (within a parent task)
                        // Caching is disabled when cycle dependencies are detected
                        self.event_handler.handle_event(ExecutionEvent {
                            execution_id,
                            kind: ExecutionEventKind::Start {
                                display: display.cloned(),
                                cache_status: CacheStatus::Disabled(
                                    CacheDisabledReason::CycleDetected,
                                ),
                            },
                        });

                        self.event_handler.handle_event(ExecutionEvent {
                            execution_id,
                            kind: ExecutionEventKind::Error {
                                message: format!("Cycle dependencies detected: {cycle:?}"),
                            },
                        });

                        return Err(ExecutionAborted);
                    }
                };

                let ordered_executions =
                    node_indices.into_iter().map(|id| graph.remove_node(id).unwrap());
                for task_execution in ordered_executions {
                    for item in task_execution.items.iter() {
                        match &item.kind {
                            ExecutionItemKind::Leaf(leaf_kind) => {
                                self.execute_leaf(Some(&item.execution_item_display), leaf_kind)
                                    .boxed_local()
                                    .await?;
                            }
                            ExecutionItemKind::Expanded(_) => {
                                self.execute_item_kind(
                                    Some(&item.execution_item_display),
                                    &item.kind,
                                )
                                .boxed_local()
                                .await?;
                            }
                        }
                    }
                }
            }
            ExecutionItemKind::Leaf(leaf_execution_kind) => {
                self.execute_leaf(display, leaf_execution_kind).await?;
            }
        }
        Ok(())
    }

    async fn execute_leaf(
        &mut self,
        display: Option<&ExecutionItemDisplay>,
        leaf_execution_kind: &LeafExecutionKind,
    ) -> Result<(), ExecutionAborted> {
        let execution_id = self.current_execution_id;
        self.current_execution_id = self.current_execution_id.next();

        match leaf_execution_kind {
            LeafExecutionKind::InProcess(in_process_execution) => {
                // Emit Start event with cache_status for in-process (built-in) commands
                // Caching is disabled for built-in commands
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Start {
                        display: display.cloned(),
                        cache_status: CacheStatus::Disabled(
                            CacheDisabledReason::InProcessExecution,
                        ),
                    },
                });

                // Execute the in-process command
                let execution_output = in_process_execution.execute().await;
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Output {
                        kind: OutputKind::Stdout,
                        content: execution_output.stdout.into(),
                    },
                });

                // Emit Finish with CacheDisabled status (in-process executions don't cache)
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Finish {
                        status: None,
                        cache_update_status: CacheUpdateStatus::NotUpdated(
                            CacheNotUpdatedReason::CacheDisabled,
                        ),
                    },
                });
            }
            LeafExecutionKind::Spawn(spawn_execution) => {
                self.execute_spawn(execution_id, display, spawn_execution).await?;
            }
        }
        Ok(())
    }

    async fn execute_spawn(
        &mut self,
        execution_id: ExecutionId,
        display: Option<&ExecutionItemDisplay>,
        spawn_execution: &SpawnExecution,
    ) -> Result<(), ExecutionAborted> {
        let cache_metadata = spawn_execution.cache_metadata.as_ref();

        // 1. Determine cache status FIRST by trying cache hit
        //    We need to know the status before emitting Start event so users
        //    see cache status immediately when execution begins
        let (cache_status, cached_value) = if let Some(cache_metadata) = cache_metadata {
            match self.cache.try_hit(cache_metadata, &*self.cache_base_path).await {
                Ok(Ok(cached)) => (
                    // Cache hit - we can replay the cached outputs
                    CacheStatus::Hit { replayed_duration: cached.duration },
                    Some(cached),
                ),
                Ok(Err(cache_miss)) => (
                    // Cache miss - includes detailed reason (NotFound or FingerprintMismatch)
                    CacheStatus::Miss(cache_miss),
                    None,
                ),
                Err(err) => {
                    // Cache lookup error - emit error and abort
                    self.event_handler.handle_event(ExecutionEvent {
                        execution_id,
                        kind: ExecutionEventKind::Error {
                            message: format!("Cache lookup failed: {err}"),
                        },
                    });
                    return Err(ExecutionAborted);
                }
            }
        } else {
            // No cache metadata provided - caching is disabled for this task
            (CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata), None)
        };

        // 2. NOW emit Start event with cache_status (ALWAYS emit Start)
        //    This ensures all spawn executions emit Start, including cache hits
        //    (previously cache hits didn't emit Start at all)
        self.event_handler.handle_event(ExecutionEvent {
            execution_id,
            kind: ExecutionEventKind::Start { display: display.cloned(), cache_status },
        });

        // 3. If cache hit, replay outputs and return early
        //    No need to actually execute the command - just replay what was cached
        if let Some(cached) = cached_value {
            for output in cached.std_outputs.iter() {
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Output {
                        kind: match output.kind {
                            SpawnOutputKind::StdOut => OutputKind::Stdout,
                            SpawnOutputKind::StdErr => OutputKind::Stderr,
                        },
                        content: output.content.clone().into(),
                    },
                });
            }
            // Emit Finish with CacheHit status (no cache update needed)
            self.event_handler.handle_event(ExecutionEvent {
                execution_id,
                kind: ExecutionEventKind::Finish {
                    status: None,
                    cache_update_status: CacheUpdateStatus::NotUpdated(
                        CacheNotUpdatedReason::CacheHit,
                    ),
                },
            });
            return Ok(());
        }

        // 4. Execute spawn (cache miss or disabled)
        //    Track file system access if caching is enabled (for future cache updates)
        let mut track_result_with_cache_metadata = if let Some(cache_metadata) = cache_metadata {
            Some((SpawnTrackResult::default(), cache_metadata))
        } else {
            None
        };

        // Execute command with tracking, emitting output events in real-time
        let result = match spawn_with_tracking(
            &spawn_execution.spawn_command,
            &*self.cache_base_path,
            |kind, content| {
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Output {
                        kind: match kind {
                            SpawnOutputKind::StdOut => OutputKind::Stdout,
                            SpawnOutputKind::StdErr => OutputKind::Stderr,
                        },
                        content,
                    },
                });
            },
            track_result_with_cache_metadata.as_mut().map(|(track_result, _)| track_result),
        )
        .await
        {
            Ok(result) => result,
            Err(err) => {
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Error {
                        message: format!("Failed to spawn process: {err}"),
                    },
                });
                return Err(ExecutionAborted);
            }
        };

        // 5. Update cache if successful and determine cache update status
        let cache_update_status = if let Some((track_result, cache_metadata)) =
            track_result_with_cache_metadata
        {
            if result.exit_status.success() {
                // Execution succeeded, attempt cache update
                let fingerprint_ignores =
                    cache_metadata.spawn_fingerprint.fingerprint_ignores().map(|v| v.as_slice());
                match PostRunFingerprint::create(
                    &track_result.path_reads,
                    &*self.cache_base_path,
                    fingerprint_ignores,
                ) {
                    Ok(post_run_fingerprint) => {
                        let cache_value = CommandCacheValue {
                            post_run_fingerprint,
                            std_outputs: track_result.std_outputs.clone().into(),
                            duration: result.duration,
                        };
                        if let Err(err) = self.cache.update(cache_metadata, cache_value).await {
                            self.event_handler.handle_event(ExecutionEvent {
                                execution_id,
                                kind: ExecutionEventKind::Error {
                                    message: format!("Failed to update cache: {err}"),
                                },
                            });
                            return Err(ExecutionAborted);
                        }
                        CacheUpdateStatus::Updated
                    }
                    Err(err) => {
                        self.event_handler.handle_event(ExecutionEvent {
                            execution_id,
                            kind: ExecutionEventKind::Error {
                                message: format!("Failed to create post-run fingerprint: {err}"),
                            },
                        });
                        return Err(ExecutionAborted);
                    }
                }
            } else {
                // Execution failed with non-zero exit status, don't update cache
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::NonZeroExitStatus)
            }
        } else {
            // Caching was disabled for this task
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled)
        };

        // 6. Emit finish with cache_update_status
        self.event_handler.handle_event(ExecutionEvent {
            execution_id,
            kind: ExecutionEventKind::Finish {
                status: Some(result.exit_status),
                cache_update_status,
            },
        });

        Ok(())
    }
}

impl<'a> Session<'a> {
    /// Execute an execution plan, reporting events to the provided reporter.
    ///
    /// Returns Err(ExitStatus) to suggest the caller to abort and exit the process with the given exit status.
    ///
    /// The return type isn't just ExitStatus because we want to distinguish between normal successful execution,
    /// and execution that failed and needs to exit with a specific code which can be zero.
    pub(crate) async fn execute(
        &self,
        plan: ExecutionPlan,
        mut reporter: Box<dyn Reporter>,
    ) -> Result<(), ExitStatus> {
        // Lazily initialize the cache on first execution
        let cache = match self.cache() {
            Ok(cache) => cache,
            Err(err) => {
                reporter.handle_event(ExecutionEvent {
                    execution_id: ExecutionId::zero(),
                    kind: ExecutionEventKind::Error {
                        message: format!("Failed to initialize cache: {err}"),
                    },
                });
                return Err(ExitStatus(1));
            }
        };

        let mut execution_context = ExecutionContext {
            event_handler: &mut *reporter,
            current_execution_id: ExecutionId::zero(),
            cache,
            cache_base_path: &self.workspace_path,
        };

        // Execute and swallow ExecutionAborted error
        // display is None for top-level execution
        let _ = execution_context.execute_item_kind(None, plan.root_node()).await;

        // Always call post_execution, whether execution succeeded or failed
        reporter.post_execution()
    }
}
