//! Decide which observed paths a task consumed and which it produced.
//!
//! Two problems are solved here.
//!
//! **Read-write overlaps.** A path a task both read and wrote is either a source
//! it rewrote or state it manages, and the two need opposite treatment. Access
//! mechanics cannot tell them apart: a formatter rewriting a source and a linter
//! rewriting its own cache both read, truncate and write. So order decides
//! first — a task that wrote before reading is observing its own output — and
//! where order does not settle it, version control does. A tracked file the task
//! rewrote is a modified input; an ignored one is derived state.
//!
//! **Output directory cleanup.** Build tools stat and list their output
//! directory before writing to it, which would make the directory an input whose
//! content is the task's own product. Such a fingerprint can never match on a
//! clean checkout, because the directory is not there. Two rules drop those
//! reads: a bare directory stat carries only existence and is ignored, and a
//! directory whose subtree this task wrote into is the task's own product rather
//! than something it consumed.
//!
//! Nothing here consults whether a call succeeded. Only the call's arguments are
//! available, because the Linux seccomp supervisor is notified before the
//! syscall runs and can never report an outcome; a rule built on outcomes would
//! hold on some backends and silently not on others.
//!
//! Explicit input and output globs are applied by the caller and always win;
//! this module only classifies what the user did not declare.

#![cfg(fspy)]

use fspy::AccessMode;
use rustc_hash::{FxHashMap, FxHashSet};
use vite_path::{AbsolutePath, RelativePathBuf};

/// One normalized, workspace-relative access, in the order it was observed.
#[derive(Debug)]
pub struct TrackedEvent {
    pub path: RelativePathBuf,
    pub mode: AccessMode,
}

/// What the caller must supply for the parts of the decision that do not come
/// from access data.
pub struct ClassifyContext<'a> {
    pub workspace_root: &'a AbsolutePath,
    /// Whether reads may become inferred inputs.
    pub infer_inputs: bool,
    /// Whether writes may become inferred outputs.
    pub infer_outputs: bool,
    /// Version control's answer for a path, used only to break a read-first
    /// overlap. `None` means not ignored, or no repository, which resolves to
    /// input.
    pub is_gitignored: &'a dyn Fn(&RelativePathBuf) -> bool,
}

/// The classification result.
#[derive(Default)]
pub struct Classification {
    /// Paths to fingerprint. A path present here is a dependency of the task.
    pub inputs: FxHashSet<RelativePathBuf>,
    /// Of those inputs, the ones whose directory entries were listed and so
    /// must be fingerprinted as a listing rather than as content.
    pub listed_directories: FxHashSet<RelativePathBuf>,
    /// Paths to archive as this task's outputs.
    pub outputs: FxHashSet<RelativePathBuf>,
    /// A tracked path the task rewrote after reading it, so the prerun
    /// fingerprint is stale and this run must not be cached.
    pub modified_input: Option<RelativePathBuf>,
    /// A mutation whose target is gone at archive time with no rename to explain
    /// it. The output set is incomplete, so this run must not be cached.
    pub unexplained_mutation: Option<RelativePathBuf>,
}

/// Everything observed about one path, folded in observation order.
#[derive(Default)]
struct PathHistory {
    /// Index of the first access that observed existing content.
    first_content_read: Option<usize>,
    /// Index of the first access that changed the path.
    first_mutation: Option<usize>,
    /// Index of the first successful directory listing.
    first_listing: Option<usize>,
    /// Index of the first successful removal.
    first_deletion: Option<usize>,
    /// The path was named as a rename source, so its absence is explained.
    renamed_away: bool,
    /// The path was replaced by a rename.
    rename_destination: bool,
    /// Any access described the path as a directory.
    known_directory: bool,
}

