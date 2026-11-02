use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

#[cfg(unix)]
use allocator_api2::alloc::Global;
#[cfg(unix)]
use fspy_shared_unix::exec::{Exec, to_vec_in};
use rustc_hash::FxHashMap;
use tokio::process::Command as TokioCommand;
use tokio_util::sync::CancellationToken;
use vt_casefold::EnvName;

use crate::{SPY_IMPL, TrackedChild, error::SpawnError};

#[derive(derive_more::Debug)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    envs: FxHashMap<EnvName<OsString>, OsString>,
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
    pub(crate) fn get_exec(&self) -> Exec<Global> {
        use std::{iter::once, os::unix::ffi::OsStrExt};

        let arg0 = self.arg0.as_ref().unwrap_or(&self.program);
        Exec {
            program: to_vec_in(self.program.as_bytes(), Global),
            args: once(arg0)
                .chain(&self.args)
                .map(|arg| to_vec_in(arg.as_bytes(), Global))
                .collect(),
            envs: self
                .envs
                .iter()
                .map(|(name, value)| {
                    (
                        to_vec_in(name.inner().as_bytes(), Global),
                        Some(to_vec_in(value.as_bytes(), Global)),
                    )
                })
                .collect(),
        }
    }

    #[cfg(unix)]
    pub(crate) fn set_exec(&mut self, mut exec: Exec<Global>) {
        use std::os::unix::ffi::OsStrExt;

        let to_os_string = |bytes: &[u8]| OsStr::from_bytes(bytes).to_os_string();
        self.program = to_os_string(&exec.program);
        self.arg0 = Some(to_os_string(&exec.args.remove(0)));
        self.args = exec.args.iter().map(|arg| to_os_string(arg)).collect();
        self.envs = exec
            .envs
            .iter()
            .map(|(name, value)| {
                (
                    EnvName::new(to_os_string(name)),
                    to_os_string(value.as_deref().unwrap_or_default()),
                )
            })
            .collect();
    }

    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.envs.remove(EnvName::from_ref(key.as_ref()));
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

    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.envs.insert(EnvName::new(key.as_ref().to_os_string()), val.as_ref().to_os_string());
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.envs.extend(vars.into_iter().map(|(key, val)| {
            (EnvName::new(key.as_ref().to_os_string()), val.as_ref().to_os_string())
        }));
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
    /// Returns [`SpawnError`] if program resolution fails, the tracking
    /// machinery cannot be initialized (e.g. the preload library cannot be
    /// materialized), or the process cannot be spawned.
    pub async fn spawn(
        mut self,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        self.resolve_program()?;
        match &*SPY_IMPL {
            Ok(spy) => spy.spawn(self, cancellation_token).await,
            Err(e) => Err(SpawnError::Injection(io::Error::new(e.kind(), e.to_string()))),
        }
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
        let path_env =
            self.envs.get(EnvName::from_ref(OsStr::new("PATH"))).map(OsString::as_os_str);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_follow_platform_rules() {
        let mut command = Command::new("program");
        command.env("Path", "first").env("PATH", "second");

        let mut envs: Vec<_> =
            command.envs.iter().map(|(name, value)| (&**name.inner(), &**value)).collect();
        envs.sort_unstable();
        if cfg!(windows) {
            assert_eq!(envs, [(OsStr::new("Path"), OsStr::new("second"))]);
        } else {
            assert_eq!(
                envs,
                [
                    (OsStr::new("PATH"), OsStr::new("second")),
                    (OsStr::new("Path"), OsStr::new("first"))
                ]
            );
        }

        command.env_remove("path");
        assert_eq!(command.envs.len(), if cfg!(windows) { 0 } else { 2 });
    }
}
