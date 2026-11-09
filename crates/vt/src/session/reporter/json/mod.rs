//! JSON reporter — prints the run plan of a dry run (`vp run --dry-run --json`).
//!
//! When it's created, it lays out every task and step of the graph. Each leaf's cache status
//! then fills in its step, and the whole document is written once, at the end, so stdout
//! holds one JSON document or nothing.

mod document;

use std::{
    cell::RefCell, collections::BTreeMap, io::Write, process::ExitStatus as StdExitStatus, rc::Rc,
    sync::Arc, time::Duration,
};

use document::{
    Baseline, Cache, Change, Changed, CommandStep, Hit, HowChanged, NestedStep, PlanTask, RunPlan,
    Setting, Step,
};
use rustc_hash::{FxHashMap, FxHashSet};
use vt_materialized_artifact::{Artifact, artifact};
use vt_path::AbsolutePath;
use vt_plan::{
    ExecutionGraph, ExecutionItem, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind,
    TaskExecution, execution_graph::ExecutionNodeIndex,
};
use vt_str::Str;

use super::{
    ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder, LeafExecutionReporter,
    LeafPath, PipeWriters, StdioConfig, StdioSuggestion,
};
use crate::session::{
    cache::{self, CacheMiss, Comparison, InputChangeKind},
    event::{CacheDisabledReason, CacheStatus, CacheUpdateStatus, ExecutionError},
    json::{self, Version, path_key, task_id},
};

/// The run plan's JSON Schema, `schemas/run-plan.json`.
pub const RUN_PLAN_SCHEMA: Artifact = artifact!("run-plan", "VT_RUN_PLAN_SCHEMA");

pub struct JsonReporterBuilder(JsonGraphReporter);

impl JsonReporterBuilder {
    /// Lay out every task and step of `graph`, the graph the reporter reports
    /// on. `writer` gets the document, indented when `pretty` is set, as in a
    /// terminal, and on one line otherwise. `schema` is its `$schema` URL.
    pub fn new(
        graph: &ExecutionGraph,
        workspace_path: &AbsolutePath,
        writer: Box<dyn Write>,
        pretty: bool,
        schema: Str,
    ) -> Self {
        let mut layout = Layout { workspace_path, steps: FxHashMap::default(), unreported: 0 };
        let plan = layout.plan(graph, &[], &[]);
        let Layout { steps, unreported, .. } = layout;
        let document = RunPlan {
            schema,
            version: Version,
            // `clean` drops a trailing separator, which a workspace root found from such a cwd keeps.
            workspace_root: Arc::from(workspace_path.clean().as_absolute_path()),
            plan,
        };
        Self(JsonGraphReporter {
            state: Rc::new(RefCell::new(State { document, unreported, errors: Vec::new() })),
            steps,
            writer,
            pretty,
        })
    }
}

impl GraphExecutionReporterBuilder for JsonReporterBuilder {
    fn build(self: Box<Self>) -> Box<dyn GraphExecutionReporter> {
        Box::new(self.0)
    }
}

/// Where a step is in the document: the task ID and step index in each plan,
/// outermost first, down through the nested plans of `vp run` steps.
type StepLocation = Vec<(Str, usize)>;

/// Lays out the plans of a graph and its nested graphs, before any cache is
/// checked.
struct Layout<'a> {
    workspace_path: &'a AbsolutePath,
    /// Where each spawned or built-in leaf's step is, by the leaf's path in
    /// the graph.
    steps: FxHashMap<Vec<(ExecutionNodeIndex, usize)>, StepLocation>,
    /// The number of spawned steps, which each report a cache status.
    unreported: usize,
}

