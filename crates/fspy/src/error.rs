use std::{ffi::OsString, path::PathBuf};

#[derive(thiserror::Error, Debug)]
pub enum SpawnError {
    #[error(
        "could not resolve the full path of program '{}' with PATH={} under cwd({})",
        .program.display(),
        .path.as_deref().unwrap_or_else(|| std::ffi::OsStr::new("<not set>")).display(),
        .cwd.display()
    )]
    Which {
        program: OsString,
        path: Option<OsString>,
        cwd: PathBuf,
        #[source]
        cause: which::Error,
    },

    #[error("failed to create IPC channel: {0}")]
    ChannelCreation(std::io::Error),

    /// On unix systems, the injection happens before the spawn actually occurs on.
    /// On Windows, the injection happens after the spawn but before resuming the process.
    #[error("failed to prepare the command for injection: {0}")]
    Injection(std::io::Error),

    #[error("underlying os error: {0}")]
    OsSpawn(std::io::Error),
}

/// Why complete file accesses could not be collected for a tracked child
/// process.
///
/// The run itself is unaffected: recording must never stop the program doing
/// the work. What cannot be done is anything that needs every access, caching
/// above all, which has to treat the run as untracked rather than as having
/// touched only a subset of paths.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum UntrackedReason {
    /// A tracked process could not record a file access it went on to perform,
    /// so the accesses collected for the run are a subset of what it really
    /// touched (for example, the tracking channel filled up, or on Linux the
    /// seccomp filter could not be installed or the launcher could not be
    /// materialized).
    #[error("automatic file tracking could not record every file access")]
    Incomplete,

    /// The current process is already running inside an outer `fspy` tracker,
    /// so this child was spawned untracked and its accesses are captured by
    /// the outer tracker instead.
    #[error("automatic file tracking is already active in a parent process")]
    Nested,
}
