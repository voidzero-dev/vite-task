//! Input discovery is independent of cache eligibility and survives task exits.
use std::sync::{Arc, Mutex};

use rustc_hash::{FxHashMap, FxHashSet};
use vt_graph::config::ResolvedGlobConfig;
use vt_path::{AbsolutePath, AbsolutePathBuf};
use vt_plan::{ExecutionItemKind, LeafExecutionKind, SpawnExecution, TaskExecution};
use vt_str::Str;
use wax::{Glob, Program as _};

use crate::session::{
    cache::CacheEntryValue,
    execute::{
        fingerprint::{PathFingerprint, PathRead, fingerprint_path},
        glob::collect_glob_paths,
    },
};

#[derive(Default)]
pub(super) struct Changes {
    pub version: u64,
    pub paths: FxHashMap<AbsolutePathBuf, u64>,
    pub error: Option<Str>,
}

pub(super) type Journal = Arc<Mutex<Changes>>;

#[derive(Default)]
struct Observed {
    reads: FxHashMap<AbsolutePathBuf, (bool, Option<PathFingerprint>)>,
    ignored: FxHashSet<AbsolutePathBuf>,
}

/// One command's input interests and observations, shared with tracing threads.
#[derive(Clone)]
pub struct LeafWatch {
    pub nested_watch: bool,
    workspace: Arc<AbsolutePath>,
    cache_root: Arc<AbsolutePath>,
    config: Arc<ResolvedGlobConfig>,
    positive: Arc<[Glob<'static>]>,
    negative: Arc<[Glob<'static>]>,
    observed: Arc<Mutex<Observed>>,
    writes: Arc<Mutex<FxHashSet<AbsolutePathBuf>>>,
    journal: Journal,
    start_version: u64,
}

fn beneath(path: &AbsolutePath, parent: &AbsolutePath) -> bool {
    matches!(path.strip_prefix(parent), Ok(Some(_)))
}

pub(super) fn excluded(
    path: &AbsolutePath,
    workspace: &AbsolutePath,
    cache_root: &AbsolutePath,
) -> bool {
    if beneath(path, cache_root) {
        return true;
    }
    let Ok(Some(relative)) = path.strip_prefix(workspace) else {
        return true;
    };
    relative.as_str().split('/').any(|part| part == ".git")
}

impl LeafWatch {
    fn new(
        config: &ResolvedGlobConfig,
        workspace: Arc<AbsolutePath>,
        cache_root: Arc<AbsolutePath>,
        journal: Journal,
        start_version: u64,
    ) -> anyhow::Result<Self> {
        let compile =
            |patterns: &std::collections::BTreeSet<Str>| -> anyhow::Result<Arc<[Glob<'static>]>> {
                patterns.iter().map(|p| Ok(Glob::new(p)?.into_owned())).collect()
            };
        let this = Self {
            nested_watch: false,
            positive: compile(&config.positive_globs)?,
            negative: compile(&config.negative_globs)?,
            config: Arc::new(config.clone()),
            workspace,
            cache_root,
            observed: Arc::default(),
            writes: Arc::default(),
            journal,
            start_version,
        };
        for relative in
            collect_glob_paths(&this.workspace, &config.positive_globs, &config.negative_globs)?
        {
            let path = this.workspace.join(relative);
            let baseline =
                fingerprint_path(&Arc::from(path.as_ref()), PathRead { read_dir_entries: false })?;
            this.observed.lock().unwrap().reads.insert(path, (false, Some(baseline)));
        }
        Ok(this)
    }

    fn matches(&self, patterns: &[Glob<'_>], path: &AbsolutePath) -> bool {
        path.strip_prefix(&self.workspace)
            .ok()
            .flatten()
            .is_some_and(|relative| patterns.iter().any(|glob| glob.is_match(relative.as_path())))
    }

    fn ignored(&self, path: &AbsolutePath, observed: &Observed) -> bool {
        excluded(path, &self.workspace, &self.cache_root)
            || self.matches(&self.negative, path)
            || (!self.matches(&self.positive, path)
                && observed.ignored.iter().any(|ignored| beneath(path, ignored)))
    }

    pub(crate) fn fail(&self, error: Str) {
        self.journal.lock().unwrap().error = Some(error);
    }

    pub(crate) fn ignore_observer(&self) -> vt_server::InputObserver {
        let observed = Arc::clone(&self.observed);
        Arc::new(move |path| {
            observed.lock().unwrap().ignored.insert(path.clean());
        })
    }

    #[cfg(fspy)]
    pub(crate) fn observer(&self) -> fspy::AccessObserver {
        let this = self.clone();
        Arc::new(move |result| {
            let access = match result {
                Ok(access) => access,
                Err(error) => {
                    this.journal.lock().unwrap().error =
                        Some(vt_str::format!("Watch input tracking failed: {error}"));
                    return;
                }
            };
            let path = access.path.strip_path_prefix(&*this.workspace, |result| {
                result
                    .ok()
                    .and_then(|path| vt_path::RelativePathBuf::new(path).ok())
                    .and_then(|path| path.clean().ok())
            });
            let Some(path) = path else {
                return;
            };
            let path = this.workspace.join(path);
            this.observe_path(path, access.mode);
        })
    }

    #[cfg(fspy)]
    fn observe_path(&self, path: AbsolutePathBuf, mode: fspy::AccessMode) {
        let mut observed = self.observed.lock().unwrap();
        if excluded(&path, &self.workspace, &self.cache_root) {
            return;
        }
        if mode.contains(fspy::AccessMode::WRITE) {
            self.writes.lock().unwrap().insert(path.clone());
            if self.matches(&self.positive, &path) && !self.ignored(&path, &observed) {
                self.journal.lock().unwrap().error = Some(vt_str::format!(
                    "Watch input is also an output: {}. Exclude it from input to prevent a restart loop.",
                    path.as_path().display()
                ));
            }
        }
        if !self.ignored(&path, &observed)
            && self.config.includes_auto
            && mode.intersects(fspy::AccessMode::READ | fspy::AccessMode::READ_DIR)
        {
            let entry = observed.reads.entry(path).or_insert((false, None));
            if mode.contains(fspy::AccessMode::READ_DIR) && !entry.0 {
                entry.0 = true;
                entry.1 = None;
            }
        }
        drop(observed);
    }

    pub(crate) fn cached_inputs(&self, cached: &CacheEntryValue) {
        let mut observed = self.observed.lock().unwrap();
        for (relative, fingerprint) in &cached.post_run_fingerprint.inferred_inputs {
            let directory = matches!(fingerprint, PathFingerprint::Folder(Some(_)));
            observed
                .reads
                .insert(self.workspace.join(relative), (directory, Some(fingerprint.clone())));
        }
    }

    pub(crate) fn cached_result(
        &self,
        cached: &CacheEntryValue,
        cache_dir: &AbsolutePath,
    ) -> anyhow::Result<()> {
        self.cached_inputs(cached);
        if let Some(name) = &cached.output_archive {
            crate::session::cache::archive::visit_output_paths(
                &cache_dir.join(name.as_str()),
                |relative| {
                    self.restored_output(&self.workspace.join(relative));
                },
            )?;
        }
        Ok(())
    }

    pub(crate) fn restored_output(&self, path: &AbsolutePath) {
        self.writes.lock().unwrap().insert(path.clean());
    }

    fn unchanged(
        &self,
        input: &AbsolutePath,
        directory: bool,
        baseline: &PathFingerprint,
        observed: &Observed,
        writes: &FxHashSet<AbsolutePathBuf>,
    ) -> bool {
        let Ok(current) =
            fingerprint_path(&Arc::from(input), PathRead { read_dir_entries: directory })
        else {
            return false;
        };
        if let (PathFingerprint::Folder(Some(before)), PathFingerprint::Folder(Some(after))) =
            (baseline, &current)
        {
            // Some backends coalesce entry events into a parent-directory event.
            // Compare only entries which could be inputs, excluding our outputs.
            before.keys().chain(after.keys()).all(|name| {
                let path = input.join(name.as_str());
                writes.contains(&path)
                    || self.ignored(&path, observed)
                    || before.get(name) == after.get(name)
            })
        } else {
            baseline == &current
        }
    }

    fn explicit_change(
        &self,
        path: &AbsolutePathBuf,
        observed: &Observed,
        writes: &FxHashSet<AbsolutePathBuf>,
    ) -> bool {
        if self.positive.is_empty() {
            return false;
        }
        if path.as_path().is_dir() {
            // A new directory can contain files before the backend installs a
            // recursive subscription. Its event must discover those files too.
            let Ok(paths) = collect_glob_paths(
                &self.workspace,
                &self.config.positive_globs,
                &self.config.negative_globs,
            ) else {
                return true;
            };
            paths.into_iter().map(|relative| self.workspace.join(relative)).any(|input| {
                beneath(&input, path)
                    && !self.ignored(&input, observed)
                    && !writes.contains(&input)
                    && observed
                        .reads
                        .get(&input)
                        .and_then(|(_, baseline)| baseline.as_ref())
                        .is_none_or(|baseline| {
                            !self.unchanged(&input, false, baseline, observed, writes)
                        })
            })
        } else {
            self.matches(&self.positive, path)
                && observed
                    .reads
                    .get(path)
                    .and_then(|(_, baseline)| baseline.as_ref())
                    .is_none_or(|baseline| !self.unchanged(path, false, baseline, observed, writes))
        }
    }

    /// Examine changes since this run began. Unknown reads retain the change
    /// journal until discovery, so an event before the first trace poll is safe.
    pub(crate) fn changed(&self) -> bool {
        self.change_version().is_some()
    }

    fn change_version(&self) -> Option<u64> {
        // The observer takes these locks in this order too.
        let mut observed = self.observed.lock().unwrap();
        let writes = self.writes.lock().unwrap();
        let journal = self.journal.lock().unwrap();
        let mut latest = None;
        for (path, &version) in &journal.paths {
            if version <= self.start_version
                || self.ignored(path, &observed)
                || writes.contains(path)
            {
                continue;
            }
            if self.explicit_change(path, &observed, &writes) {
                latest = Some(latest.unwrap_or(0).max(version));
            }
            for (input, (directory, baseline)) in &observed.reads {
                if self.ignored(input, &observed) || writes.contains(input) {
                    continue;
                }
                let relevant = path == input
                    || beneath(input, path)
                    || (*directory && path.parent() == Some(input.as_ref()));
                if relevant
                    && baseline.as_ref().is_none_or(|baseline| {
                        !self.unchanged(input, *directory, baseline, &observed, &writes)
                    })
                {
                    latest = Some(latest.unwrap_or(0).max(version));
                }
            }
        }
        drop(journal);
        drop(writes);
        if latest.is_some() {
            return latest;
        }
        for (path, (directory, baseline)) in &mut observed.reads {
            if baseline.is_none() {
                *baseline = fingerprint_path(
                    &Arc::from(path.as_ref()),
                    PathRead { read_dir_entries: *directory },
                )
                .ok();
            }
        }
        drop(observed);
        None
    }
}

pub struct RunInputs {
    leaves: FxHashMap<usize, LeafWatch>,
    explicit: Vec<LeafWatch>,
    writes: Arc<Mutex<FxHashSet<AbsolutePathBuf>>>,
}

impl RunInputs {
    pub(super) fn new(
        task: &TaskExecution,
        workspace: &Arc<AbsolutePath>,
        cache_root: &Arc<AbsolutePath>,
        journal: &Journal,
    ) -> anyhow::Result<Self> {
        let version = journal.lock().unwrap().version;
        let mut this =
            Self { leaves: FxHashMap::default(), explicit: Vec::new(), writes: Arc::default() };
        this.collect(task, workspace, cache_root, journal, version)?;
        Ok(this)
    }

    fn collect(
        &mut self,
        task: &TaskExecution,
        workspace: &Arc<AbsolutePath>,
        cache_root: &Arc<AbsolutePath>,
        journal: &Journal,
        version: u64,
    ) -> anyhow::Result<()> {
        if !task.input_config.positive_globs.is_empty()
            && task.items.iter().any(|item| {
                !matches!(item.kind, ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(_)))
            })
        {
            let mut watch = LeafWatch::new(
                &task.input_config,
                workspace.clone(),
                cache_root.clone(),
                journal.clone(),
                version,
            )?;
            watch.writes = self.writes.clone();
            self.explicit.push(watch);
        }
        for item in &task.items {
            match &item.kind {
                ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(execution)) => {
                    let mut watch = LeafWatch::new(
                        &execution.input_config,
                        workspace.clone(),
                        cache_root.clone(),
                        journal.clone(),
                        version,
                    )?;
                    watch.nested_watch = execution.nested_watch;
                    watch.writes = self.writes.clone();
                    self.leaves.insert(std::ptr::from_ref(execution).addr(), watch);
                }
                ExecutionItemKind::Expanded(graph) => {
                    for task in graph.graph.node_weights() {
                        self.collect(task, workspace, cache_root, journal, version)?;
                    }
                }
                ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(_)) => {}
            }
        }
        Ok(())
    }

