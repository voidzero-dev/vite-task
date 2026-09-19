//! A persistent session that invalidates and restarts individual task executions.
pub mod inputs;
mod plan;

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use futures_util::{
    FutureExt as _, StreamExt as _, future::LocalBoxFuture, stream::FuturesUnordered,
};
use inputs::{Journal, RunInputs};
use notify::Watcher as _;
use plan::WatchPlan;
use tokio_util::sync::CancellationToken;
use vt_plan::ExecutionGraph;

use super::{
    execute::execute_watched_task,
    reporter::{ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder},
};
use crate::Session;

const DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Default)]
struct TaskState {
    pending: bool,
    succeeded: bool,
    cancel: Option<CancellationToken>,
    inputs: Option<Rc<RunInputs>>,
}

impl TaskState {
    fn start(&mut self, inputs: Rc<RunInputs>) -> CancellationToken {
        let cancel = CancellationToken::new();
        self.pending = false;
        self.cancel = Some(cancel.clone());
        self.inputs = Some(inputs);
        cancel
    }
}

impl Session<'_> {
    pub(crate) async fn watch_graph(
        &self,
        graph: ExecutionGraph,
        builder: Box<dyn GraphExecutionReporterBuilder>,
        interrupt: CancellationToken,
    ) -> Result<(), ExitStatus> {
        let journal = Journal::default();
        let workspace = self.workspace_path.clone();
        let cache_root: Arc<vt_path::AbsolutePath> = (&*self.cache_root).into();
        let (tx, mut events) = tokio::sync::mpsc::channel(1);
        let observer_journal = journal.clone();
        let observer_workspace = workspace.clone();
        let observer_cache = cache_root.clone();
        let make_watcher = || -> anyhow::Result<notify::RecommendedWatcher> {
            let mut watcher = notify::recommended_watcher(
                move |event: notify::Result<notify::Event>| {
                    let mut journal = observer_journal.lock().unwrap();
                    match event {
                        Ok(event) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                            if event.need_rescan() {
                                journal.error = Some("Watch event stream overflowed; restart watch to reestablish input tracking".into());
                            }
                            for path in event.paths {
                                let Some(path) = vt_path::AbsolutePathBuf::new(path) else {
                                    continue;
                                };
                                let path = path.clean();
                                if inputs::excluded(&path, &observer_workspace, &observer_cache) {
                                    continue;
                                }
                                journal.version += 1;
                                let version = journal.version;
                                journal.paths.insert(path, version);
                            }
                        }
                        Ok(_) => return,
                        Err(error) => {
                            journal.error = Some(vt_str::format!("File watching failed: {error}"));
                        }
                    }
                    let _ = tx.try_send(());
                },
            )?;
            watcher.watch(workspace.as_path(), notify::RecursiveMode::Recursive)?;
            Ok(watcher)
        };
        // Install the watcher before creating input baselines or running tasks.
        let _watcher = match make_watcher() {
            Ok(watcher) => watcher,
            Err(error) => {
                watch_error(&error);
                return Err(ExitStatus::FAILURE);
            }
        };
        if let Err(error) = self.cache() {
            watch_error(&error);
            return Err(ExitStatus::FAILURE);
        }
        let reporter = RefCell::new(builder.build());
        let result = self
            .watch_loop(&graph, &reporter, &journal, &cache_root, &mut events, &interrupt)
            .await;
        let _ = reporter.into_inner().finish();
        result
    }

    async fn watch_loop(
        &self,
        graph: &ExecutionGraph,
        reporter: &RefCell<Box<dyn GraphExecutionReporter>>,
        journal: &Journal,
        cache_root: &Arc<vt_path::AbsolutePath>,
        events: &mut tokio::sync::mpsc::Receiver<()>,
        interrupt: &CancellationToken,
    ) -> Result<(), ExitStatus> {
        let plan = WatchPlan::new(graph);
        let mut states: Vec<_> = plan
            .nodes
            .iter()
            .map(|_| TaskState { pending: true, ..TaskState::default() })
            .collect();
        let mut running: FuturesUnordered<LocalBoxFuture<'_, (usize, bool)>> =
            FuturesUnordered::new();
        let mut tick = tokio::time::interval(Duration::from_millis(25));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut deadline = None;
        let mut debounced_version = 0;
        let mut waiting = false;
        watch_message("Watching task inputs. Press Ctrl+C to stop.");
        let result = loop {
            if interrupt.is_cancelled() {
                break Err(ExitStatus(130));
            }
            let error = journal.lock().unwrap().error.take();
            if let Some(error) = error {
                watch_error(&anyhow::anyhow!("{error}"));
                break Err(ExitStatus::FAILURE);
            }
            let version = latest_change(&states);
            if version > debounced_version {
                debounced_version = version;
                deadline = Some(tokio::time::Instant::now() + DEBOUNCE);
            }
            if deadline.is_none_or(|deadline| tokio::time::Instant::now() >= deadline) {
                deadline = None;
                if invalidate_changed(&plan, &mut states) {
                    waiting = false;
                }
            }
            // Do not start another generation in the middle of an editor's
            // burst, or while the previous process tree is still being reaped.
            if deadline.is_none() {
                for node in 0..plan.nodes.len() {
                    if !plan.ready(node, &states) {
                        continue;
                    }
                    let inputs = match RunInputs::new(
                        plan.nodes[node].task(),
                        &self.workspace_path,
                        cache_root,
                        journal,
                    ) {
                        Ok(inputs) => Rc::new(inputs),
                        Err(error) => {
                            journal.lock().unwrap().error =
                                Some(vt_str::format!("Watch input setup failed: {error}"));
                            break;
                        }
                    };
                    let cancel = states[node].start(inputs.clone());
                    waiting = false;
                    let execution = &plan.nodes[node];
                    running.push(
                        async move {
                            let succeeded = execute_watched_task(
                                self,
                                execution.graph,
                                execution.index,
                                reporter,
                                cancel,
                                &inputs,
                            )
                            .await;
                            (node, succeeded)
                        }
                        .boxed_local(),
                    );
                }
            }
            if running.is_empty() && !waiting {
                watch_message("Waiting for input changes...");
                pty_terminal_test_client::mark_milestone("watch-idle");
                waiting = true;
            }
            tokio::select! {
                () = interrupt.cancelled() => {}
                _ = events.recv() => {}
                Some((node, succeeded)) = running.next(), if !running.is_empty() => {
                    let state = &mut states[node];
                    state.cancel = None;
                    state.succeeded = succeeded && !state.pending;
                }
                _ = tick.tick() => {}
            }
        };
        for state in states {
            if let Some(cancel) = state.cancel {
                cancel.cancel();
            }
        }
        while running.next().await.is_some() {}
        result
    }
}

