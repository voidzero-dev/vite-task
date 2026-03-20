//! musl-specific child process spawning using `posix_spawn()`.
//!
//! On musl libc, `fork()` in a multi-threaded process is unsafe: musl internal
//! state (locks, allocator metadata) in other threads can be left inconsistent
//! in the child, causing SIGSEGV/SIGBUS. Rust's `std::process::Command` falls
//! back to `fork()+exec()` whenever a `pre_exec` closure is set, and
//! `portable_pty` always sets one for PTY setup (setsid, TIOCSCTTY, close FDs,
//! signal reset).
//!
//! musl's `posix_spawn()` implementation uses `clone(CLONE_VM | CLONE_VFORK)`
//! instead of `fork()`, which avoids duplicating parent state entirely. This
//! module reimplements the child spawn using `posix_spawn()` directly, handling
//! all PTY setup via spawn attributes and file actions:
//!
//! - `POSIX_SPAWN_SETSID`: makes the child a session leader (replaces `setsid()`)
//! - `posix_spawn_file_actions_addopen()`: opens the slave TTY as fd 0, which
//!   also sets it as the controlling terminal (because the child is a session
//!   leader without a controlling terminal)
//! - `posix_spawn_file_actions_adddup2()`: copies fd 0 to fd 1 and fd 2
//! - `POSIX_SPAWN_SETSIGDEF` + `POSIX_SPAWN_SETSIGMASK`: resets signal
//!   dispositions and unblocks all signals

use std::{
    ffi::{CString, OsStr},
    io, mem,
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr,
};

use portable_pty::{ChildKiller, CommandBuilder, ExitStatus};

/// A child process spawned via `posix_spawn()`.
pub struct PosixSpawnChild {
    pid: libc::pid_t,
}

/// A cloneable handle for killing a `posix_spawn`'d child.
#[derive(Debug)]
struct PosixSpawnChildKiller {
    pid: libc::pid_t,
}

impl ChildKiller for PosixSpawnChildKiller {
    fn kill(&mut self) -> io::Result<()> {
        // SAFETY: Sending SIGHUP to a valid PID. If the process already exited,
        // the kernel returns ESRCH which we surface as an error.
        let result = unsafe { libc::kill(self.pid, libc::SIGHUP) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Self { pid: self.pid })
    }
}

impl PosixSpawnChild {
    pub fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(PosixSpawnChildKiller { pid: self.pid })
    }

    /// Blocks until the child exits and returns its exit status.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let mut status: libc::c_int = 0;
        loop {
            // SAFETY: Calling waitpid with a valid PID and status pointer.
            let result = unsafe { libc::waitpid(self.pid, &raw mut status, 0) };
            if result == -1 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            break;
        }

        if libc::WIFEXITED(status) {
            let code = libc::WEXITSTATUS(status);
            Ok(ExitStatus::with_exit_code(code.cast_unsigned()))
        } else if libc::WIFSIGNALED(status) {
            let sig = libc::WTERMSIG(status);
            // SAFETY: strsignal returns a valid C string for known signals or NULL.
            let signame = unsafe {
                let s = libc::strsignal(sig);
                if s.is_null() {
                    format!("Signal {sig}")
                } else {
                    std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned()
                }
            };
            Ok(ExitStatus::with_signal(&signame))
        } else {
            Ok(ExitStatus::with_exit_code(1))
        }
    }
}