impl Layout<'_> {
    /// The plan of `graph`, whose items are at `path` in the root graph and
    /// whose steps are at `location` in the document. Each plan's
    /// `dependsOn` refers only to its own tasks.
    fn plan(
        &mut self,
        graph: &ExecutionGraph,
        path: &LeafPath,
        location: &[(Str, usize)],
    ) -> BTreeMap<Str, PlanTask> {
        let ids: FxHashMap<ExecutionNodeIndex, Str> = graph
            .graph
            .node_indices()
            .map(|node| (node, task_key(&graph.graph[node], self.workspace_path).1))
            .collect();
        let mut plan = BTreeMap::new();
        for node in graph.graph.node_indices() {
            let task = &graph.graph[node];
            let (package, id) = task_key(task, self.workspace_path);
            let mut depends_on: Vec<Str> =
                graph.graph.neighbors(node).map(|dependency| ids[&dependency].clone()).collect();
            depends_on.sort_unstable();
            let steps = task
                .items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let item_path: Vec<_> = path.iter().copied().chain([(node, index)]).collect();
                    let item_location: StepLocation =
                        location.iter().cloned().chain([(id.clone(), index)]).collect();
                    self.step(item, task, item_path, item_location)
                })
                .collect();
            plan.insert(
                id,
                PlanTask { package, name: task.task_display.task_name.clone(), depends_on, steps },
            );
        }
        plan
    }

    /// The step of `item`, a step of `task`, at `path` in the graph and
    /// `location` in the document. A nested `vp run` is its own plan.
    fn step(
        &mut self,
        item: &ExecutionItem,
        task: &TaskExecution,
        path: Vec<(ExecutionNodeIndex, usize)>,
        location: StepLocation,
    ) -> Step {
        let display = &item.execution_item_display;
        // A spawned step runs its spawn command, which a command handler may
        // have changed from what the display shows.
        let (program, args, cwd) = match &item.kind {
            ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn)) => {
                self.unreported += 1;
                (
                    path_key(&spawn.spawn_command.program_path, self.workspace_path),
                    spawn.spawn_command.args.to_vec(),
                    &spawn.spawn_command.cwd,
                )
            }
            ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(in_process)) => {
                (Str::from(in_process.name()), in_process.args(), &display.cwd)
            }
            ExecutionItemKind::Expanded(nested_graph) => {
                let plan = self.plan(nested_graph, &path, &location);
                return Step::Nested(NestedStep { expanded_from: display.command.clone(), plan });
            }
        };
        self.steps.insert(path, location);
        let cwd =
            (*cwd != task.task_display.package_path).then(|| path_key(cwd, self.workspace_path));
        // A `pre` or `post` script's steps keep the script's own name.
        let hook = (display.task_display.task_name != task.task_display.task_name)
            .then(|| display.task_display.task_name.clone());
        Step::Command(CommandStep { program, args, cwd, hook, cache: None })
    }
}

/// The task's package path and its task ID.
fn task_key(task: &TaskExecution, workspace_path: &AbsolutePath) -> (Str, Str) {
    let package = path_key(&task.task_display.package_path, workspace_path);
    let id = task_id(&package, &task.task_display.task_name);
    (package, id)
}

/// The document, filled in as leaves report.
struct State {
    document: RunPlan,
    /// The spawned steps whose cache status hasn't been reported yet. A run
    /// stopped by an error, or cancelled by Ctrl-C, leaves some, and its plan
    /// isn't printed.
    unreported: usize,
    /// The errors that stopped the run, such as a failed cache lookup.
    errors: Vec<Str>,
}

struct JsonGraphReporter {
    state: Rc<RefCell<State>>,
    /// Where each leaf's step is, by the leaf's path in the graph.
    steps: FxHashMap<Vec<(ExecutionNodeIndex, usize)>, StepLocation>,
    writer: Box<dyn Write>,
    pretty: bool,
}

impl GraphExecutionReporter for JsonGraphReporter {
    fn new_leaf_execution(
        &mut self,
        path: &LeafPath,
        _display: &ExecutionItemDisplay,
        _leaf_kind: &LeafExecutionKind,
    ) -> Box<dyn LeafExecutionReporter> {
        let location = self.steps.get(path).cloned();
        Box::new(JsonLeafReporter { state: Rc::clone(&self.state), location })
    }

