//! Forwarding of termination signals to running task processes.
//!
//! A signal sent to the runner alone (`kill <pid>`, a supervisor, an editor's
//! stop button) does not reach the task processes that runner started.
//! Without forwarding, the runner exits and the tasks are orphaned, holding
//! ports and files behind it. The terminal is the only sender that reaches
//! tasks on its own, because Ctrl-C goes to the whole foreground process
//! group; this module makes directed signals behave the same way by
//! forwarding SIGINT, SIGTERM and SIGHUP to every registered task process.
//!
//! The handler also cancels the run's tokens, exactly like an in-terminal
//! Ctrl-C: no new tasks are scheduled and in-flight tasks aren't cached.
//! The run then ends through the regular wait paths, once the tasks exit.
//!
//! Forwarding happens on a dedicated thread fed by a self-pipe
//! ([`signal_hook`]), never inside the signal handler itself, which only
//! writes to the pipe (the one async-signal-safe operation available).
//!
//! Task processes stay in the runner's process group, so on a terminal they
//! receive Ctrl-C from the terminal driver *and* through this forwarding. The
//! duplicate is harmless because handling a termination signal is idempotent,
//! and keeping tasks in the foreground group is what lets interactive tasks
//! read from the terminal at all (a separate process group would stop them
//! with SIGTTIN).
//!
//! On Windows there are no Unix-style signals. Ctrl-C console events reach
//! every process attached to the console, and cancelling the run's tokens
//! terminates the task trees through their Job Objects, so the handler only
//! cancels the tokens there.

use std::io;

use tokio_util::sync::CancellationToken;

/// Registers a spawned task process so that termination signals received by
/// this process are forwarded to it.
///
/// Callers keep the registration for as long as the process runs and drop it
/// once the process has exited, so signals are never forwarded to a process
/// id that has been reaped and possibly reused.
pub struct ChildRegistration {
    #[cfg(unix)]
    pid: Option<u32>,
}

#[cfg(unix)]
impl Drop for ChildRegistration {
    fn drop(&mut self) {
        let Some(pid) = self.pid else {
            return;
        };
        CHILDREN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&pid);
    }
}

/// Registers `pid` for signal forwarding. `None` (the process id is unknown)
/// registers nothing.
pub fn register_child(pid: Option<u32>) -> ChildRegistration {
    #[cfg(unix)]
    if let Some(pid) = pid {
        CHILDREN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(pid);
    }
    #[cfg(not(unix))]
    let _ = pid;
    ChildRegistration {
        #[cfg(unix)]
        pid,
    }
}

/// Installs the signal handler and points it at the current run's tokens.
///
/// The handler thread is started once per process; later calls only update the
/// tokens. Every call registers the handler for its tokens before returning,
/// so the caller can rely on signals being handled from here on.
#[cfg(unix)]
pub fn install(cancel: CancellationToken, interrupt: CancellationToken) -> io::Result<()> {
    use std::{
        sync::{OnceLock, PoisonError},
        thread,
    };

    use signal_hook::{
        consts::{SIGHUP, SIGINT, SIGTERM},
        iterator::Signals,
    };

    static STARTED: OnceLock<()> = OnceLock::new();

    *TARGETS.lock().unwrap_or_else(PoisonError::into_inner) = Some(Targets { cancel, interrupt });

    if STARTED.get().is_some() {
        return Ok(());
    }
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])?;
    let _ = STARTED.set(());
    thread::Builder::new().name("vt-signal-forward".to_string()).spawn(move || {
        for signal in signals.forever() {
            handle(signal);
        }
    })?;
    Ok(())
}

/// Cancels the current run's tokens and forwards `signal` to every registered
/// task process.
#[cfg(unix)]
fn handle(signal: i32) {
    use std::sync::PoisonError;

    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    if let Some(targets) = TARGETS.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
        targets.cancel.cancel();
        targets.interrupt.cancel();
    }

    let Ok(signal) = Signal::try_from(signal) else {
        return;
    };
    let children: Vec<u32> =
        CHILDREN.lock().unwrap_or_else(PoisonError::into_inner).iter().copied().collect();
    for pid in children {
        let _ = kill(Pid::from_raw(pid.cast_signed()), signal);
    }
}

/// On Windows, console Ctrl-C events already reach every process attached to
/// the console, and the Job Object assigned to each task kills its whole
/// process tree when the run's tokens are cancelled. The handler therefore
/// only cancels the tokens.
#[cfg(windows)]
pub(crate) fn install(cancel: CancellationToken, interrupt: CancellationToken) -> io::Result<()> {
    // An ancestor process (e.g. cargo) may have been created with
    // CREATE_NEW_PROCESS_GROUP, which sets a per-process flag that silently
    // drops CTRL_C_EVENT before it reaches registered handlers. Clear it so
    // our handler fires.
    //
    // SAFETY: Passing (None, FALSE) clears the inherited CTRL_C ignore flag.
    unsafe {
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        SetConsoleCtrlHandler(None, 0);
    }
    ctrlc::set_handler(move || {
        cancel.cancel();
        interrupt.cancel();
    })
    .map_err(io::Error::other)
}

/// Task processes to forward termination signals to.
#[cfg(unix)]
static CHILDREN: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// The tokens of the run in progress.
#[cfg(unix)]
static TARGETS: std::sync::Mutex<Option<Targets>> = std::sync::Mutex::new(None);

/// The tokens a forwarded signal cancels, mirroring the in-terminal Ctrl-C
/// handler this module replaced.
#[cfg(unix)]
struct Targets {
    cancel: CancellationToken,
    interrupt: CancellationToken,
}