/// Spawns a child process into the PTY slave using `posix_spawn()`.
///
/// # Errors
///
/// Returns an error if `posix_spawn()` or any setup step fails.
pub fn spawn_child_posix(
    slave_tty_path: &Path,
    cmd: &CommandBuilder,
) -> anyhow::Result<PosixSpawnChild> {
    let argv = cmd.get_argv();
    anyhow::ensure!(!argv.is_empty(), "empty argv");

    let program = to_cstring(&argv[0])?;
    let arg_cstrings: Vec<CString> =
        argv.iter().map(|a| to_cstring(a)).collect::<Result<_, _>>()?;
    let mut c_argv: Vec<*mut libc::c_char> =
        arg_cstrings.iter().map(|a| a.as_ptr().cast_mut()).collect();
    c_argv.push(ptr::null_mut());

    let env_strings: Vec<CString> = cmd
        .iter_full_env_as_str()
        .map(|(k, v)| CString::new(format!("{k}={v}")))
        .collect::<Result<_, _>>()?;
    let mut c_envp: Vec<*mut libc::c_char> =
        env_strings.iter().map(|e| e.as_ptr().cast_mut()).collect();
    c_envp.push(ptr::null_mut());

    let slave_path = to_cstring(slave_tty_path.as_os_str())?;
    let cwd_cstring = cmd.get_cwd().map(|cwd| to_cstring(cwd)).transpose()?;

    // SAFETY: All `posix_spawn_*` calls below operate on stack-allocated,
    // properly initialized structures with valid pointers. The `CString` vectors
    // (`arg_cstrings`, `env_strings`, `slave_path`, `cwd_cstring`) are kept alive
    // for the duration of this block, ensuring all raw pointers remain valid.
    // `mem::zeroed()` produces valid initial state for `posix_spawn_file_actions_t`
    // and `posix_spawnattr_t` before their respective `_init()` calls.
    // `sigfillset`/`sigemptyset` operate on zeroed `sigset_t` which is valid.
    // `posix_spawnp` is called with properly null-terminated argv/envp arrays.
    #[expect(clippy::cast_possible_truncation, reason = "spawn flags fit in c_short")]
    unsafe {
        let mut file_actions: libc::posix_spawn_file_actions_t = mem::zeroed();
        let mut file_actions_initialized = false;
        let mut attr: libc::posix_spawnattr_t = mem::zeroed();
        let mut attr_initialized = false;

        let result = (|| -> anyhow::Result<PosixSpawnChild> {
            check_posix(libc::posix_spawn_file_actions_init(&raw mut file_actions))?;
            file_actions_initialized = true;

            // Open slave TTY as fd 0 (stdin). Because the child is a session
            // leader (POSIX_SPAWN_SETSID) with no controlling terminal, this
            // open() automatically sets the slave TTY as the controlling terminal.
            check_posix(libc::posix_spawn_file_actions_addopen(
                &raw mut file_actions,
                0,
                slave_path.as_ptr(),
                libc::O_RDWR,
                0,
            ))?;

            // dup2 stdin (now the slave TTY) to stdout and stderr.
            check_posix(libc::posix_spawn_file_actions_adddup2(&raw mut file_actions, 0, 1))?;
            check_posix(libc::posix_spawn_file_actions_adddup2(&raw mut file_actions, 0, 2))?;

            if let Some(ref cwd) = cwd_cstring {
                check_posix(libc::posix_spawn_file_actions_addchdir_np(
                    &raw mut file_actions,
                    cwd.as_ptr(),
                ))?;
            }

            check_posix(libc::posix_spawnattr_init(&raw mut attr))?;
            attr_initialized = true;

            // Create new session + reset signal dispositions + clear signal mask.
            let flags: libc::c_short = (libc::POSIX_SPAWN_SETSID
                | libc::POSIX_SPAWN_SETSIGDEF
                | libc::POSIX_SPAWN_SETSIGMASK)
                as libc::c_short;
            check_posix(libc::posix_spawnattr_setflags(&raw mut attr, flags))?;

            // Reset ALL signal dispositions to default.
            let mut all_signals: libc::sigset_t = mem::zeroed();
            libc::sigfillset(&raw mut all_signals);
            check_posix(libc::posix_spawnattr_setsigdefault(
                &raw mut attr,
                &raw const all_signals,
            ))?;

            // Unblock all signals.
            let mut empty_mask: libc::sigset_t = mem::zeroed();
            libc::sigemptyset(&raw mut empty_mask);
            check_posix(libc::posix_spawnattr_setsigmask(&raw mut attr, &raw const empty_mask))?;

            let mut pid: libc::pid_t = 0;
            check_posix(libc::posix_spawnp(
                &raw mut pid,
                program.as_ptr(),
                &raw const file_actions,
                &raw const attr,
                c_argv.as_ptr(),
                c_envp.as_ptr(),
            ))?;

            Ok(PosixSpawnChild { pid })
        })();

        if file_actions_initialized {
            libc::posix_spawn_file_actions_destroy(&raw mut file_actions);
        }
        if attr_initialized {
            libc::posix_spawnattr_destroy(&raw mut attr);
        }

        result
    }
}

fn to_cstring(s: &OsStr) -> anyhow::Result<CString> {
    CString::new(s.as_bytes()).map_err(|e| anyhow::anyhow!("nul byte in argument: {e}"))
}

fn check_posix(ret: libc::c_int) -> anyhow::Result<()> {
    if ret != 0 { Err(io::Error::from_raw_os_error(ret).into()) } else { Ok(()) }
}