    /// A run stopped by an error, or cancelled by Ctrl-C, leaves some steps
    /// unreported, and prints no plan. Its errors go to stderr. Otherwise,
    /// the steps that earlier steps may change are marked `unknown` first.
    fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        let mut state = self.state.borrow_mut();
        if state.unreported > 0 {
            #[expect(clippy::print_stderr, reason = "errors go to stderr, and stdout gets no plan")]
            for message in &state.errors {
                eprintln!("{message}");
            }
            return Err(ExitStatus::FAILURE);
        }
        mark_unknown(&mut state.document.plan, false);
        let mut writer = self.writer;
        json::write_document(&mut writer, &state.document, self.pretty)
            .map_err(|_| ExitStatus::FAILURE)
    }
}

struct JsonLeafReporter {
    state: Rc<RefCell<State>>,
    /// Where the leaf's step is in the document.
    location: Option<StepLocation>,
}

impl JsonLeafReporter {
    fn report(&self, cache: Option<Cache>) {
        let Some(location) = &self.location else { return };
        let mut state = self.state.borrow_mut();
        let State { document, unreported, .. } = &mut *state;
        if let Some(step) = find_step(&mut document.plan, location) {
            if cache.is_some() {
                *unreported = unreported.saturating_sub(1);
            }
            step.cache = cache;
        }
    }
}

/// The command step at `location` in `plan`.
fn find_step<'a>(
    plan: &'a mut BTreeMap<Str, PlanTask>,
    location: &[(Str, usize)],
) -> Option<&'a mut CommandStep> {
    let ((id, index), rest) = location.split_first()?;
    match (plan.get_mut(id)?.steps.get_mut(*index)?, rest) {
        (Step::Command(step), []) => Some(step),
        (Step::Nested(nested), rest) => find_step(&mut nested.plan, rest),
        (Step::Command(_), _) => None,
    }
}

impl LeafExecutionReporter for JsonLeafReporter {
    fn start(&mut self, cache_status: CacheStatus) -> StdioConfig {
        self.report(cache_of(&cache_status));
        // Nothing runs or is replayed, so nothing is written to these.
        StdioConfig {
            suggestion: StdioSuggestion::Piped,
            writers: PipeWriters {
                stdout_writer: Box::new(std::io::sink()),
                stderr_writer: Box::new(std::io::sink()),
            },
        }
    }

    /// An error, such as a failed cache lookup, comes without `start()`. It
    /// stops the run, as it stops a real run, and leaves its step unreported.
    ///
    /// A lookup cancelled by Ctrl-C has neither, and also leaves its step
    /// unreported.
    fn finish(
        self: Box<Self>,
        _status: Option<StdExitStatus>,
        _cache_update_status: CacheUpdateStatus,
        error: Option<ExecutionError>,
    ) {
        if let Some(error) = error {
            let message = vt_str::format!("{:#}", anyhow::Error::from(error));
            let message = match self.location.as_ref().and_then(|location| location.last()) {
                Some((id, _)) => vt_str::format!("{id}: {message}"),
                None => message,
            };
            self.state.borrow_mut().errors.push(message);
        }
    }
}

/// Mark as `unknown` each `hit` or `changed` step of `plan` that waits on a
/// step that writes files. `before_writes` says whether something the plan
/// waits on writes: the steps before the nested `vp run` step it comes from.
///
/// A step waits on the earlier steps of its task and on every step of the
/// tasks in its `dependsOn`, transitively. Whether a step writes is decided
/// from today's check, before any step is marked.
fn mark_unknown(plan: &mut BTreeMap<Str, PlanTask>, before_writes: bool) {
    let task_writes: FxHashMap<Str, bool> =
        plan.iter().map(|(id, task)| (id.clone(), task.steps.iter().any(step_writes))).collect();
    let ids: Vec<Str> = plan.keys().cloned().collect();
    for id in ids {
        let mut writes = before_writes || depends_on_writes(plan, &task_writes, &id);
        let Some(task) = plan.get_mut(&id) else { continue };
        for step in &mut task.steps {
            let step_writes = step_writes(step);
            match step {
                Step::Command(CommandStep { cache: Some(cache), .. }) if writes => {
                    mark_step_unknown(cache);
                }
                Step::Command(_) => {}
                Step::Nested(nested) => mark_unknown(&mut nested.plan, writes),
            }
            writes |= step_writes;
        }
    }
}

