use std::sync::Arc;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use vite_path::RelativePathBuf;
use vite_str::Str;

use crate::envs::EnvFingerprints;

/// Key to identify an execution across sessions.
#[derive(Debug, Encode, Decode, Serialize)]
pub enum ExecutionCacheKey {
    /// This execution is from a script of a user-defined task.
    UserTask {
        /// The name of the user-defined task.
        task_name: Str,
        /// The index of the execution item in the task's command split by `&&`.
        /// This is to distinguish multiple execution items from the same task.
        and_item_index: usize,
        /// Extra args provided when invoking the user-defined task (`vite [task_name] [extra_args...]`).
        /// These args are appended to the last and_item. Non-last and_items don't get extra args.
        extra_args: Arc<[Str]>,
        /// The package path where the user-defined task is defined, relative to the workspace root.
        package_path: RelativePathBuf,
    },
    /// This execution is from a synthetic task directly invoked from `Session::plan_exec` API.
    ///
    /// The cache key is an opaque value provided by the caller.
    ExecAPI(Arc<[Str]>),
}

/// Cache information for a spawn execution.
/// It only contains information needed for hitting existing cache entries pre-execution.
/// It doesn't contain any post-execution information like file fingerprints
/// (which needs actual execution and is out of scope for planning).
#[derive(Debug, Encode, Decode, Serialize)]
pub struct CacheMetadata {
    /// Fingerprint for spawn execution that affects caching.
    pub spawn_fingerprint: SpawnFingerprint,

    /// Key to identify an execution across sessions.
    pub execution_cache_key: ExecutionCacheKey,
}

/// Fingerprint for spawn execution that affects caching.
///
/// # Environment Variable Impact on Cache
///
/// The `envs_without_pass_through` field is crucial for cache correctness:
/// - Only includes envs explicitly declared in the task's `envs` array
/// - Does NOT include pass-through envs (PATH, CI, etc.)
/// - These envs become part of the cache key
///
/// When a task runs:
/// 1. All envs (including pass-through) are available to the process
/// 2. Only declared envs affect the cache key
/// 3. If a declared env changes value, cache will miss
/// 4. If a pass-through env changes, cache will still hit
///
/// For built-in tasks (lint, build, etc):
/// - The resolver provides envs which become part of the fingerprint
/// - If resolver provides different envs between runs, cache breaks
/// - Each built-in task type must have unique task name to avoid cache collision
///
/// # Fingerprint Ignores Impact on Cache
///
/// The `fingerprint_ignores` field controls which files are tracked in `PostRunFingerprint`:
/// - Changes to this config must invalidate the cache
/// - Vec maintains insertion order (pattern order matters for last-match-wins semantics)
/// - Even though ignore patterns only affect `PostRunFingerprint`, the config itself is part of the cache key
#[derive(Encode, Decode, Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct SpawnFingerprint {
    pub(crate) cwd: RelativePathBuf,
    pub(crate) program_fingerprint: ProgramFingerprint,
    pub(crate) args: Arc<[Str]>,
    pub(crate) env_fingerprints: EnvFingerprints,

    /// Glob patterns for fingerprint filtering. Order matters (last match wins).
    /// Changes to this config invalidate the cache to ensure correct fingerprint tracking.
    pub(crate) fingerprint_ignores: Option<Vec<Str>>,
}

impl SpawnFingerprint {
    /// Get the fingerprint ignores patterns.
    pub fn fingerprint_ignores(&self) -> Option<&Vec<Str>> {
        self.fingerprint_ignores.as_ref()
    }

    /// Get the environment fingerprints.
    pub fn env_fingerprints(&self) -> &EnvFingerprints {
        &self.env_fingerprints
    }

    /// Get the program fingerprint as a debug string.
    pub fn program_fingerprint_debug(&self) -> String {
        format!("{:?}", self.program_fingerprint)
    }

    /// Get the command args.
    pub fn args(&self) -> &Arc<[Str]> {
        &self.args
    }

    /// Get the working directory.
    pub fn cwd(&self) -> &RelativePathBuf {
        &self.cwd
    }
}

/// The program fingerprint used in `SpawnFingerprint`
#[derive(Encode, Decode, Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub(crate) enum ProgramFingerprint {
    /// If the program is outside the workspace, fingerprint by its name only (like `node`, `npm`, etc)
    OutsideWorkspace { program_name: Str },

    /// If the program is inside the workspace, fingerprint by its path relative to the workspace root
    InsideWorkspace { relative_program_path: RelativePathBuf },
}
