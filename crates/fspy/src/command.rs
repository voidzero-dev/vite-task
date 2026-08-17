use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
};

use fspy_shared::ipc::ChannelSize;
#[cfg(unix)]
use fspy_shared_unix::exec::Exec;
use rustc_hash::FxHashMap;
use tokio::process::Command as TokioCommand;
use tokio_util::sync::CancellationToken;

use crate::{SPY_IMPL, TrackedChild, error::SpawnError};

/// Shared memory for a tracked run's file-access records when the caller
/// does not say otherwise.
///
/// 4 GiB of sparse address space: none of it becomes real memory until
/// records land in it, and it leaves room for tens of millions of
/// accesses.
pub const DEFAULT_SHM_CAPACITY: usize = 4 << 30;

/// Splits a byte budget into a descriptor table and payload room, at one
/// record per 64 bytes: 8 for its descriptor and 56 for its payload.
///
/// Records run a few hundred bytes each, so payload space runs out well
/// before slots do. A caller that knows its records are shaped differently
/// can build a [`ChannelSize`] itself.
#[must_use]
pub const fn shm_for_capacity(capacity: usize) -> ChannelSize {
    ChannelSize { capacity, slots: capacity / 64 }
}

#[derive(derive_more::Debug)]
pub struct Command {
    program: OsString,
    /// Shared memory for this run's file-access records.
    #[cfg_attr(
        target_env = "musl",
        expect(dead_code, reason = "musl builds track through seccomp, with no channel to size")
    )]
    pub(crate) shm: ChannelSize,
    args: Vec<OsString>,
    envs: FxHashMap<OsString, OsString>,
    cwd: Option<PathBuf>,
    #[cfg(unix)]
    arg0: Option<OsString>,

    stderr: Option<Stdio>,
    stdout: Option<Stdio>,
    stdin: Option<Stdio>,

    #[cfg(unix)]
    #[debug("({} pre_exec closures)", pre_exec_closures.len())]
    pre_exec_closures: Vec<Box<dyn FnMut() -> std::io::Result<()> + Send + Sync>>,
}

