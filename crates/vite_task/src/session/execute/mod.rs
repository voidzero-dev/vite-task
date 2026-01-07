pub mod fingerprint;
pub mod spawn;

use std::sync::Arc;

use futures_util::FutureExt;
use petgraph::{algo::toposort, graph::DiGraph};
use vite_path::AbsolutePath;
use vite_task_graph::IndexedTaskGraph;
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
        CacheDisabledReason, CacheStatus, ExecutionEvent, ExecutionEventKind, ExecutionId,
        ExecutionItemDisplay, OutputKind,
    },
    reporter::Reporter,
};
use crate::Session;

/// Internal error type used to abort execution when errors occur.
/// This error is swallowed in Session::execute and never exposed externally.
#[derive(Debug)]
struct ExecutionAborted;

struct ExecutionContext<'a> {
    indexed_task_graph: Option<&'a IndexedTaskGraph>,
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
                // clone for reversing edges and removing nodes
                let mut graph: DiGraph<&TaskExecution, (), ExecutionIx> =
                    graph.map(|_, task_execution| task_execution, |_, ()| ());

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

                        // display is None for top-level execution (no parent task)
                        // display is Some for nested execution (within a parent task)
                        self.event_handler.handle_event(ExecutionEvent {
                            execution_id,
                            kind: ExecutionEventKind::Start(display.cloned()),
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
                                self.execute_leaf(&item.execution_item_display, leaf_kind)
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
                // Top-level leaf execution (built-in commands like 'vite lint')
                // These don't have display info since they're not from the task graph
                let execution_id = self.current_execution_id;
                self.current_execution_id = self.current_execution_id.next();

                // Emit start event with None display (no task info)
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Start(None),
                });

                // Execute the leaf directly
                match leaf_execution_kind {
                    LeafExecutionKind::InProcess(in_process_execution) => {
                        let execution_output = in_process_execution.execute().await;
                        self.event_handler.handle_event(ExecutionEvent {
                            execution_id,
                            kind: ExecutionEventKind::Output {
                                kind: OutputKind::Stdout,
                                content: execution_output.stdout.into(),
                            },
                        });
                        self.event_handler.handle_event(ExecutionEvent {
                            execution_id,
                            kind: ExecutionEventKind::Finish {
                                status: Some(0),
                                cache_status: CacheStatus::Disabled(
                                    CacheDisabledReason::InProcessExecution,
                                ),
                            },
                        });
                    }
                    LeafExecutionKind::Spawn(spawn_execution) => {
                        self.execute_spawn(execution_id, spawn_execution).await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn execute_leaf(
        &mut self,
        display: &ExecutionItemDisplay,
        leaf_execution_kind: &LeafExecutionKind,
    ) -> Result<(), ExecutionAborted> {
        let execution_id = self.current_execution_id;
        self.current_execution_id = self.current_execution_id.next();
        self.event_handler.handle_event(ExecutionEvent {
            execution_id,
            kind: ExecutionEventKind::Start(Some(display.clone())),
        });

        match leaf_execution_kind {
            LeafExecutionKind::InProcess(in_process_execution) => {
                let execution_output = in_process_execution.execute().await;
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Output {
                        kind: OutputKind::Stdout,
                        content: execution_output.stdout.into(),
                    },
                });
                self.event_handler.handle_event(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Finish {
                        status: Some(0),
                        cache_status: CacheStatus::Disabled(
                            CacheDisabledReason::InProcessExecution,
                        ),
                    },
                });
            }
            LeafExecutionKind::Spawn(spawn_execution) => {
                self.execute_spawn(execution_id, spawn_execution).await?;
            }
        }
        Ok(())
    }

    async fn execute_spawn(
        &mut self,
        execution_id: ExecutionId,
        spawn_execution: &SpawnExecution,
    ) -> Result<(), ExecutionAborted> {
        let cache_metadata = spawn_execution.cache_metadata.as_ref();

        // 1. Try cache hit
        if let Some(cache_metadata) = cache_metadata {
            match self.cache.try_hit(cache_metadata, &*self.cache_base_path).await {
                Ok(Ok(cached)) => {
                    // Replay cached outputs
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
                    self.event_handler.handle_event(ExecutionEvent {
                        execution_id,
                        kind: ExecutionEventKind::Finish {
                            status: Some(0),
                            cache_status: CacheStatus::Hit { replayed_duration: cached.duration },
                        },
                    });
                    return Ok(());
                }
                Ok(Err(_cache_miss)) => {
                    // Continue to execute
                }
                Err(err) => {
                    self.event_handler.handle_event(ExecutionEvent {
                        execution_id,
                        kind: ExecutionEventKind::Error {
                            message: format!("Cache lookup failed: {err}"),
                        },
                    });
                    return Err(ExecutionAborted);
                }
            }
        }

        // 2. Execute command with tracking
        let result =
            match spawn_with_tracking(&spawn_execution.spawn_command, &*self.cache_base_path).await
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

        // 3. Emit outputs
        for output in result.std_outputs.iter() {
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

        // 4. Update cache if successful
        if let Some(cache_metadata) = cache_metadata
            && result.exit_status.success()
        {
            let fingerprint_ignores =
                cache_metadata.spawn_fingerprint.fingerprint_ignores().map(|v| v.as_slice());
            match PostRunFingerprint::create(
                &result.path_reads,
                &*self.cache_base_path,
                fingerprint_ignores,
            ) {
                Ok(post_run_fingerprint) => {
                    let cache_value = CommandCacheValue {
                        post_run_fingerprint,
                        std_outputs: result.std_outputs.clone(),
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
        }

        // 5. Emit finish
        self.event_handler.handle_event(ExecutionEvent {
            execution_id,
            kind: ExecutionEventKind::Finish {
                status: result.exit_status.code(),
                cache_status: CacheStatus::Miss,
            },
        });

        Ok(())
    }
}

impl<'a, CustomSubcommand> Session<'a, CustomSubcommand> {
    pub async fn execute(
        &self,
        plan: ExecutionPlan,
        mut reporter: Box<dyn Reporter>,
    ) -> anyhow::Result<()> {
        let mut execution_context = ExecutionContext {
            indexed_task_graph: self.lazy_task_graph.try_get(),
            event_handler: &mut *reporter,
            current_execution_id: ExecutionId::zero(),
            cache: &self.cache,
            cache_base_path: &self.workspace_path,
        };

        // Execute and swallow ExecutionAborted error
        // display is None for top-level execution
        let _ = execution_context.execute_item_kind(None, plan.root_node()).await;

        // Always call post_execution, whether execution succeeded or failed
        reporter.post_execution()
    }
}