impl PathHistory {
    const fn observe(&mut self, index: usize, mode: AccessMode) {
        if mode.contains(AccessMode::IS_DIR) {
            self.known_directory = true;
        }
        if mode.contains(AccessMode::RENAME_FROM) {
            self.renamed_away = true;
        }
        if mode.contains(AccessMode::RENAME_TO) {
            self.rename_destination = true;
        }
        if mode.contains(AccessMode::DELETED) && self.first_deletion.is_none() {
            self.first_deletion = Some(index);
        }
        if mode.contains(AccessMode::READ_DIR) && self.first_listing.is_none() {
            self.first_listing = Some(index);
        }
        if mode.is_content_read() && self.first_content_read.is_none() {
            self.first_content_read = Some(index);
        }
        if mode.is_mutation() && self.first_mutation.is_none() {
            self.first_mutation = Some(index);
        }
    }
}

/// Apply the rules to an ordered event stream.
///
/// `already_fingerprinted` reports paths the caller hashed before the run, which
/// do not need fingerprinting again.
/// Fold the event stream into per-path histories, and collect the directory
/// renames that need writes re-attributed.
fn fold_events(
    events: &[TrackedEvent],
) -> (FxHashMap<&RelativePathBuf, PathHistory>, Vec<(&RelativePathBuf, &RelativePathBuf)>) {
    let mut histories: FxHashMap<&RelativePathBuf, PathHistory> = FxHashMap::default();
    // Directory renames, newest last. A build that stages into `dist.tmp` and
    // swaps it into place records every write against the staging path, so those
    // writes have to be re-attributed or the real outputs are lost entirely.
    let mut directory_renames: Vec<(&RelativePathBuf, &RelativePathBuf)> = Vec::new();
    let mut last_rename_source: Option<&RelativePathBuf> = None;

    for (index, event) in events.iter().enumerate() {
        histories.entry(&event.path).or_default().observe(index, event.mode);

        // A rename is reported as source then destination, adjacent.
        if event.mode.contains(AccessMode::RENAME_FROM) {
            last_rename_source = Some(&event.path);
        } else if event.mode.contains(AccessMode::RENAME_TO) {
            if event.mode.contains(AccessMode::IS_DIR)
                && let Some(source) = last_rename_source
            {
                directory_renames.push((source, &event.path));
            }
            last_rename_source = None;
        }
    }
    (histories, directory_renames)
}

/// Every directory the task wrote into, including ancestors.
///
/// A listing of one of these enumerated the task's own product, so treating it
/// as a dependency would make the cache key depend on the output and could never
/// match on a clean checkout where the directory does not exist yet.
fn mutated_subtrees<'a>(
    candidates: &[(&'a RelativePathBuf, &'a PathHistory)],
) -> FxHashSet<&'a str> {
    let mut subtrees: FxHashSet<&str> = FxHashSet::default();
    for (path, history) in candidates {
        if history.first_mutation.is_none() {
            continue;
        }
        let mut remaining = path.as_str();
        while let Some(separator) = remaining.rfind('/') {
            remaining = &remaining[..separator];
            if !subtrees.insert(remaining) {
                // An ancestor already recorded, so the rest are too.
                break;
            }
        }
    }
    subtrees
}

/// Whether a mutated path that is now gone can be accounted for.
///
/// Without an explanation the thing that produced it is missing from the archive
/// and a cache hit would restore an incomplete tree. Being renamed or removed
/// explains it, including by an ancestor: renaming a staging directory takes
/// everything beneath it along, and only the directory itself carries the rename.
fn absence_is_explained(
    path: &RelativePathBuf,
    history: &PathHistory,
    retired_ancestors: &[&str],
) -> bool {
    history.renamed_away
        || history.first_deletion.is_some()
        || retired_ancestors.iter().any(|retired| {
            path.as_str().strip_prefix(*retired).is_some_and(|tail| tail.starts_with('/'))
        })
}

