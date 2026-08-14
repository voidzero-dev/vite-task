//! One run of a task, from the filesystem's point of view.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use fspy_shared::ipc::PathAccess;
use rustc_hash::FxHashSet;
use vt_glob::path::PathGlobSet;
use vt_graph::config::ResolvedGlobConfig;
use vt_path::{AbsolutePath, RelativePathBuf};
use vt_str::Str;

use super::{
    collections::HashMap,
    fingerprint::{self, InputChange, InputFingerprints, PathRead},
    glob,
    tracked_accesses::TrackedPathAccesses,
};

/// One run of a task, from the filesystem's point of view.
///
/// Two stages: [`pre_run`](Self::pre_run) before the task executes,
/// [`post_run`](Self::post_run) after it finished. The final reuse decision
/// belongs to the caller — a run with no input change may still be
/// invalidated by checks outside this crate.
pub struct TaskFs<'a> {
    workspace_root: &'a AbsolutePath,
    input: &'a ResolvedGlobConfig,
    output: &'a ResolvedGlobConfig,
    /// Present iff either side has auto tracking — the same condition under
    /// which a trace is attached, so filters exist whenever a side counts.
    auto_filters: Option<AutoFilters<'a>>,
    /// Content hashes of the listed inputs, captured before the run.
    snapshot: BTreeMap<RelativePathBuf, u64>,
}

/// Compiled negative globs for filtering traced accesses, one per side.
struct AutoFilters<'a> {
    input_negative_globs: PathGlobSet<'a>,
    output_negative_globs: PathGlobSet<'a>,
}

/// How the run ended, from the filesystem's point of view.
#[derive(Debug)]
pub enum Conclusion {
    /// The task wrote a path it also read: the inputs it started from no
    /// longer exist as such, so caching this run would be unsound.
    InputModified { path: RelativePathBuf },
    /// Caching is sound; this is what the cache should remember.
    Cacheable {
        /// Fingerprints of everything the run read.
        input_fingerprints: InputFingerprints,
        /// The files the run produced, sorted.
        outputs: Vec<RelativePathBuf>,
    },
}

/// Which half of [`TaskFs::post_run`] failed.
#[derive(Debug, thiserror::Error)]
pub enum PostRunError {
    /// The run's input fingerprints could not be computed.
    #[error(transparent)]
    InputFingerprints(anyhow::Error),
    /// The run's outputs could not be collected.
    #[error(transparent)]
    Outputs(anyhow::Error),
}

impl<'a> TaskFs<'a> {
    /// Before the task runs: validate the task's io configuration, capture
    /// the current state of its listed inputs, and — when a previous run's
    /// fingerprints are given — report the first input that changed since
    /// that run.
    ///
    /// A returned change of `None` means no input changed on the filesystem
    /// (trivially so when `previous` is `None`); whether the previous result
    /// can be reused may involve further checks by the caller.
    ///
    /// # Errors
    ///
    /// Fails when the io configuration is invalid or the inputs' state cannot
    /// be read — in both cases before the task runs.
    #[tracing::instrument(level = "debug", skip_all)]
    pub fn pre_run(
        workspace_root: &'a AbsolutePath,
        input: &'a ResolvedGlobConfig,
        output: &'a ResolvedGlobConfig,
        previous: Option<&InputFingerprints>,
    ) -> anyhow::Result<(Self, Option<InputChange>)> {
        // Negative globs are only ever matched against traced accesses, so
        // they are compiled exactly when a trace will exist (either side has
        // auto). Tasks without auto never pay for — or fail on — this.
        let auto_filters = if input.includes_auto || output.includes_auto {
            Some(AutoFilters {
                input_negative_globs: PathGlobSet::new(&input.negative_globs)?,
                output_negative_globs: PathGlobSet::new(&output.negative_globs)?,
            })
        } else {
            None
        };

        let snapshot = glob::compute_globbed_inputs(
            workspace_root,
            &input.positive_globs,
            &input.negative_globs,
        )?;

        let task_fs = Self { workspace_root, input, output, auto_filters, snapshot };
        let change = previous
            .map(|previous| previous.find_change(&task_fs.snapshot, workspace_root))
            .transpose()?
            .flatten();
        Ok((task_fs, change))
    }