impl Command {
    /// Create a new command to spy on the given program.
    /// Initially, environment variables are not inherited from the parent.
    /// To inherit, explicitly use `.envs(std::env::vars_os())`.
    pub fn new<P: AsRef<OsStr>>(program: P) -> Self {
        Self {
            program: program.as_ref().to_os_string(),
            shm: shm_for_capacity(DEFAULT_SHM_CAPACITY),
            args: Vec::new(),
            envs: FxHashMap::default(),
            cwd: None,
            #[cfg(unix)]
            arg0: None,
            stderr: None,
            stdout: None,
            stdin: None,
            #[cfg(unix)]
            pre_exec_closures: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[must_use]
    pub(crate) fn get_exec(&self) -> Exec {
        use std::{
            iter::once,
            os::unix::ffi::{OsStrExt, OsStringExt},
        };

        use bstr::{BString, ByteSlice as _};
        let arg0 =
            BString::from(self.arg0.clone().unwrap_or_else(|| self.program.clone()).into_vec());
        Exec {
            program: self.program.as_bytes().into(),
            args: once(arg0)
                .chain(self.args.iter().map(|arg| arg.as_bytes().as_bstr().to_owned()))
                .collect(),
            envs: self
                .envs
                .iter()
                .map(|(name, value)| (name.as_bytes().into(), Some(value.as_bytes().into())))
                .collect(),
        }
    }

    #[cfg(unix)]
    pub(crate) fn set_exec(&mut self, mut exec: Exec) {
        use std::os::unix::ffi::OsStringExt;

        self.program = OsString::from_vec(exec.program.into());
        self.arg0 = Some(OsString::from_vec(exec.args.remove(0).into()));
        self.args = exec.args.into_iter().map(|arg| OsString::from_vec(arg.into())).collect();
        self.envs = exec
            .envs
            .into_iter()
            .map(|(name, value)| {
                (
                    OsString::from_vec(name.into()),
                    OsString::from_vec(value.unwrap_or_default().into()),
                )
            })
            .collect();
    }

    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.envs.remove(key.as_ref());
        self
    }

    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.stderr = Some(cfg.into());
        self
    }

    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.stdout = Some(cfg.into());
        self
    }

    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.stdin = Some(cfg.into());
        self
    }

    /// Sizes the shared memory this run's file-access records go through.
    ///
    /// How many accesses a program makes is the caller's business rather
    /// than this crate's, so a caller that knows better than
    /// [`DEFAULT_SHM_CAPACITY`] says so here, usually through
    /// [`shm_for_capacity`].
    pub const fn shm(&mut self, size: ChannelSize) -> &mut Self {
        self.shm = size;
        self
    }

    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.envs.insert(key.as_ref().to_os_string(), val.as_ref().to_os_string());
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.envs.extend(
            vars.into_iter()
                .map(|(key, val)| (key.as_ref().to_os_string(), val.as_ref().to_os_string())),
        );
        self
    }

    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
        self.cwd = Some(dir.as_ref().to_owned());
        self
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(args.into_iter().map(|arg| arg.as_ref().to_os_string()));
        self
    }

    #[cfg(unix)]
    pub fn arg0<S>(&mut self, arg: S) -> &mut Self
    where
        S: AsRef<OsStr>,
    {
        self.arg0 = Some(arg.as_ref().to_os_string());
        self
    }

    /// Spawn the command with file system access tracking.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError`] if program resolution fails or the process cannot be spawned.
    pub async fn spawn(
        mut self,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        self.resolve_program()?;
        SPY_IMPL.spawn(self, cancellation_token).await
    }

    /// Resolve program name to full path using `PATH` and cwd.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::Which`] if the program cannot be found in `PATH`.
    ///
    /// # Panics
    ///
    /// Panics if no `cwd` is set and `std::env::current_dir()` fails.
    pub fn resolve_program(&mut self) -> Result<(), SpawnError> {
        let mut path_env: Option<&OsStr> = None;
        for (env_name, env_value) in &self.envs {
            let Some(env_name) = env_name.to_str() else {
                continue;
            };
            if env_name.eq_ignore_ascii_case("path") {
                path_env = Some(env_value.as_ref());
                break;
            }
        }

        let cwd = self
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("failed to get current dir"));
        self.program = which::which_in(self.program.as_os_str(), path_env, &cwd)
            .map_err(|err| SpawnError::Which {
                program: self.program.clone(),
                path: path_env.map(OsStr::to_owned),
                cwd,
                cause: err,
            })?
            .into_os_string();
        Ok(())
    }

    /// Schedules a closure to be run just before the exec function is invoked.
    ///
    /// # Safety
    ///
    /// <https://doc.rust-lang.org/1.91.1/std/os/unix/process/trait.CommandExt.html#tymethod.pre_exec>
    #[cfg(unix)]
    pub unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Self
    where
        F: FnMut() -> std::io::Result<()> + Send + Sync + 'static,
    {
        self.pre_exec_closures.push(Box::new(f));
        self
    }

    /// Convert to a `tokio::process::Command` without tracking.
    #[must_use]
    pub(crate) fn into_tokio_command(self) -> TokioCommand {
        let mut tokio_cmd = TokioCommand::new(self.program);
        if let Some(cwd) = &self.cwd {
            tokio_cmd.current_dir(cwd);
        }

        #[cfg(unix)]
        if let Some(arg0) = self.arg0 {
            tokio_cmd.arg0(arg0);
        }
        tokio_cmd.args(self.args);
        tokio_cmd.env_clear();
        tokio_cmd.envs(self.envs);

        if let Some(stdin) = self.stdin {
            tokio_cmd.stdin(stdin);
        }

        if let Some(stdout) = self.stdout {
            tokio_cmd.stdout(stdout);
        }

        if let Some(stderr) = self.stderr {
            tokio_cmd.stderr(stderr);
        }

        #[cfg(unix)]
        for pre_exec in self.pre_exec_closures {
            // Safety: The caller of `pre_exec` is responsible for ensuring safety.
            unsafe { tokio_cmd.pre_exec(pre_exec) };
        }

        tokio_cmd
    }
}
