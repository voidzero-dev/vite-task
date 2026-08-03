pub mod convert;
pub mod raw_exec;

use std::{
    cell::{Cell, RefCell},
    ffi::OsStr,
    fmt::Debug,
    os::unix::ffi::OsStrExt as _,
    path::Path,
    sync::OnceLock,
};

use convert::{ToAbsolutePath, ToAccessMode};
use fspy_shared::ipc::{PathAccess, PathAccessSender};
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::EncodedPayload,
    spawn::{PreExec, handle_exec},
};
use raw_exec::RawExec;

pub struct Client {
    encoded_payload: EncodedPayload,
    process_id: u32,
}

// SAFETY: Client fields are only mutated during initialization in the ctor; after that, all access is read-only
#[cfg(target_os = "macos")]
unsafe impl Sync for Client {}
// SAFETY: Client is only sent once during initialization; after that it lives in a static OnceLock
#[cfg(target_os = "macos")]
unsafe impl Send for Client {}

impl Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish()
    }
}

impl Client {
    #[cfg(not(test))]
    fn from_env() -> Self {
        use fspy_shared_unix::payload::decode_payload_from_env;

        let encoded_payload = decode_payload_from_env().unwrap();
        Self { encoded_payload, process_id: std::process::id() }
    }

    fn send(&self, mode: fspy_shared::ipc::AccessMode, path: &Path) {
        let path_bytes = path.as_os_str().as_bytes();
        if path_bytes.starts_with(b"/dev/")
            || (cfg!(target_os = "linux")
                && (path_bytes.starts_with(b"/proc/") || path_bytes.starts_with(b"/sys/")))
        {
            return;
        }
        let path_access = PathAccess { mode, path: path.into() };
        let process_id = std::process::id();

        // A spawn implementation can repurpose inherited descriptors before
        // calling exec. Dropping or reconnecting the inherited pipe client in
        // that window could close one of the repurposed descriptors. The
        // exec'd process gets fresh TLS and establishes its own client.
        if PRE_EXEC_CHILD.get()
            || POSIX_SPAWN_PARENT_PID
                .get()
                .is_some_and(|parent_process_id| parent_process_id != process_id)
        {
            return;
        }

        THREAD_CLIENT.with(|thread_client| {
            // Connecting the pipe opens files and can re-enter an interposed
            // function. The nested report is transport noise, so skip it.
            let Ok(mut thread_client) = thread_client.try_borrow_mut() else {
                return;
            };

            if matches!(&*thread_client, ThreadClient::Connected { process_id: owner, .. } if *owner != process_id)
            {
                // A fork inherits the calling thread's descriptor and TLS.
                // Give the child process its own connection before it writes.
                *thread_client = ThreadClient::Uninitialized;
            }

            if matches!(&*thread_client, ThreadClient::Uninitialized) {
                let server_name = self.encoded_payload.payload.server_name.to_cow_os_str();
                *thread_client = match PathAccessSender::connect(&server_name) {
                    Ok(sender) => ThreadClient::Connected { process_id, sender },
                    Err(error) => {
                        report_connection_error(&error);
                        ThreadClient::Disconnected
                    }
                };
            }

            let error = match &mut *thread_client {
                ThreadClient::Connected { sender, .. } => sender.send(path_access).err(),
                ThreadClient::Uninitialized | ThreadClient::Disconnected => None,
            };
            if let Some(error) = error {
                report_connection_error(&error);
                *thread_client = ThreadClient::Disconnected;
            }
        });
    }