    /// After the task ran: judge the traced file accesses and produce what
    /// the cache should remember.
    ///
    /// Traced reads and writes each count only for a side whose configuration
    /// has auto tracking, and are filtered by that side's negative globs and
    /// the tool-reported ignore paths. `accesses: None` means the run was not
    /// traced; the configured outputs are still collected.
    ///
    /// # Errors
    ///
    /// Fails when the run's input fingerprints or outputs cannot be collected
    /// from the filesystem; the [`PostRunError`] variant says which half.
    #[tracing::instrument(level = "debug", skip_all)]
    pub fn post_run<'r>(
        self,
        accesses: Option<impl IntoIterator<Item = PathAccess<'r>>>,
        reported_ignored_inputs: &FxHashSet<Arc<AbsolutePath>>,
        reported_ignored_outputs: &FxHashSet<Arc<AbsolutePath>>,
    ) -> Result<Conclusion, PostRunError> {
        let tracked = accesses
            .map(|raw| TrackedPathAccesses::from_raw(raw, self.workspace_root))
            .unwrap_or_default();

        // A trace can be attached for auto-output-only tasks; in that mode
        // reads must not become discovered inputs. Symmetrically, writes must
        // not become outputs (or overlap candidates) for auto-input-only
        // tasks. Each side is therefore gated on its own config.
        let path_reads: HashMap<RelativePathBuf, PathRead> = if self.input.includes_auto
            && let Some(filters) = &self.auto_filters
        {
            let ignored = normalize_ignored_paths(reported_ignored_inputs, self.workspace_root);
            tracked
                .path_reads
                .iter()
                .filter(|(path, _)| {
                    !filters.input_negative_globs.is_match(path.as_str())
                        && !is_ignored(path, &ignored)
                })
                .map(|(path, read)| (path.clone(), *read))
                .collect()
        } else {
            HashMap::default()
        };
        let path_writes: FxHashSet<RelativePathBuf> = if self.output.includes_auto
            && let Some(filters) = &self.auto_filters
        {
            let ignored = normalize_ignored_paths(reported_ignored_outputs, self.workspace_root);
            tracked
                .path_writes
                .iter()
                .filter(|path| {
                    !filters.output_negative_globs.is_match(path.as_str())
                        && !is_ignored(path, &ignored)
                })
                .cloned()
                .collect()
        } else {
            FxHashSet::default()
        };

        // The verdict, checked before any fingerprinting so a doomed run does
        // no filesystem work: a path both read and written means the pre-run
        // snapshot is stale and caching would be unsound. Exact-path equality
        // only. (Only traced reads are checked, not the snapshot: a task that
        // writes a listed input it never reads causes perpetual cache misses,
        // which is wasteful but not a correctness bug.)
        if let Some(path) = path_reads.keys().find(|path| path_writes.contains(*path)).cloned() {
            return Ok(Conclusion::InputModified { path });
        }

        let discovered =
            fingerprint::fingerprint_discovered(&path_reads, self.workspace_root, &self.snapshot)
                .map_err(PostRunError::InputFingerprints)?;

        let outputs = collect_outputs(
            path_writes,
            self.workspace_root,
            &self.output.positive_globs,
            &self.output.negative_globs,
        )
        .map_err(PostRunError::Outputs)?;

        Ok(Conclusion::Cacheable {
            input_fingerprints: InputFingerprints::new(self.snapshot, discovered),
            outputs,
        })
    }
}

/// Files the run produced: filtered traced writes ∪ configured output-glob
/// matches, sorted.
fn collect_outputs(
    writes: FxHashSet<RelativePathBuf>,
    root: &AbsolutePath,
    positive_globs: &BTreeSet<Str>,
    negative_globs: &BTreeSet<Str>,
) -> anyhow::Result<Vec<RelativePathBuf>> {
    let mut files = writes;
    if !positive_globs.is_empty() {
        files.extend(glob::collect_glob_paths(root, positive_globs, negative_globs)?);
    }
    let mut sorted: Vec<RelativePathBuf> = files.into_iter().collect();
    sorted.sort();
    Ok(sorted)
}

