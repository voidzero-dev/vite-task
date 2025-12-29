use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use bincode::{Decode, Encode};
use diff::Diff;
use serde::{Deserialize, Serialize};
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use vite_task_plan::SpawnExecution;

/// Fingerprint for command execution that affects caching.
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
#[derive(Encode, Decode, Debug, Serialize, Deserialize, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
pub struct SpawnFingerprint {
    pub cwd: RelativePathBuf,
    pub command_fingerprint: CommandFingerprint,
    /// Environment variables that affect caching (excludes pass-through envs)
    pub fingerprinted_envs: BTreeMap<Str, Str>, // using BTreeMap to have a stable order in cache db

    /// even though value changes to `pass_through_envs` shouldn't invalidate the cache,
    /// The names should still be fingerprinted so that the cache can be invalidated if the `pass_through_envs` config changes
    pub pass_through_envs: BTreeSet<Str>, // using BTreeSet to have a stable order in cache db

    /// Glob patterns for fingerprint filtering. Order matters (last match wins).
    /// Changes to this config invalidate the cache to ensure correct fingerprint tracking.
    pub fingerprint_ignores: Option<Vec<Str>>,
}

/// The program fingerprint used in `SpawnFingerprint`
#[derive(Encode, Decode, Debug, Serialize, Deserialize, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
enum ProgramFingerprint {
    /// If the program is outside the workspace, fingerprint by its name only (like `node`, `npm`, etc)
    OutsideWorkspace { program_name: Str },

    /// If the program is inside the workspace, fingerprint by its path relative to the workspace root
    InsideWorkspace { relative_path: RelativePathBuf },
}

#[derive(Encode, Decode, Debug, Serialize, Deserialize, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
enum CommandFingerprint {
    /// A program with args to be executed directly
    Program { program_fingerprint: ProgramFingerprint, args: Vec<Str> },
    /// A script to be executed by os shell (sh or cmd)
    ShellScript { script: Str, extra_args: Vec<Str> },
}