fn expand_affected(affected: &mut [bool], edges: &[(usize, usize)]) {
    loop {
        let mut changed = false;
        for &(dependent, dependency) in edges {
            if affected[dependency] && !affected[dependent] {
                affected[dependent] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn watch_message(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "{message}");
}

fn watch_error(error: &anyhow::Error) {
    watch_message(&vt_str::format!("error: {error:#}"));
}

fn invalidate_changed(plan: &WatchPlan<'_>, states: &mut [TaskState]) -> bool {
    let mut affected: Vec<_> = states
        .iter()
        .map(|state| state.inputs.as_ref().is_some_and(|inputs| inputs.changed()))
        .collect();
    expand_affected(&mut affected, &plan.impact_edges);
    for (node, state) in states.iter_mut().enumerate().filter(|(node, _)| affected[*node]) {
        state.pending = true;
        state.succeeded = false;
        state.inputs = None;
        if let Some(cancel) = &state.cancel {
            cancel.cancel();
        }
        let task = &plan.nodes[node].task().task_display;
        watch_message(&vt_str::format!(
            "Input changed; restarting {}#{}",
            task.package_name,
            task.task_name
        ));
    }
    affected.into_iter().any(|changed| changed)
}

fn latest_change(states: &[TaskState]) -> u64 {
    states.iter().filter_map(|state| state.inputs.as_ref()?.change_version()).max().unwrap_or(0)
}
