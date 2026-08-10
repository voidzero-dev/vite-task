//! Reusable Unix client for reporting file accesses to the fspy supervisor.

// Compile as an empty crate on non-Unix targets and on musl, matching the
// current preload client. Musl support will be enabled as its dependencies are
// made usable before libc initialization.
#![cfg(all(unix, not(target_env = "musl")))]

pub mod convert;
pub mod raw_exec;

use std::{ffi::OsStr, fmt::Debug, num::NonZeroUsize, os::unix::ffi::OsStrExt as _, path::Path};

use convert::{ToAbsolutePath, ToAccessMode};
use fspy_shared::ipc::{PathAccess, channel::Sender};
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::{EncodedPayload, decode_payload_from_env},
    spawn::{PreExec, handle_exec},
};
use raw_exec::RawExec;
use wincode::Serialize as _;

pub struct Client {
    encoded_payload: EncodedPayload,
    ipc_sender: Option<Sender>,
}

// SAFETY: construction owns every field, later methods borrow them immutably,
// and the sender synchronizes its shared-memory access.
#[cfg(target_os = "macos")]
unsafe impl Sync for Client {}
// SAFETY: ownership of every field can move with the client, and the sender
// synchronizes its shared-memory access.
#[cfg(target_os = "macos")]
unsafe impl Send for Client {}

impl Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish()
    }
}

impl Client {
    /// Constructs a client from the encoded payload in the process environment.
    ///
    /// # Panics
    ///
    /// Panics when the payload is missing, malformed, or cannot be decoded.
    #[expect(
        clippy::print_stderr,
        reason = "the client intentionally reports an unavailable supervisor channel"
    )]
    pub fn from_env(envs: impl Iterator<Item = sigsafe::env::Entry>) -> Self {
        let encoded_payload = decode_payload_from_env(envs).unwrap();

        let ipc_sender = match encoded_payload.payload.ipc_channel_conf.sender() {
            Ok(sender) => Some(sender),
            Err(err) => {
                // This can happen if the process starts after the root target
                // has exited and the receiver has closed the channel.
                eprintln!("fspy: failed to create ipc sender: {err}");
                None
            }
        };

        Self { encoded_payload, ipc_sender }
    }

    fn send(&self, mode: fspy_shared::ipc::AccessMode, path: &Path) -> anyhow::Result<()> {
        let Some(ipc_sender) = &self.ipc_sender else {
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

    /// Resolves and reports an exec before forwarding its transformed arguments.
    ///
    /// # Safety
    ///
    /// `raw_exec` must contain the valid C strings and pointer arrays required
    /// by [`RawExec::to_exec`]. The callback must not retain pointers from the
    /// transformed `RawExec` after it returns.
    ///
    /// # Errors
    ///
    /// Returns errors from exec resolution, platform preparation, or the
    /// forwarding callback.
    ///
    /// # Panics
    ///
    /// Panics if reporting the executable path fails.
    pub unsafe fn handle_exec<R>(
        &self,
        config: ExecResolveConfig,
        raw_exec: RawExec,
        f: impl FnOnce(RawExec, Option<PreExec>) -> nix::Result<R>,
    ) -> nix::Result<R> {
        // SAFETY: raw_exec contains valid pointers to C strings and
        // null-terminated arrays, as provided by the caller.
        let mut exec = unsafe { raw_exec.to_exec() };
        let pre_exec = handle_exec(&mut exec, config, &self.encoded_payload, |mode, path| {
            self.send(mode, path).unwrap();
        })?;
        RawExec::from_exec(exec, |raw_command| f(raw_command, pre_exec))
    }

    /// Resolves and reports one intercepted file access.
    ///
    /// # Safety
    ///
    /// `path` and `mode` must satisfy their implementation-specific raw
    /// pointer and descriptor contracts for the duration of this call.
    ///
    /// # Errors
    ///
    /// Returns errors from path resolution or shared-memory serialization.
    pub unsafe fn try_handle_open(
        &self,
        path: impl ToAbsolutePath,
        mode: impl ToAccessMode,
    ) -> anyhow::Result<()> {
        // SAFETY: mode contains a valid pointer (if ModeStr) or a plain value,
        // as provided by the caller.
        let mode = unsafe { mode.to_access_mode() };
        let arena = sigsafe_alloc::arena();
        let Some(abs_path) = path.to_absolute_path(&arena)? else {
            return Ok(());
        };
        self.send(mode, Path::new(OsStr::from_bytes(abs_path.as_bytes())))
    }
}