    pub unsafe fn handle_exec<R>(
        &self,
        config: ExecResolveConfig,
        raw_exec: RawExec,
        f: impl FnOnce(RawExec, Option<PreExec>) -> nix::Result<R>,
    ) -> nix::Result<R> {
        // fork-based spawn implementations call exec after setting up the
        // child's descriptors. Suppress reports in that transient child so
        // the inherited TLS client is neither dropped nor reconnected before
        // exec replaces it.
        let _pre_exec_child = (self.process_id != std::process::id()).then(enter_pre_exec_child);

        // SAFETY: raw_exec contains valid pointers to C strings and null-terminated arrays, as provided by the caller
        let mut exec = unsafe { raw_exec.to_exec() };
        let pre_exec = handle_exec(&mut exec, config, &self.encoded_payload, |mode, path| {
            self.send(mode, path);
        })?;
        RawExec::from_exec(exec, |raw_command| f(raw_command, pre_exec))
    }

    pub unsafe fn try_handle_open(
        &self,
        path: impl ToAbsolutePath,
        mode: impl ToAccessMode,
    ) -> anyhow::Result<()> {
        // SAFETY: mode contains a valid pointer (if ModeStr) or a plain value, as provided by the caller
        let mode = unsafe { mode.to_access_mode() };
        // SAFETY: path contains valid pointers to C strings/file descriptors, as provided by the caller
        let () = unsafe {
            path.to_absolute_path(|abs_path| {
                let Some(abs_path) = abs_path else {
                    return Ok(Ok::<(), anyhow::Error>(()));
                };
                self.send(mode, Path::new(OsStr::from_bytes(abs_path)));
                Ok(Ok::<(), anyhow::Error>(()))
            })
        }??;

        Ok(())
    }
}

static CLIENT: OnceLock<Client> = OnceLock::new();

enum ThreadClient {
    Uninitialized,
    Connected { process_id: u32, sender: PathAccessSender },
    Disconnected,
}

thread_local! {
    static THREAD_CLIENT: RefCell<ThreadClient> = const { RefCell::new(ThreadClient::Uninitialized) };
    static POSIX_SPAWN_PARENT_PID: Cell<Option<u32>> = const { Cell::new(None) };
    static PRE_EXEC_CHILD: Cell<bool> = const { Cell::new(false) };
}

struct PreExecChildGuard(bool);

impl Drop for PreExecChildGuard {
    fn drop(&mut self) {
        PRE_EXEC_CHILD.set(self.0);
    }
}

fn enter_pre_exec_child() -> PreExecChildGuard {
    PreExecChildGuard(PRE_EXEC_CHILD.replace(true))
}

pub struct PosixSpawnGuard(Option<u32>);

impl Drop for PosixSpawnGuard {
    fn drop(&mut self) {
        POSIX_SPAWN_PARENT_PID.set(self.0);
    }
}

pub fn enter_posix_spawn() -> PosixSpawnGuard {
    PosixSpawnGuard(POSIX_SPAWN_PARENT_PID.replace(Some(std::process::id())))
}

#[expect(
    clippy::print_stderr,
    reason = "preload library intentionally uses stderr for error reporting"
)]
fn report_connection_error(error: &std::io::Error) {
    eprintln!("fspy: path access connection failed: {error}");
}

// Resolving and reporting a file access can call another interposed function.
// Suppress same-thread re-entry to prevent recursive access handling while
// still recording accesses from other threads.
thread_local! {
    static HANDLING_OPEN: Cell<bool> = const { Cell::new(false) };
}

struct ResetHandling<'a>(&'a Cell<bool>);
impl Drop for ResetHandling<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub fn global_client() -> Option<&'static Client> {
    CLIENT.get()
}

pub unsafe fn handle_open(path: impl ToAbsolutePath, mode: impl ToAccessMode) {
    HANDLING_OPEN.with(|handling| {
        if handling.replace(true) {
            return;
        }

        let _reset = ResetHandling(handling);

        if let Some(client) = global_client() {
            // SAFETY: path and mode contain valid pointers/values forwarded from the interposed function's caller
            unsafe { client.try_handle_open(path, mode) }.unwrap();
        }
    });
}

#[cfg(not(test))]
#[ctor::ctor(unsafe)]
fn init_client() {
    CLIENT.set(Client::from_env()).unwrap();
}