/// Normalize tool-reported absolute paths to cleaned workspace-relative paths.
/// Paths outside the workspace are dropped — they can't contribute to inputs
/// or outputs.
fn normalize_ignored_paths(
    paths: &FxHashSet<Arc<AbsolutePath>>,
    workspace_root: &AbsolutePath,
) -> FxHashSet<RelativePathBuf> {
    paths
        .iter()
        .filter_map(|p| p.strip_prefix(workspace_root).ok().flatten()?.clean().ok())
        .collect()
}

/// Whether `path` is covered by any `ignored` entry. An ignored entry matches
/// itself (exact file) and everything under it (directory subtree).
fn is_ignored(path: &RelativePathBuf, ignored: &FxHashSet<RelativePathBuf>) -> bool {
    if ignored.is_empty() {
        return false;
    }
    ignored.contains(path) || ignored.iter().any(|ig| path.strip_prefix(ig).is_some())
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, fs};

    use fspy_shared::ipc::AccessMode;
    use tempfile::TempDir;
    use vt_path::AbsolutePathBuf;

    use super::{super::fingerprint::InputChangeKind, *};

    fn workspace() -> (TempDir, AbsolutePathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        (tmp, root)
    }

    fn config(auto: bool, positive: &[&str], negative: &[&str]) -> ResolvedGlobConfig {
        ResolvedGlobConfig {
            includes_auto: auto,
            positive_globs: positive.iter().map(|s| (*s).into()).collect(),
            negative_globs: negative.iter().map(|s| (*s).into()).collect(),
        }
    }

    fn rel(s: &str) -> RelativePathBuf {
        RelativePathBuf::new(s).unwrap()
    }

    fn no_ignored() -> FxHashSet<Arc<AbsolutePath>> {
        FxHashSet::default()
    }

    /// Owns the backing storage for synthetic traced accesses, so tests can
    /// drive `post_run` without spawning a traced process.
    #[derive(Default)]
    struct Trace {
        #[cfg(unix)]
        entries: Vec<(AccessMode, std::ffi::OsString)>,
        #[cfg(windows)]
        entries: Vec<(AccessMode, Vec<u16>)>,
    }

    impl Trace {
        fn push(&mut self, mode: AccessMode, path: impl AsRef<OsStr>) {
            #[cfg(unix)]
            self.entries.push((mode, path.as_ref().to_os_string()));
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt as _;
                self.entries.push((mode, path.as_ref().encode_wide().collect()));
            }
        }

        fn accesses(&self) -> impl Iterator<Item = PathAccess<'_>> {
            self.entries.iter().map(|(mode, path)| {
                #[cfg(unix)]
                let path: &fspy_shared::ipc::NativePath = path.into();
                #[cfg(windows)]
                let path = fspy_shared::ipc::NativePath::from_wide(path);
                PathAccess { mode: *mode, path }
            })
        }
    }

    /// `pre_run` with no previous run; asserts the trivially-empty change.
    fn first_run<'a>(
        root: &'a AbsolutePath,
        input: &'a ResolvedGlobConfig,
        output: &'a ResolvedGlobConfig,
    ) -> TaskFs<'a> {
        let (task_fs, change) = TaskFs::pre_run(root, input, output, None).unwrap();
        assert!(change.is_none(), "a first run has nothing to differ from");
        task_fs
    }

    fn cacheable(conclusion: Conclusion) -> (InputFingerprints, Vec<RelativePathBuf>) {
        match conclusion {
            Conclusion::Cacheable { input_fingerprints, outputs } => (input_fingerprints, outputs),
            Conclusion::InputModified { path } => panic!("unexpected InputModified at {path}"),
        }
    }

    /// Run the whole pipeline once: trace → conclude, returning the stored
    /// fingerprints for a later run to validate against.
    fn conclude(
        root: &AbsolutePath,
        input: &ResolvedGlobConfig,
        output: &ResolvedGlobConfig,
        trace: &Trace,
    ) -> (InputFingerprints, Vec<RelativePathBuf>) {
        let task_fs = first_run(root, input, output);
        cacheable(task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap())
    }

    /// What changed since the run that stored `previous`?
    fn change_since(
        root: &AbsolutePath,
        input: &ResolvedGlobConfig,
        output: &ResolvedGlobConfig,
        previous: &InputFingerprints,
    ) -> Option<InputChange> {
        let (_task_fs, change) = TaskFs::pre_run(root, input, output, Some(previous)).unwrap();
        change
    }

    // ── the verdict ─────────────────────────────────────────────────────────

    #[test]
    fn read_write_same_path_is_input_modified() {
        let (_tmp, root) = workspace();
        fs::write(root.join("file.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let task_fs = first_run(&root, &io, &io);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("file.txt").as_path());
        trace.push(AccessMode::WRITE, root.join("file.txt").as_path());

        let conclusion =
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap();
        assert!(
            matches!(conclusion, Conclusion::InputModified { path } if path == rel("file.txt"))
        );
    }

    #[test]
    fn single_read_write_access_is_input_modified() {
        let (_tmp, root) = workspace();
        fs::write(root.join("file.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let task_fs = first_run(&root, &io, &io);
        let mut trace = Trace::default();
        // One O_RDWR-style access carrying both bits.
        trace.push(AccessMode::READ | AccessMode::WRITE, root.join("file.txt").as_path());

        let conclusion =
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap();
        assert!(
            matches!(conclusion, Conclusion::InputModified { path } if path == rel("file.txt"))
        );
    }

    #[test]
    fn reading_dir_and_writing_child_is_not_overlap() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join("sub")).unwrap();
        let io = config(true, &[], &[]);

        let task_fs = first_run(&root, &io, &io);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("sub").as_path());
        trace.push(AccessMode::WRITE, root.join("sub/out.txt").as_path());

        let (_fingerprints, outputs) = cacheable(
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap(),
        );
        assert_eq!(outputs, vec![rel("sub/out.txt")]);
    }

    #[test]
    fn input_negative_glob_dissolves_overlap() {
        let (_tmp, root) = workspace();
        fs::write(root.join("build.log"), "x").unwrap();
        let input = config(true, &[], &["**/*.log"]);
        let output = config(true, &[], &[]);

        let task_fs = first_run(&root, &input, &output);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ | AccessMode::WRITE, root.join("build.log").as_path());

        // The read is excluded by the input negative, so no overlap remains;
        // the write still counts as an output.
        let (_fingerprints, outputs) = cacheable(
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap(),
        );
        assert_eq!(outputs, vec![rel("build.log")]);
    }

    #[test]
    fn output_negative_glob_dissolves_overlap() {
        let (_tmp, root) = workspace();
        fs::write(root.join("build.log"), "x").unwrap();
        let input = config(true, &[], &[]);
        let output = config(true, &[], &["**/*.log"]);

        let task_fs = first_run(&root, &input, &output);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ | AccessMode::WRITE, root.join("build.log").as_path());

        let (_fingerprints, outputs) = cacheable(
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap(),
        );
        assert!(outputs.is_empty(), "the write is excluded by the output negative");
    }

    #[test]
    fn reported_ignored_input_dissolves_overlap() {
        let (_tmp, root) = workspace();
        fs::write(root.join("state.json"), "x").unwrap();
        let io = config(true, &[], &[]);

        let task_fs = first_run(&root, &io, &io);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ | AccessMode::WRITE, root.join("state.json").as_path());

        let mut ignored = FxHashSet::default();
        ignored.insert(Arc::<AbsolutePath>::from(root.join("state.json")));

        let (_fingerprints, outputs) =
            cacheable(task_fs.post_run(Some(trace.accesses()), &ignored, &no_ignored()).unwrap());
        assert_eq!(outputs, vec![rel("state.json")]);
    }

    #[test]
    fn reported_ignored_input_covers_subtree() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join("gen")).unwrap();
        fs::write(root.join("gen/state.json"), "x").unwrap();
        let io = config(true, &[], &[]);

        let task_fs = first_run(&root, &io, &io);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ | AccessMode::WRITE, root.join("gen/state.json").as_path());

        let mut ignored = FxHashSet::default();
        ignored.insert(Arc::<AbsolutePath>::from(root.join("gen")));

        let conclusion = task_fs.post_run(Some(trace.accesses()), &ignored, &no_ignored()).unwrap();
        assert!(matches!(conclusion, Conclusion::Cacheable { .. }));
    }

    // ── per-side gating ─────────────────────────────────────────────────────

    #[test]
    fn input_auto_only_writes_dont_count() {
        let (_tmp, root) = workspace();
        fs::write(root.join("file.txt"), "x").unwrap();
        let input = config(true, &[], &[]);
        let output = config(false, &[], &[]);

        let task_fs = first_run(&root, &input, &output);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ | AccessMode::WRITE, root.join("file.txt").as_path());

        // Writes don't count for a non-auto output side: no overlap verdict,
        // and no traced outputs.
        let (_fingerprints, outputs) = cacheable(
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap(),
        );
        assert!(outputs.is_empty());
    }

    #[test]
    fn output_auto_only_reads_dont_count() {
        let (_tmp, root) = workspace();
        fs::write(root.join("read.txt"), "x").unwrap();
        let input = config(false, &[], &[]);
        let output = config(true, &[], &[]);

        let task_fs = first_run(&root, &input, &output);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("read.txt").as_path());
        trace.push(AccessMode::WRITE, root.join("written.txt").as_path());

        let (previous, outputs) = cacheable(
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap(),
        );
        assert_eq!(outputs, vec![rel("written.txt")]);

        // The read was never fingerprinted, so changing it goes unnoticed.
        fs::write(root.join("read.txt"), "changed").unwrap();
        assert!(change_since(&root, &input, &output, &previous).is_none());
    }

    #[test]
    fn untraced_run_still_collects_configured_outputs() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join("out")).unwrap();
        fs::write(root.join("out/a.js"), "x").unwrap();
        let input = config(false, &[], &[]);
        let output = config(false, &["out/**"], &[]);

        let task_fs = first_run(&root, &input, &output);
        let conclusion = task_fs
            .post_run(None::<std::iter::Empty<PathAccess<'static>>>, &no_ignored(), &no_ignored())
            .unwrap();
        let (_fingerprints, outputs) = cacheable(conclusion);
        assert_eq!(outputs, vec![rel("out/a.js")]);
    }

    #[test]
    fn outputs_union_deduplicates_and_sorts() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join("out")).unwrap();
        fs::write(root.join("out/a.js"), "a").unwrap();
        fs::write(root.join("out/b.js"), "b").unwrap();
        let input = config(false, &[], &[]);
        let output = config(true, &["out/**"], &[]);

        let task_fs = first_run(&root, &input, &output);
        let mut trace = Trace::default();
        // b.js is both traced and glob-matched; zzz.txt only traced.
        trace.push(AccessMode::WRITE, root.join("zzz.txt").as_path());
        trace.push(AccessMode::WRITE, root.join("out/b.js").as_path());

        let (_fingerprints, outputs) = cacheable(
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap(),
        );
        assert_eq!(outputs, vec![rel("out/a.js"), rel("out/b.js"), rel("zzz.txt")]);
    }

    // ── trace normalization ─────────────────────────────────────────────────

    #[test]
    fn accesses_outside_the_workspace_are_dropped() {
        let (_tmp, root) = workspace();
        let (_other_tmp, other) = workspace();
        fs::write(other.join("outside.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let task_fs = first_run(&root, &io, &io);
        let mut trace = Trace::default();
        trace.push(AccessMode::READ, other.join("outside.txt").as_path());
        trace.push(AccessMode::WRITE, other.join("outside.txt").as_path());

        // Neither the read-write overlap nor the write registers.
        let (previous, outputs) = cacheable(
            task_fs.post_run(Some(trace.accesses()), &no_ignored(), &no_ignored()).unwrap(),
        );
        assert!(outputs.is_empty());

        // The outside read was never fingerprinted either.
        fs::remove_file(other.join("outside.txt")).unwrap();
        assert!(change_since(&root, &io, &io, &previous).is_none());
    }

    #[test]
    fn git_accesses_are_skipped() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/index"), "x").unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join(".git/index").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        fs::remove_file(root.join(".git/index")).unwrap();
        assert!(change_since(&root, &io, &io, &previous).is_none());
    }

    #[test]
    fn parent_components_are_cleaned() {
        let (tmp, root) = workspace();
        fs::create_dir(root.join("pkg")).unwrap();
        fs::write(root.join("data.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ, tmp.path().join("pkg/../data.txt"));
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        fs::write(root.join("data.txt"), "changed").unwrap();
        let change = change_since(&root, &io, &io, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::ContentModified, path }) if path == rel("data.txt")
        ));
    }

    // ── discovered inputs, fingerprinted and re-checked ─────────────────────

    #[test]
    fn discovered_content_change_is_reported() {
        let (_tmp, root) = workspace();
        fs::write(root.join("file.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("file.txt").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        assert!(change_since(&root, &io, &io, &previous).is_none(), "unchanged file");

        fs::write(root.join("file.txt"), "changed").unwrap();
        let change = change_since(&root, &io, &io, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::ContentModified, path }) if path == rel("file.txt")
        ));
    }

    #[test]
    fn discovered_removal_is_reported() {
        let (_tmp, root) = workspace();
        fs::write(root.join("file.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("file.txt").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        fs::remove_file(root.join("file.txt")).unwrap();
        let change = change_since(&root, &io, &io, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::Removed, path }) if path == rel("file.txt")
        ));
    }

    #[test]
    fn discovered_missing_path_that_appears_is_reported_as_added() {
        let (_tmp, root) = workspace();
        let io = config(true, &[], &[]);

        // The task probed a path that didn't exist.
        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("config.local").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        fs::write(root.join("config.local"), "now exists").unwrap();
        let change = change_since(&root, &io, &io, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::Added, path }) if path == rel("config.local")
        ));
    }

    #[test]
    fn discovered_file_replaced_by_dir_is_reported() {
        let (_tmp, root) = workspace();
        fs::write(root.join("thing"), "x").unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("thing").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        fs::remove_file(root.join("thing")).unwrap();
        fs::create_dir(root.join("thing")).unwrap();
        let change = change_since(&root, &io, &io, &previous);
        assert!(
            matches!(change, Some(InputChange { kind: InputChangeKind::Added, path }) if path == rel("thing"))
        );
    }

    #[test]
    fn dir_opened_without_listing_ignores_new_entries() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join("dir")).unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ, root.join("dir").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        fs::write(root.join("dir/new.txt"), "x").unwrap();
        assert!(change_since(&root, &io, &io, &previous).is_none());
    }

    #[test]
    fn listed_dir_reports_added_and_removed_entries() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/old.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ_DIR, root.join("dir").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        fs::write(root.join("dir/new.txt"), "x").unwrap();
        let change = change_since(&root, &io, &io, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::Added, path }) if path == rel("dir/new.txt")
        ));

        fs::remove_file(root.join("dir/new.txt")).unwrap();
        fs::remove_file(root.join("dir/old.txt")).unwrap();
        let change = change_since(&root, &io, &io, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::Removed, path }) if path == rel("dir/old.txt")
        ));
    }

    #[test]
    fn listed_dir_does_not_see_ds_store_or_dist() {
        let (_tmp, root) = workspace();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/keep.txt"), "x").unwrap();
        let io = config(true, &[], &[]);

        let mut trace = Trace::default();
        trace.push(AccessMode::READ_DIR, root.join("dir").as_path());
        let (previous, _outputs) = conclude(&root, &io, &io, &trace);

        // Neither `.DS_Store` nor a `dist` entry (any casing) is visible to
        // directory fingerprints.
        fs::write(root.join("dir/.DS_Store"), "junk").unwrap();
        fs::create_dir(root.join("dir/DIST")).unwrap();
        assert!(change_since(&root, &io, &io, &previous).is_none());
    }

    // ── listed inputs, snapshotted and diffed ───────────────────────────────

    /// A listed-inputs-only task: no auto tracking on either side.
    fn listed_only() -> (ResolvedGlobConfig, ResolvedGlobConfig) {
        (config(false, &["src/**"], &[]), config(false, &[], &[]))
    }

    fn listed_workspace() -> (TempDir, AbsolutePathBuf) {
        let (tmp, root) = workspace();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/a.txt"), "a").unwrap();
        fs::write(root.join("src/b.txt"), "b").unwrap();
        (tmp, root)
    }

    fn conclude_untraced(
        root: &AbsolutePath,
        input: &ResolvedGlobConfig,
        output: &ResolvedGlobConfig,
    ) -> InputFingerprints {
        let task_fs = first_run(root, input, output);
        let conclusion = task_fs
            .post_run(None::<std::iter::Empty<PathAccess<'static>>>, &no_ignored(), &no_ignored())
            .unwrap();
        cacheable(conclusion).0
    }

    #[test]
    fn unchanged_listed_inputs_report_nothing() {
        let (_tmp, root) = listed_workspace();
        let (input, output) = listed_only();
        let previous = conclude_untraced(&root, &input, &output);

        assert!(change_since(&root, &input, &output, &previous).is_none());
    }

    #[test]
    fn modified_listed_input_is_reported() {
        let (_tmp, root) = listed_workspace();
        let (input, output) = listed_only();
        let previous = conclude_untraced(&root, &input, &output);

        fs::write(root.join("src/b.txt"), "changed").unwrap();
        let change = change_since(&root, &input, &output, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::ContentModified, path }) if path == rel("src/b.txt")
        ));
    }

    #[test]
    fn removed_listed_input_is_reported() {
        let (_tmp, root) = listed_workspace();
        let (input, output) = listed_only();
        let previous = conclude_untraced(&root, &input, &output);

        fs::remove_file(root.join("src/a.txt")).unwrap();
        let change = change_since(&root, &input, &output, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::Removed, path }) if path == rel("src/a.txt")
        ));
    }

    #[test]
    fn added_listed_input_is_reported() {
        let (_tmp, root) = listed_workspace();
        let (input, output) = listed_only();
        let previous = conclude_untraced(&root, &input, &output);

        fs::write(root.join("src/c.txt"), "c").unwrap();
        let change = change_since(&root, &input, &output, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::Added, path }) if path == rel("src/c.txt")
        ));
    }

    #[test]
    fn first_listed_change_in_path_order_wins() {
        let (_tmp, root) = listed_workspace();
        let (input, output) = listed_only();
        let previous = conclude_untraced(&root, &input, &output);

        // Both changed; `src/a.txt` sorts first, so its removal is the answer.
        fs::remove_file(root.join("src/a.txt")).unwrap();
        fs::write(root.join("src/b.txt"), "changed").unwrap();
        let change = change_since(&root, &input, &output, &previous);
        assert!(matches!(
            change,
            Some(InputChange { kind: InputChangeKind::Removed, path }) if path == rel("src/a.txt")
        ));
    }

    #[test]
    fn normalize_ignored_paths_cleans_relative_components() {
        let workspace_root =
            AbsolutePath::new(if cfg!(windows) { r"C:\repo" } else { "/repo" }).unwrap();
        let ignored =
            workspace_root.join(if cfg!(windows) { r"pkg\..\cache" } else { "pkg/../cache" });
        let mut ignored_paths = FxHashSet::default();
        ignored_paths.insert(Arc::<AbsolutePath>::from(ignored));

        let normalized = normalize_ignored_paths(&ignored_paths, workspace_root);

        let expected = RelativePathBuf::new("cache").unwrap();
        assert!(normalized.contains(&expected));
    }
}
