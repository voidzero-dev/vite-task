use std::{
    ffi::{OsStr, OsString},
    fmt,
    path::PathBuf,
};

#[derive(thiserror::Error, Debug)]
pub enum SpawnError {
    #[error(
        "could not resolve the full path of program '{}' with PATH={} under cwd({})",
        .program.display(),
        display_path_env(.path.as_deref()),
        .cwd.display()
    )]
    Which {
        program: OsString,
        path: Option<OsString>,
        cwd: PathBuf,
        #[source]
        cause: which::Error,
    },

    #[error("failed to initialize seccomp_unotify supervisor: {0}")]
    Supervisor(std::io::Error),

    #[error("failed to create IPC channel: {0}")]
    ChannelCreation(std::io::Error),

    /// On unix systems, the injection happens before the spawn actually occurs on.
    /// On Windows, the injection happens after the spawn but before resuming the process.
    #[error("failed to prepare the command for injection: {0}")]
    Injection(std::io::Error),

    #[error("underlying os error: {0}")]
    OsSpawn(std::io::Error),
}

struct DisplayPathEnv<'a>(Option<&'a OsStr>);

impl fmt::Display for DisplayPathEnv<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(path) => fmt::Display::fmt(&path.display(), f),
            None => f.write_str("<not set>"),
        }
    }
}

const fn display_path_env(path: Option<&OsStr>) -> DisplayPathEnv<'_> {
    DisplayPathEnv(path)
}
