pub mod fingerprint;
pub mod spawn;

use std::{path::Path, sync::Arc};

use futures_util::FutureExt;
use petgraph::{
    algo::{Cycle, toposort},
    graph::DiGraph,
};
use vite_path::{AbsolutePath, RelativePathBuf, relative::InvalidPathDataError};
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, display::TaskDisplay};
use vite_task_plan::{
    ExecutionItem, ExecutionItemKind, ExecutionPlan, LeafExecutionKind, SpawnExecution,
    TaskExecution,
    execution_graph::{ExecutionIx, ExecutionNodeIndex},
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
};
use crate::Session;

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("Path {path:?} is outside of the workspace {workspace_path:?}")]
    PathOutsideWorkspace { path: Arc<AbsolutePath>, workspace_path: Arc<AbsolutePath> },
    #[error("Path {path:?} contains characters that make it non-portable")]
    NonPortableRelativePath {
        path: Arc<Path>,
        #[source]
        error: InvalidPathDataError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error("Cycle dependencies detected: {0:?}")]
    CycleDependencies(Cycle<ExecutionNodeIndex>),

    #[error(transparent)]
    PathError(#[from] PathError),

    #[error(
        "Leaf execution item missing display information - execution items should be wrapped with ExecutionItem"
    )]
    MissingDisplayInfo,
}

struct ExecutionContext<'a> {
    indexed_task_graph: Option<&'a IndexedTaskGraph>,
    event_handler: &'a mut (dyn FnMut(ExecutionEvent) + 'a),
    current_execution_id: ExecutionId,
    cache: &'a ExecutionCache,
    /// All relative paths in cache are relative to this base path
    cache_base_path: &'a Arc<AbsolutePath>,
}

/// The origin of the current execution item, either directly from CLI args, or from a task in the task graph
enum ExecutionOrigin<'a> {
    CLIArgs {
        args_without_program: &'a Arc<[Str]>,
        cwd: &'a Arc<AbsolutePath>,
    },
    UserTask {
        task_display: TaskDisplay,
        item: &'a ExecutionItem,
        item_index: usize,
        item_count: usize,
    },
}

impl ExecutionContext<'_> {
    fn strip_prefix_for_cache(
        &self,
        path: &Arc<AbsolutePath>,
    ) -> Result<RelativePathBuf, PathError> {
        match path.strip_prefix(&*self.cache_base_path) {
            Ok(Some(rel_path)) => Ok(rel_path),
            Ok(None) => Err(PathError::PathOutsideWorkspace {
                path: Arc::clone(path),
                workspace_path: Arc::clone(self.cache_base_path),
            }),
            Err(err) => Err(PathError::NonPortableRelativePath {
                path: err.stripped_path.into(),
                error: err.invalid_path_data_error,
            }),
        }
    }

    async fn execute_item_kind(
        &mut self,
        item_kind: &ExecutionItemKind,
    ) -> Result<(), ExecuteError> {
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
                    Err(err) => return Err(ExecuteError::CycleDependencies(err)),
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
                                self.execute_item_kind(&item.kind).boxed_local().await?;
                            }
                        }
                    }
                }
            }
            ExecutionItemKind::Leaf(leaf_execution_kind) => {
                // This case should not happen in practice since we always wrap leaf items
                // in an ExecutionItem with display info. Log a warning if it does.
                tracing::warn!("execute_item_kind called with bare Leaf - missing display info");
                return Err(ExecuteError::MissingDisplayInfo);
            }
        }
        Ok(())
    }

    async fn execute_leaf(
        &mut self,
        display: &ExecutionItemDisplay,
        leaf_execution_kind: &LeafExecutionKind,
    ) -> Result<(), ExecuteError> {
        let execution_id = self.current_execution_id;
        self.current_execution_id = self.current_execution_id.next();
        (self.event_handler)(ExecutionEvent {
            execution_id,
            kind: ExecutionEventKind::Start(display.clone()),
        });

        match leaf_execution_kind {
            LeafExecutionKind::InProcess(in_process_execution) => {
                let execution_output = in_process_execution.execute().await;
                (self.event_handler)(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Output {
                        kind: OutputKind::Stdout,
                        content: execution_output.stdout.into(),
                    },
                });
                (self.event_handler)(ExecutionEvent {
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
    ) -> Result<(), ExecuteError> {
        let cache_metadata = spawn_execution.cache_metadata.as_ref();

        // 1. Try cache hit
        if let Some(cache_metadata) = cache_metadata {
            match self.cache.try_hit(cache_metadata, &*self.cache_base_path).await {
                Ok(Ok(cached)) => {
                    // Replay cached outputs
                    for output in cached.std_outputs.iter() {
                        (self.event_handler)(ExecutionEvent {
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
                    (self.event_handler)(ExecutionEvent {
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
                    tracing::warn!("Cache lookup failed: {:?}", err);
                    // Continue to execute on cache error
                }
            }
        }

        // 2. Execute command with tracking
        let result =
            match spawn_with_tracking(&spawn_execution.spawn_command, &*self.cache_base_path).await
            {
                Ok(result) => result,
                Err(err) => {
                    // Emit error output and finish
                    (self.event_handler)(ExecutionEvent {
                        execution_id,
                        kind: ExecutionEventKind::Output {
                            kind: OutputKind::Stderr,
                            content: format!("Failed to spawn: {err}").into(),
                        },
                    });
                    (self.event_handler)(ExecutionEvent {
                        execution_id,
                        kind: ExecutionEventKind::Finish {
                            status: None,
                            cache_status: CacheStatus::Miss,
                        },
                    });
                    return Ok(());
                }
            };

        // 3. Emit outputs
        for output in result.std_outputs.iter() {
            (self.event_handler)(ExecutionEvent {
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
                        tracing::warn!("Failed to update cache: {:?}", err);
                    }
                }
                Err(err) => {
                    tracing::warn!("Failed to create post-run fingerprint: {:?}", err);
                }
            }
        }

        // 5. Emit finish
        (self.event_handler)(ExecutionEvent {
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
        event_handler: &mut (dyn FnMut(ExecutionEvent) + '_),
    ) -> Result<(), ExecuteError> {
        let mut execution_context = ExecutionContext {
            indexed_task_graph: self.lazy_task_graph.try_get(),
            event_handler,
            current_execution_id: ExecutionId::zero(),
            cache: &self.cache,
            cache_base_path: &self.workspace_path,
        };
        execution_context.execute_item_kind(plan.root_node()).await
    }
}
