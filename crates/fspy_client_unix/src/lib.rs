//! Reusable Unix client for reporting file accesses to the fspy supervisor.

// Compile as an empty crate on non-Unix targets and on musl, matching the
// current preload client. Musl support will be enabled as its dependencies are
// made usable before libc initialization.
#![cfg(all(unix, not(target_env = "musl")))]

pub mod convert;
pub mod raw_exec;

use std::{ffi::OsStr, fmt::Debug, os::unix::ffi::OsStrExt as _, path::Path};

use convert::{ToAbsolutePath, ToAccessMode};
use fspy_shared::ipc::{PathAccess, channel::Sender};
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::{EncodedPayload, decode_payload_from_env},
    spawn::{PreExec, handle_exec},
};
use raw_exec::RawExec;

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
    /// Panics when the payload is missing, malformed, or cannot be decoded,
    /// and when the channel is there but cannot be attached to (see
    /// [`ChannelConf::sender`](fspy_shared::ipc::channel::ChannelConf::sender)).
    pub fn from_env(envs: impl Iterator<Item = fspy_nostd::env::Entry>) -> Self {
        let encoded_payload = decode_payload_from_env(envs).unwrap();

        // `None` when the channel is already over, which happens when this
        // process starts after the root target exited. Nothing is said
        // about it: a preload library writing to the traced process's
        // stderr corrupts whatever that process is printing.
        let ipc_sender = encoded_payload.payload.ipc_channel_conf.sender();

        Self { encoded_payload, ipc_sender }
    }

    fn send(&self, mode: fspy_shared::ipc::AccessMode, path: &Path) {
        let Some(ipc_sender) = &self.ipc_sender else {
            return;
        };
        let path_bytes = path.as_os_str().as_bytes();
        if path_bytes.starts_with(b"/dev/")
            || (cfg!(target_os = "linux")
                && (path_bytes.starts_with(b"/proc/") || path_bytes.starts_with(b"/sys/")))
        {
            return;
        }
        // The interception proceeds whether or not the record could be
        // sent — a preload library can never panic its host process.
        ipc_sender.send(&PathAccess { mode, path: path.into() });
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
            self.send(mode, path);
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
        let arena = fspy_nostd_alloc::pooled_bump();
        let Some(abs_path) = path.to_absolute_path(&arena)? else {
            return Ok(());
        };
        self.send(mode, Path::new(OsStr::from_bytes(abs_path.as_units())));
        Ok(())
    }
}