/// Whether a task in the transitive `dependsOn` of the task `id` writes files.
fn depends_on_writes(
    plan: &BTreeMap<Str, PlanTask>,
    task_writes: &FxHashMap<Str, bool>,
    id: &Str,
) -> bool {
    let mut seen = FxHashSet::default();
    let mut pending: Vec<&Str> =
        plan.get(id).map(|task| task.depends_on.iter().collect()).unwrap_or_default();
    while let Some(dependency) = pending.pop() {
        if !seen.insert(dependency) {
            continue;
        }
        if task_writes.get(dependency).copied().unwrap_or(false) {
            return true;
        }
        if let Some(task) = plan.get(dependency) {
            pending.extend(&task.depends_on);
        }
    }
    false
}

/// Whether a step would write files before the steps that wait on it start:
/// it would run, or it's a hit that restores outputs. A nested plan writes if
/// any of its steps does. A built-in writes nothing.
fn step_writes(step: &Step) -> bool {
    match step {
        Step::Command(CommandStep { cache, .. }) => match cache {
            None | Some(Cache::Hit(Hit { restores_outputs: false, .. })) => false,
            Some(
                Cache::Hit(_)
                | Cache::Changed(_)
                | Cache::Unknown(_)
                | Cache::NoBaseline
                | Cache::Error { .. }
                | Cache::Disabled,
            ) => true,
        },
        Step::Nested(nested) => nested.plan.values().any(|task| task.steps.iter().any(step_writes)),
    }
}

/// Turn a `hit` or `changed` status into `unknown`, keeping today's check.
fn mark_step_unknown(cache: &mut Cache) {
    let unknown = match std::mem::replace(cache, Cache::Disabled) {
        Cache::Hit(Hit { baseline, .. }) => {
            Changed { baseline: Some(baseline), changes: Vec::new(), changes_partial: false }
        }
        Cache::Changed(changed) => changed,
        other => {
            *cache = other;
            return;
        }
    };
    *cache = Cache::Unknown(unknown);
}

/// How the plan shows a cache status. `None` for a built-in, which isn't cached.
fn cache_of(status: &CacheStatus) -> Option<Cache> {
    Some(match status {
        CacheStatus::Hit { replayed_duration, restores_outputs, .. } => Cache::Hit(Hit {
            baseline: baseline(*replayed_duration),
            restores_outputs: *restores_outputs,
        }),
        CacheStatus::Miss(CacheMiss::NotFound) => Cache::NoBaseline,
        CacheStatus::Miss(CacheMiss::Changed(comparison)) => changed(comparison),
        CacheStatus::Miss(CacheMiss::RemoteReadFailed(error)) => {
            Cache::Error { message: vt_str::format!("{error}") }
        }
        CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata) => Cache::Disabled,
        CacheStatus::Disabled(CacheDisabledReason::InProcessExecution) => return None,
    })
}

/// A step that differs from its cached run. After a command, config or env
/// change, only the key of the task's previous run is compared, so there's
/// no `baseline`.
fn changed(comparison: &Comparison) -> Cache {
    let mut changes: Vec<Change> = comparison.changes.iter().map(change_of).collect();
    // Untracked env names are one `untrackedEnv` setting.
    changes.dedup();
    Cache::Changed(Changed {
        baseline: comparison.baseline_duration.map(baseline),
        changes,
        changes_partial: comparison.changes_partial,
    })
}

fn baseline(duration: Duration) -> Baseline {
    Baseline { duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX) }
}