    pub fn leaf(&self, execution: &SpawnExecution) -> &LeafWatch {
        &self.leaves[&std::ptr::from_ref(execution).addr()]
    }

    pub fn change_version(&self) -> Option<u64> {
        self.leaves.values().chain(&self.explicit).filter_map(LeafWatch::change_version).max()
    }

    pub fn changed(&self) -> bool {
        self.change_version().is_some()
    }
}

#[cfg(all(test, fspy))]
mod tests {
    use super::*;
    use crate::session::reporter::test_fixtures::spawn_task;

    fn setup(patterns: &[&str], auto: bool) -> (tempfile::TempDir, LeafWatch) {
        let temp = tempfile::tempdir().unwrap();
        let workspace: Arc<AbsolutePath> =
            vt_path::AbsolutePathBuf::new(temp.path().canonicalize().unwrap()).unwrap().into();
        let task = spawn_task("test");
        let ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(mut execution)) =
            task.items.into_iter().next().unwrap().kind
        else {
            unreachable!()
        };
        execution.input_config.includes_auto = auto;
        for pattern in patterns {
            if let Some(negative) = pattern.strip_prefix('!') {
                execution.input_config.negative_globs.insert(negative.into());
            } else {
                execution.input_config.positive_globs.insert((*pattern).into());
            }
        }
        let watch = LeafWatch::new(
            &execution.input_config,
            workspace.clone(),
            workspace.join("cache").into(),
            Journal::default(),
            0,
        )
        .unwrap();
        (temp, watch)
    }

    fn access(watch: &LeafWatch, path: &str, mode: fspy::AccessMode) {
        let path = watch.workspace.join(path);
        watch.observe_path(path, mode);
    }

    fn change(watch: &LeafWatch, path: &str) {
        let mut journal = watch.journal.lock().unwrap();
        journal.version += 1;
        let version = journal.version;
        journal.paths.insert(watch.workspace.join(path), version);
    }

    #[test]
    fn event_before_live_trace_registration_is_not_lost() {
        let (_temp, watch) = setup(&[], true);
        std::fs::write(watch.workspace.join("source"), "new").unwrap();
        change(&watch, "source");
        access(&watch, "source", fspy::AccessMode::READ);
        assert!(watch.changed());
    }

    #[test]
    fn new_glob_matches_and_exclusions_work_without_cache() {
        let (_temp, watch) = setup(&["*.txt", "!ignored.txt"], false);
        change(&watch, "ignored.txt");
        assert!(!watch.changed());
        change(&watch, "new.txt");
        assert!(watch.changed());
    }

    #[test]
    fn directory_listing_detects_new_entries_but_directory_stat_does_not() {
        for (mode, expected) in
            [(fspy::AccessMode::READ_DIR, true), (fspy::AccessMode::READ, false)]
        {
            let (_temp, watch) = setup(&[], true);
            std::fs::create_dir(watch.workspace.join("src")).unwrap();
            access(&watch, "src", mode);
            assert!(!watch.changed());
            std::fs::write(watch.workspace.join("src/new"), "new").unwrap();
            change(&watch, "src/new");
            assert_eq!(watch.changed(), expected);
        }
    }

    #[test]
    fn own_outputs_and_runtime_exclusions_do_not_restart_the_producer() {
        let (_temp, watch) = setup(&[], true);
        access(&watch, "output", fspy::AccessMode::READ | fspy::AccessMode::WRITE);
        change(&watch, "output");
        assert!(!watch.changed());
        access(&watch, "ignored", fspy::AccessMode::READ);
        watch.ignore_observer()(&watch.workspace.join("ignored").into());
        change(&watch, "ignored");
        assert!(!watch.changed());
        access(&watch, "cache/db", fspy::AccessMode::READ);
        change(&watch, "cache/db");
        assert!(!watch.changed());
    }

    #[test]
    fn explicit_read_write_overlap_reports_an_actionable_error() {
        let (_temp, watch) = setup(&["source"], false);
        access(&watch, "source", fspy::AccessMode::WRITE);
        assert!(watch.journal.lock().unwrap().error.as_ref().unwrap().contains("also an output"));
    }

    #[test]
    fn unchanged_content_does_not_trigger_a_restart() {
        let (_temp, watch) = setup(&[], true);
        std::fs::write(watch.workspace.join("source"), "same").unwrap();
        access(&watch, "source", fspy::AccessMode::READ);
        assert!(!watch.changed());
        change(&watch, "source");
        assert!(!watch.changed());
        std::fs::write(watch.workspace.join("source"), "different").unwrap();
        change(&watch, "source");
        assert!(watch.changed());
    }
    #[test]
    fn ignored_directories_exclude_descendants_but_explicit_inputs_still_win() {
        let (_temp, watch) = setup(&["generated/required"], true);
        watch.ignore_observer()(&watch.workspace.join("generated").into());
        access(&watch, "generated/ignored", fspy::AccessMode::READ);
        change(&watch, "generated/ignored");
        assert!(!watch.changed());
        change(&watch, "generated/required");
        assert!(watch.changed());
    }

    #[test]
    fn parent_directory_events_ignore_own_outputs_and_excluded_entries() {
        let (_temp, watch) = setup(&["!files/ignored"], true);
        std::fs::create_dir(watch.workspace.join("files")).unwrap();
        access(&watch, "files", fspy::AccessMode::READ_DIR);
        assert!(!watch.changed());
        access(&watch, "files/output", fspy::AccessMode::WRITE);
        std::fs::write(watch.workspace.join("files/output"), "output").unwrap();
        std::fs::write(watch.workspace.join("files/ignored"), "ignored").unwrap();
        change(&watch, "files");
        assert!(!watch.changed());
        std::fs::write(watch.workspace.join("files/input"), "input").unwrap();
        change(&watch, "files");
        assert!(watch.changed());
    }
    #[test]
    fn new_directories_discover_glob_matches_from_a_parent_event() {
        let (_temp, watch) = setup(&["new/**/*.txt"], false);
        std::fs::create_dir_all(watch.workspace.join("new/nested")).unwrap();
        change(&watch, "new");
        assert!(!watch.changed(), "empty directories are not explicit file inputs");
        std::fs::write(watch.workspace.join("new/nested/input.txt"), "new").unwrap();
        change(&watch, "new");
        assert!(watch.changed());
    }
}
