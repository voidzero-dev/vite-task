//! `vp run --dry-run`: walk the planned graph in dependency order and predict
//! each task's cache result from the local cache, without running anything or
//! writing to the cache.

use std::{io::Write as _, sync::Arc};

use rustc_hash::FxHashMap;
use vt_path::AbsolutePath;
use vt_plan::{
    ExecutionGraph, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind, SpawnExecution,
    execution_graph::ExecutionNodeIndex,
};
use vt_str::Str;

use super::glob::compute_globbed_inputs;
use crate::{
    Session,
    session::{
        cache::ExecutionCache,
        reporter::{
            dry_run::{DryRunPrediction, format_dry_run_line, format_remote_not_checked},
            summary::SavedCacheMissReason,
        },
    },
};

struct DryRun<'a> {
    /// `None` when there is no local cache yet, so every lookup is a miss.
    cache: Option<&'a ExecutionCache>,
    workspace_root: &'a Arc<AbsolutePath>,
    out: Vec<u8>,
    /// Set when a task has a remote cache configured. It isn't queried.
    remote_cache_configured: bool,
}

impl DryRun<'_> {
    /// Predict every task in `graph`, dependencies first. `runs_after` names a
    /// task outside `graph` that runs first and isn't a cache hit.
    ///
    /// Returns whether every task in `graph` is a predicted hit.
    async fn walk_graph(
        &mut self,
        graph: &ExecutionGraph,
        runs_after: Option<&Str>,
    ) -> anyhow::Result<bool> {
        // Tasks that aren't predicted hits, by node. Edge A→B means A depends
        // on B, so reversing the topological order puts dependencies first.
        let mut not_hit: FxHashMap<ExecutionNodeIndex, Str> = FxHashMap::default();
        for node_ix in graph.graph.compute_topological_order().into_iter().rev() {
            let task = &graph.graph[node_ix];
            let mut blocker: Option<Str> = runs_after.cloned().or_else(|| {
                graph.graph.neighbors(node_ix).find_map(|dep| not_hit.get(&dep).cloned())
            });
            let mut task_hit = true;
            for item in &task.items {
                let display = &item.execution_item_display;
                let item_hit = match &item.kind {
                    ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(_)) => {
                        // Built-ins don't change files, so they don't block later parts.
                        self.write(display, &DryRunPrediction::BuiltIn);
                        true
                    }
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn)) => {
                        self.spawn_leaf(display, spawn, blocker.as_ref()).await?
                    }
                    ExecutionItemKind::Expanded(nested) => {
                        Box::pin(self.walk_graph(nested, blocker.as_ref())).await?
                    }
                };
                if !item_hit {
                    task_hit = false;
                    // Later `&&` parts run after this one.
                    blocker.get_or_insert_with(|| display.command.clone());
                }
            }
            if !task_hit {
                not_hit.insert(node_ix, vt_str::format!("{}", task.task_display));
            }
        }
        Ok(not_hit.is_empty())
    }

    /// Predict one spawned process and write its line. Returns whether it's a
    /// predicted hit.
    async fn spawn_leaf(
        &mut self,
        display: &ExecutionItemDisplay,
        spawn: &SpawnExecution,
        runs_after: Option<&Str>,
    ) -> anyhow::Result<bool> {
        let Some(metadata) = &spawn.cache_metadata else {
            self.write(display, &DryRunPrediction::Disabled);
            return Ok(false);
        };
        self.remote_cache_configured |= metadata.remote_cache.is_some();

        let prediction = if let Some(runs_after) = runs_after {
            DryRunPrediction::Unknown { runs_after: runs_after.clone() }
        } else {
            let globbed_inputs = compute_globbed_inputs(
                self.workspace_root,
                &metadata.input_config.positive_globs,
                &metadata.input_config.negative_globs,
            )?;
            let lookup = match self.cache {
                Some(cache) => {
                    cache.peek_local(metadata, &globbed_inputs, self.workspace_root).await?
                }
                None => Err(crate::session::CacheMiss::NotFound),
            };
            match lookup {
                Ok(()) => DryRunPrediction::Hit,
                Err(miss) => DryRunPrediction::Miss(SavedCacheMissReason::from_cache_miss(&miss)),
            }
        };
        let hit = matches!(prediction, DryRunPrediction::Hit);
        self.write(display, &prediction);
        Ok(hit)
    }

    fn write(&mut self, display: &ExecutionItemDisplay, prediction: &DryRunPrediction) {
        self.out.extend_from_slice(
            format_dry_run_line(display, self.workspace_root, prediction).as_bytes(),
        );
    }
}

impl Session<'_> {
    /// Report what running `graph` would do with the local cache, without
    /// running any task. Nothing is written to the cache directory.
    pub(crate) async fn dry_run(&self, graph: &ExecutionGraph) -> anyhow::Result<()> {
        let cache = ExecutionCache::open_read_only(&self.cache_path)?;
        let mut dry_run = DryRun {
            cache: cache.as_ref(),
            workspace_root: &self.workspace_path,
            out: Vec::new(),
            remote_cache_configured: false,
        };
        dry_run.walk_graph(graph, None).await?;
        if dry_run.remote_cache_configured {
            dry_run.out.extend_from_slice(format_remote_not_checked().as_bytes());
        }
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&dry_run.out)?;
        stdout.flush()?;
        Ok(())
    }
}