fn change_of(change: &cache::Change) -> Change {
    let relative = |path: &vt_path::RelativePathBuf| {
        if path.as_str().is_empty() { Str::from(".") } else { Str::from(path.as_str()) }
    };
    match change {
        cache::Change::Program { from, to } => {
            Change::Program { from: from.clone(), to: to.clone() }
        }
        cache::Change::Args { from, to } => Change::Args { from: from.to_vec(), to: to.to_vec() },
        cache::Change::Cwd { from, to } => Change::Cwd { from: relative(from), to: relative(to) },
        cache::Change::InputConfig => Change::Config { setting: Setting::Input },
        cache::Change::OutputConfig => Change::Config { setting: Setting::Output },
        cache::Change::UntrackedEnvAdded { .. } | cache::Change::UntrackedEnvRemoved { .. } => {
            Change::Config { setting: Setting::UntrackedEnv }
        }
        cache::Change::Env { mismatch, .. } => Change::Env {
            name: mismatch.name().clone(),
            change: match mismatch {
                cache::EnvMismatch::Added { .. } => HowChanged::Added,
                cache::EnvMismatch::Removed { .. } => HowChanged::Removed,
                cache::EnvMismatch::Changed { .. } => HowChanged::Changed,
            },
        },
        cache::Change::File { kind, path } => Change::File {
            path: Str::from(path.as_str()),
            change: match kind {
                InputChangeKind::Added => HowChanged::Added,
                InputChangeKind::Removed => HowChanged::Removed,
                InputChangeKind::ContentModified => HowChanged::Changed,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{
        event::{CacheErrorKind, CacheNotUpdatedReason},
        json,
        reporter::test_fixtures::{spawn_task, test_path},
    };

    /// A failed cache lookup stops the run, as it stops a real run, so no plan
    /// is printed and the run fails.
    #[test]
    fn a_failed_lookup_prints_no_plan() {
        let mut graph = petgraph::graph::DiGraph::default();
        let node = graph.add_node(spawn_task("build"));
        let graph = ExecutionGraph::try_from_graph(graph, 1).unwrap();
        let builder = JsonReporterBuilder::new(
            &graph,
            &test_path(),
            Box::new(std::io::sink()),
            false,
            Str::default(),
        );
        let mut reporter = Box::new(builder).build();
        let item = &graph.graph[node].items[0];
        let ExecutionItemKind::Leaf(leaf_kind) = &item.kind else {
            panic!("test fixture item must be a Leaf");
        };
        let leaf =
            reporter.new_leaf_execution(&[(node, 0)], &item.execution_item_display, leaf_kind);
        let error = ExecutionError::Cache {
            kind: CacheErrorKind::Lookup,
            source: anyhow::anyhow!("database is locked"),
        };
        leaf.finish(
            None,
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::DryRun),
            Some(error),
        );
        assert!(reporter.finish().is_err());
    }

    /// Regenerates `schemas/run-plan.json` from the document types. Run with
    /// `UPDATE_SNAPSHOTS=1` after changing them.
    #[test]
    fn schema_matches_the_document_types() {
        json::check_schema::<RunPlan>("run-plan.json");
    }

    /// Command changes keep what they changed from and to, and several
    /// `untrackedEnv` names are one setting change.
    #[test]
    fn changes_keep_their_details() {
        let rel = |path| vt_path::RelativePathBuf::new(path).unwrap();
        let untracked = |name| cache::Change::UntrackedEnvAdded { name: Str::from(name) };
        let comparison = Comparison {
            changes: vec![
                cache::Change::Program { from: Str::from("tsc"), to: Str::from("tsdown") },
                cache::Change::Args { from: Arc::from([]), to: Arc::from([Str::from("-w")]) },
                cache::Change::Cwd { from: rel(""), to: rel("src") },
                untracked("A"),
                untracked("B"),
            ],
            baseline_duration: Some(Duration::from_millis(1830)),
            changes_partial: false,
        };
        let json = serde_json::to_value(changed(&comparison)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "status": "changed",
                "baseline": { "durationMs": 1830 },
                "changes": [
                    { "kind": "program", "from": "tsc", "to": "tsdown" },
                    { "kind": "args", "from": [], "to": ["-w"] },
                    { "kind": "cwd", "from": ".", "to": "src" },
                    { "kind": "config", "setting": "untrackedEnv" },
                ],
            })
        );
    }

    fn command(cache: Option<Cache>) -> Step {
        Step::Command(CommandStep {
            program: Str::from("vtt"),
            args: Vec::new(),
            cwd: None,
            hook: None,
            cache,
        })
    }

    fn hit(restores_outputs: bool) -> Cache {
        Cache::Hit(Hit { baseline: baseline(Duration::ZERO), restores_outputs })
    }

    fn task(depends_on: &[&str], steps: Vec<Step>) -> PlanTask {
        PlanTask {
            package: Str::from("."),
            name: Str::from("t"),
            depends_on: depends_on.iter().copied().map(Str::from).collect(),
            steps,
        }
    }

    fn statuses(plan: &BTreeMap<Str, PlanTask>) -> serde_json::Value {
        serde_json::Value::Object(
            plan.iter()
                .map(|(id, task)| {
                    let steps = task.steps.iter().map(|step| match step {
                        Step::Command(step) => {
                            serde_json::to_value(&step.cache).unwrap()["status"].clone()
                        }
                        Step::Nested(nested) => statuses(&nested.plan),
                    });
                    (id.to_string(), serde_json::Value::Array(steps.collect()))
                })
                .collect(),
        )
    }

    /// A step is `unknown` when a step it waits on writes files: an earlier
    /// step of its task, a step of a task in its `dependsOn` (transitively),
    /// or a step before the nested run it's in. A hit without outputs and a
    /// built-in write nothing.
    #[test]
    fn steps_after_a_write_are_unknown() {
        let nested_plan =
            BTreeMap::from([(Str::from("n"), task(&[], vec![command(Some(hit(false)))]))]);
        let mut plan = BTreeMap::from([
            (Str::from("restores"), task(&[], vec![command(Some(hit(true)))])),
            (Str::from("after-restore"), task(&["through"], vec![command(Some(hit(false)))])),
            (Str::from("through"), task(&["restores"], vec![command(None)])),
            (Str::from("plain"), task(&[], vec![command(Some(hit(false))), command(None)])),
            (Str::from("after-plain"), task(&["plain"], vec![command(Some(hit(false)))])),
            (
                Str::from("steps"),
                task(
                    &[],
                    vec![
                        command(Some(hit(false))),
                        command(Some(Cache::NoBaseline)),
                        Step::Nested(NestedStep {
                            expanded_from: Str::from("vt run n"),
                            plan: nested_plan,
                        }),
                    ],
                ),
            ),
        ]);
        mark_unknown(&mut plan, false);
        assert_eq!(
            statuses(&plan),
            serde_json::json!({
                "after-plain": ["hit"],
                "after-restore": ["unknown"],
                "plain": ["hit", null],
                "restores": ["hit"],
                "steps": ["hit", "noBaseline", { "n": ["unknown"] }],
                "through": [null],
            })
        );
    }

    /// After a config change, only the previous run's key is compared: the
    /// step is changed, with no `baseline`, and other files may have changed.
    #[test]
    fn key_changes_have_no_baseline() {
        let comparison = Comparison {
            changes: vec![cache::Change::InputConfig],
            baseline_duration: None,
            changes_partial: true,
        };
        assert_eq!(
            serde_json::to_value(changed(&comparison)).unwrap(),
            serde_json::json!({
                "status": "changed",
                "changes": [{ "kind": "config", "setting": "input" }],
                "changesPartial": true,
            })
        );
    }

    #[test]
    fn materializes_the_committed_schema() {
        let path = json::materialize(RUN_PLAN_SCHEMA).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            include_bytes!("../../../../schemas/run-plan.json")
        );
    }
}
