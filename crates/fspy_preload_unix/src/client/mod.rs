pub mod convert;
pub mod raw_exec;

mod fork_tracker;
mod process_sender;

use std::{
    cell::Cell, ffi::OsStr, fmt::Debug, num::NonZeroUsize, os::unix::ffi::OsStrExt as _,
    path::Path, sync::OnceLock,
};

use convert::{ToAbsolutePath, ToAccessMode};
use fspy_shared::ipc::PathAccess;
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::EncodedPayload,
    spawn::{PreExec, handle_exec},
};
use process_sender::ProcessSender;
use raw_exec::RawExec;
use wincode::Serialize as _;

pub struct Client {
    encoded_payload: EncodedPayload,
    ipc_sender: ProcessSender,
}

// SAFETY: ProcessSender publishes immutable sender states through atomics, and
// the remaining Client state is read-only after initialization.
#[cfg(target_os = "macos")]
unsafe impl Sync for Client {}
// SAFETY: Client is moved into the static OnceLock during initialization. Its
// ProcessSender and read-only payload may then be shared between threads.
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
        let ipc_sender = ProcessSender::new(&encoded_payload.payload.ipc_channel_conf).unwrap();

        Self { encoded_payload, ipc_sender }
    }

    fn send(&self, mode: fspy_shared::ipc::AccessMode, path: &Path) -> anyhow::Result<()> {
        let Some(ipc_sender) = self.ipc_sender.sender() else {
            // ipc channel not available, skip sending
            return Ok(());
        };
        let path_bytes = path.as_os_str().as_bytes();
        if path_bytes.starts_with(b"/dev/")
            || (cfg!(target_os = "linux")
                && (path_bytes.starts_with(b"/proc/") || path_bytes.starts_with(b"/sys/")))
        {
            return Ok(());
        }
        let path_access = PathAccess { mode, path: path.into() };
        let serialized_size = usize::try_from(PathAccess::serialized_size(&path_access)?)
            .expect("serialized size exceeds usize");

        let frame_size = NonZeroUsize::new(serialized_size)
            .expect("fspy: encoded PathAccess should never be empty");

        let mut frame = ipc_sender
            .claim_frame(frame_size)
            .expect("fspy: failed to claim frame in shared memory");
        let mut writer: &mut [u8] = &mut frame;
        PathAccess::serialize_into(&mut writer, &path_access)?;
        assert_eq!(writer.len(), 0);

        Ok(())
    }

    pub unsafe fn handle_exec<R>(
        &self,
        config: ExecResolveConfig,
        raw_exec: RawExec,
        f: impl FnOnce(RawExec, Option<PreExec>) -> nix::Result<R>,
    ) -> nix::Result<R> {
        // SAFETY: raw_exec contains valid pointers to C strings and null-terminated arrays, as provided by the caller
        let mut exec = unsafe { raw_exec.to_exec() };
        let pre_exec = handle_exec(&mut exec, config, &self.encoded_payload, |mode, path| {
            self.send(mode, path).unwrap();
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
                    return Ok(Ok(()));
                };
                Ok(self.send(mode, Path::new(OsStr::from_bytes(abs_path))))
            })
        }??;

        Ok(())
    }
}

static CLIENT: OnceLock<Client> = OnceLock::new();

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