/// Re-attribute mutations recorded beneath a renamed directory to the published
/// location.
///
/// A build that stages into `dist.tmp` and swaps it into place records every
/// write against the staging path. Only the directory carries the rename, so
/// without this the real outputs are never collected and a cache hit restores an
/// empty tree.
fn relocate_directory_renames(
    histories: &FxHashMap<&RelativePathBuf, PathHistory>,
    directory_renames: &[(&RelativePathBuf, &RelativePathBuf)],
) -> FxHashMap<RelativePathBuf, PathHistory> {
    let mut relocated: FxHashMap<RelativePathBuf, PathHistory> = FxHashMap::default();
    for (source, destination) in directory_renames {
        let source_prefix = source.as_str();
        for (path, history) in histories {
            if history.first_mutation.is_none() {
                continue;
            }
            let Some(tail) =
                path.as_str().strip_prefix(source_prefix).and_then(|tail| tail.strip_prefix('/'))
            else {
                continue;
            };
            let Ok(moved) =
                RelativePathBuf::new(vite_str::format!("{destination}/{tail}").as_str())
            else {
                continue;
            };
            relocated.entry(moved).or_default().first_mutation = history.first_mutation;
        }
    }
    relocated
}

pub fn classify(
    events: &[TrackedEvent],
    context: &ClassifyContext<'_>,
    already_fingerprinted: &dyn Fn(&RelativePathBuf) -> bool,
) -> Classification {
    let (histories, directory_renames) = fold_events(events);
    let mut classification = Classification::default();

    let relocated = relocate_directory_renames(&histories, &directory_renames);

    let mut candidates: Vec<(&RelativePathBuf, &PathHistory)> =
        histories.iter().map(|(path, history)| (*path, history)).collect();
    let relocated_entries: Vec<(&RelativePathBuf, &PathHistory)> = relocated.iter().collect();
    candidates.extend(relocated_entries);

    // Directories that were renamed away or removed. Anything that used to live
    // beneath one of these is accounted for, so its absence needs no further
    // explanation.
    let retired_ancestors: Vec<&str> = histories
        .iter()
        .filter(|(_, history)| history.renamed_away || history.first_deletion.is_some())
        .map(|(path, _)| path.as_str())
        .collect();

    let mutated_subtrees = mutated_subtrees(&candidates);

    for (path, history) in candidates {
        let absolute = context.workspace_root.join(path);
        let metadata = std::fs::metadata(absolute.as_path()).ok();
        let exists = metadata.is_some();
        let is_directory = metadata.as_ref().is_some_and(std::fs::Metadata::is_dir)
            || (history.known_directory && !exists);

        if history.first_mutation.is_some()
            && !exists
            && !absence_is_explained(path, history, &retired_ancestors)
        {
            {
                if std::env::var_os("VITE_TASK_DEBUG_TRACKING").is_some() {
                    #[expect(clippy::print_stderr, reason = "opt-in diagnostic")]
                    {
                        eprintln!(
                            "[tracking] unexplained {path} renamed_away={} deleted={:?} \
                             mutation={:?} read={:?}",
                            history.renamed_away,
                            history.first_deletion,
                            history.first_mutation,
                            history.first_content_read,
                        );
                    }
                }
                classification.unexplained_mutation.get_or_insert_with(|| path.clone());
            }
        }

        let wrote_into_subtree = is_directory && mutated_subtrees.contains(path.as_str());
        let own_derived_state = is_own_derived_state(
            path,
            is_directory,
            wrote_into_subtree,
            context,
            &mutated_subtrees,
        );
        let mutated = history.first_mutation.is_some() && context.infer_outputs;
        let consumed =
            is_input_candidate(history, is_directory, own_derived_state) && context.infer_inputs;

        match (mutated, consumed) {
            (true, false) => {
                if exists {
                    classification.outputs.insert(path.clone());
                }
            }
            (true, true) => {
                let write_first = history.first_mutation < history.first_content_read;
                if std::env::var_os("VITE_TASK_DEBUG_TRACKING").is_some() {
                    #[expect(clippy::print_stderr, reason = "opt-in diagnostic")]
                    {
                        eprintln!(
                            "[tracking] overlap {path} write_first={write_first} \
                             gitignored={}",
                            (context.is_gitignored)(path),
                        );
                    }
                }
                if write_first || (context.is_gitignored)(path) {
                    if exists {
                        classification.outputs.insert(path.clone());
                    }
                } else {
                    // A tracked file this run read and then rewrote. The prerun
                    // fingerprint no longer describes it.
                    classification.modified_input.get_or_insert_with(|| path.clone());
                    record_input(&mut classification, path, history, already_fingerprinted);
                }
            }
            (false, true) => {
                record_input(&mut classification, path, history, already_fingerprinted);
            }
            (false, false) => {}
        }
    }

    classification
}

