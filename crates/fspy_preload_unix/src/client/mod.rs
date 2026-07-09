pub mod convert;
pub mod raw_exec;

#[cfg(target_os = "linux")]
use std::ffi::CString;
use std::{
    ffi::OsStr, fmt::Debug, num::NonZeroUsize, os::unix::ffi::OsStrExt as _, path::Path,
    sync::OnceLock,
};

use convert::{ToAbsolutePath, ToAccessMode};
use fspy_shared::ipc::{
    PathAccess,
    channel::{ClaimFrameError, Sender, SenderFrame},
};
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::EncodedPayload,
    spawn::{PreExec, handle_exec},
};
use raw_exec::RawExec;
use wincode::Serialize as _;

#[cfg(target_os = "linux")]
pub const ENCODED_PATH_CAPACITY: usize = libc::PATH_MAX as usize + 32;

pub struct Client {
    encoded_payload: EncodedPayload,
    ipc_sender: Option<Sender>,
    #[cfg(target_os = "linux")]
    lock_file_path: Option<CString>,
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
    #[expect(
        clippy::print_stderr,
        reason = "preload library intentionally uses stderr for error reporting"
    )]
    #[cfg(not(test))]
    fn from_env() -> Self {
        use fspy_shared_unix::payload::decode_payload_from_env;

        let encoded_payload = decode_payload_from_env().unwrap();

        let ipc_sender = match encoded_payload.payload.ipc_channel_conf.sender() {
            Ok(sender) => Some(sender),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::NotFound
                ) =>
            {
                None
            }
            Err(err) => {
                // this can happen if the process is started after the root target process has exited.
                // By that time the channel would have been closed in the receiver side.
                // In this case we just leave a message and skip sending any path accesses.
                eprintln!("fspy: failed to create ipc sender: {err}");
                None
            }
        };

        #[cfg(target_os = "linux")]
        let lock_file_path = ipc_sender.as_ref().map(|sender| {
            CString::new(sender.lock_file_path().as_bytes()).expect("lock path contains NUL")
        });

        Self {
            encoded_payload,
            ipc_sender,
            #[cfg(target_os = "linux")]
            lock_file_path,
        }
    }

    fn claim_frame<'a>(
        &self,
        sender: &'a Sender,
        frame_size: NonZeroUsize,
    ) -> Result<SenderFrame<'a>, ClaimFrameError> {
        let _ = self;
        #[cfg(target_os = "linux")]
        if let Some(path) = &self.lock_file_path {
            // SAFETY: `path` is the channel's exact internal lock-file path.
            match unsafe { crate::accel::open_control_file(path) } {
                Ok(Some(lock_file)) => {
                    // SAFETY: the helper opened this sender's exact lock path
                    // and transfers sole ownership of the handle.
                    return unsafe { sender.claim_frame_with_lock(frame_size, lock_file) };
                }
                Ok(None) => {}
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        || error.raw_os_error() == Some(libc::ENOSYS) =>
                {
                    return Err(ClaimFrameError::Closed(error));
                }
                Err(error) => return Err(ClaimFrameError::Lock(error)),
            }
        }
        sender.claim_frame(frame_size)
    }

    fn send(&self, mode: fspy_shared::ipc::AccessMode, path: &Path) -> anyhow::Result<()> {
        let Some(ipc_sender) = &self.ipc_sender else {
            // ipc channel not available, skip sending
            return Ok(());
        };
        if ipc_sender.is_lock_file(path) {
            return Ok(());
        }
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

        let mut frame = match self.claim_frame(ipc_sender, frame_size) {
            Ok(frame) => frame,
            Err(ClaimFrameError::Closed(_)) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut writer: &mut [u8] = &mut frame;
        PathAccess::serialize_into(&mut writer, &path_access)?;
        assert_eq!(writer.len(), 0);

        Ok(())
    }

    /// Sends an already-resolved Unix path without allocating or panicking.
    ///
    /// The preload syscall dispatcher uses this only after copying the caller's
    /// path through `process_vm_readv`. A false result keeps the syscall on the
    /// seccomp path instead of risking an unreported allowlisted syscall.
    #[cfg(target_os = "linux")]
    #[cfg_attr(
        test,
        expect(
            dead_code,
            reason = "the constructor that registers the dispatcher is disabled in tests"
        )
    )]
    pub(crate) fn try_send_bytes(
        &self,
        mode: fspy_shared::ipc::AccessMode,
        path_bytes: &[u8],
        encoded: &mut [u8; ENCODED_PATH_CAPACITY],
    ) -> bool {
        let Some(ipc_sender) = &self.ipc_sender else {
            return false;
        };
        if accelerated_path_must_fallback(path_bytes) {
            return false;
        }

        let path = Path::new(OsStr::from_bytes(path_bytes));
        if ipc_sender.is_lock_file(path) {
            return false;
        }
        let path_access = PathAccess { mode, path: path.into() };
        let Ok(serialized_size) = PathAccess::serialized_size(&path_access) else {
            return false;
        };
        let Ok(serialized_size) = usize::try_from(serialized_size) else {
            return false;
        };
        let Some(frame_size) = NonZeroUsize::new(serialized_size) else {
            return false;
        };
        if serialized_size > ENCODED_PATH_CAPACITY {
            return false;
        }

        // Encode before claiming a frame so an unexpected encoding failure
        // cannot publish a malformed shared-memory frame.
        let mut writer: &mut [u8] = &mut encoded[..serialized_size];
        if PathAccess::serialize_into(&mut writer, &path_access).is_err() || !writer.is_empty() {
            return false;
        }
        let Ok(mut frame) = self.claim_frame(ipc_sender, frame_size) else {
            return false;
        };
        frame.copy_from_slice(&encoded[..serialized_size]);
        true
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

#[cfg(target_os = "linux")]
fn accelerated_path_must_fallback(path: &[u8]) -> bool {
    path.starts_with(b"/dev/") || path.starts_with(b"/proc/") || path.starts_with(b"/sys/")
}

static CLIENT: OnceLock<Client> = OnceLock::new();

pub fn global_client() -> Option<&'static Client> {
    CLIENT.get()
}

pub unsafe fn handle_open(path: impl ToAbsolutePath, mode: impl ToAccessMode) {
    if let Some(client) = global_client() {
        // SAFETY: path and mode contain valid pointers/values forwarded from the interposed function's caller
        unsafe { client.try_handle_open(path, mode) }.unwrap();
    }
}

#[cfg(not(test))]
#[ctor::ctor(unsafe)]
fn init_client() {
    let client = Client::from_env();
    #[cfg(target_os = "linux")]
    let post_syscall_pc = client.encoded_payload.payload.seccomp_payload.post_syscall_pc();
    CLIENT.set(client).unwrap();
    #[cfg(target_os = "linux")]
    crate::accel::init(post_syscall_pc);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::accelerated_path_must_fallback;

    #[test]
    fn accelerated_suppression_uses_seccomp_fallback() {
        assert!(accelerated_path_must_fallback(b"/dev/null"));
        assert!(accelerated_path_must_fallback(b"/proc/self/maps"));
        assert!(accelerated_path_must_fallback(b"/sys/kernel"));
        assert!(!accelerated_path_must_fallback(b"/workspace/file"));
    }
}