/// Whether a path's reads make it a dependency.
///
/// Directories are the interesting case, and three kinds of read are rejected.
///
/// A bare stat carries only existence and type. It cannot detect a content
/// change and is routinely a probe of the task's own output directory, where a
/// stored fingerprint could never match on a clean checkout.
///
/// A listing of a directory the task wrote into enumerated the task's own
/// product.
///
/// A listing of a *gitignored* directory enumerated derived state. Version
/// control already says this content is not source, so depending on the set of
/// names in it is not a source dependency — and that set moves for reasons this
/// task does not control. In emdash, `tsdown` lists `dist/locales` while a
/// sibling task writes into it, so without this the tsdown task invalidates
/// every time a locale is added, even though its own inputs never changed.
const fn is_input_candidate(
    history: &PathHistory,
    is_directory: bool,
    own_derived_state: bool,
) -> bool {
    if history.first_content_read.is_none() {
        return false;
    }
    if own_derived_state {
        return false;
    }
    if !is_directory {
        return true;
    }
    // A bare stat of a directory carries only existence and type. It cannot
    // detect a content change and is routinely a probe of the task's own output
    // directory, where a stored fingerprint could never match on a clean
    // checkout.
    history.first_listing.is_some()
}

/// Whether a read path is part of the task's own derived output tree.
///
/// Two signals have to agree. Version control must call the path derived, and it
/// must sit under a directory this same task wrote into. Requiring both is what
/// keeps the ordinary monorepo dependency intact: a package reading
/// `node_modules/<dep>/dist/index.mjs` is reading ignored, derived content, but
/// the reader never writes there, so it stays a real input and a change to the
/// dependency still invalidates the consumer.
///
/// What it does reject is a task reading inside its own output directory.
/// emdash's admin package is the case that forced this: `tsdown` reads
/// `dist/locales/<locale>/messages.mjs` while a later step in the same build
/// rewrites those files, so fingerprinting them can never settle and the task
/// misses on every run.
fn is_own_derived_state(
    path: &RelativePathBuf,
    is_directory: bool,
    wrote_into_subtree: bool,
    context: &ClassifyContext<'_>,
    mutated_subtrees: &FxHashSet<&str>,
) -> bool {
    if is_directory {
        // The listing enumerated names this task produced, or names version
        // control considers derived and therefore outside the source graph.
        return wrote_into_subtree || (context.is_gitignored)(path);
    }
    if !(context.is_gitignored)(path) {
        return false;
    }
    let mut remaining = path.as_str();
    while let Some(separator) = remaining.rfind('/') {
        remaining = &remaining[..separator];
        if mutated_subtrees.contains(remaining) {
            return true;
        }
    }
    false
}

fn record_input(
    classification: &mut Classification,
    path: &RelativePathBuf,
    history: &PathHistory,
    already_fingerprinted: &dyn Fn(&RelativePathBuf) -> bool,
) {
    if history.first_listing.is_some() {
        classification.listed_directories.insert(path.clone());
    }
    if !already_fingerprinted(path) {
        classification.inputs.insert(path.clone());
    }
}
