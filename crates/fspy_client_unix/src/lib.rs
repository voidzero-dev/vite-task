//! Reusable Unix client for reporting file accesses to the fspy supervisor.

// Compile as an empty crate on non-Unix targets and on musl, matching the
// current preload client. Musl support will be enabled as its dependencies are
// made usable before libc initialization.
#![cfg(all(unix, not(target_env = "musl")))]

pub mod convert;
pub mod raw_exec;

use std::{ffi::OsStr, fmt::Debug, os::unix::ffi::OsStrExt as _, path::Path};

use allocator_api2::alloc::Allocator;
use convert::{ToAbsolutePath, ToAccessMode};
use fspy_shared::ipc::{AccessMode, channel::Sender};
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::{EncodedPayload, decode_payload_from_env},
    spawn::{PreExec, handle_exec},
};
use raw_exec::RawExec;

pub struct Client<'a> {
    encoded_payload: EncodedPayload<'a>,
    ipc_sender: Option<Sender>,
}

// Seals the view-only design: the client holds views of leaked memory and
// the sender, never an allocator. A retained allocator handle is
// interior-mutable and would fail this assertion. (`Sender`'s own manual
// `Send`/`Sync` impls are the one audited exception the check trusts.)
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Client<'static>>();
};

impl Debug for Client<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish()
    }
}

impl<'a> Client<'a> {
    /// Constructs a client from the encoded payload in the process
    /// environment, leaking the payload's storage into `allocator`.
    ///
    /// The sender's temporary path decode also comes from `allocator` and
    /// drops before this returns; a bump allocator gets that space back,
    /// since the block is its most recent allocation. `A: 'a` bounds the
    /// client's lifetime — a `'static` allocator yields a `Client<'static>`
    /// with no further ceremony — and the client never retains the
    /// allocator itself (see the `Send + Sync` assertion above).
    ///
    /// Returns `None` when the payload is missing, malformed, or cannot be
    /// decoded — e.g. a leaked `LD_PRELOAD` in an env-scrubbed sandbox. The
    /// host process then runs untracked rather than dying in its preload
    /// constructor. When the payload decodes but its channel cannot be
    /// attached to, the client still functions with `ipc_sender: None`: it
    /// reports nothing, and [`Client::report_loss`] is a no-op.
    #[must_use]
    pub fn from_env(
        envs: impl Iterator<Item = fspy_nostd::env::Entry>,
        allocator: impl Allocator + Clone + 'a,
    ) -> Option<Self> {
        let encoded_payload = decode_payload_from_env(envs, allocator.clone()).ok()?;

        // `None` when the channel is already over, which happens when this
        // process starts after the root target exited. Nothing is said
        // about it: a preload library writing to the traced process's
        // stderr corrupts whatever that process is printing.
        let ipc_sender = encoded_payload.payload.ipc_channel_conf.sender(allocator);

        Some(Self { encoded_payload, ipc_sender })
    }

    fn send(&self, mode: AccessMode, path: &Path) {
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
        ipc_sender.send(&mode.with_path(path.into()));
    }

    /// Marks the run's trace incomplete, so the receiver treats it as
    /// untracked and the runner does not cache it. Call it before the
    /// process does something that tracking cannot record, such as an exec
    /// that continues untracked because [`PreExec::run`] failed. It does
    /// nothing when this client has no channel sender.
    pub fn report_loss(&self) {
        if let Some(ipc_sender) = &self.ipc_sender {
            ipc_sender.report_loss();
        }
    }

    /// Resolves and reports an exec, returning the transformed arguments to
    /// forward and what to run before forwarding them.
    ///
    /// # Safety
    ///
    /// `raw_exec` must contain the valid C strings and pointer arrays required
    /// by [`RawExec::to_exec`]. The returned `RawExec` points into
    /// `allocator`, and must not be used after `allocator` is dropped.
    ///
    /// # Errors
    ///
    /// Returns errors from exec resolution or platform preparation.
    pub unsafe fn handle_exec<A: Allocator>(
        &self,
        config: ExecResolveConfig,
        raw_exec: RawExec,
        allocator: &A,
    ) -> nix::Result<(RawExec, Option<PreExec>)> {
        // SAFETY: raw_exec contains valid pointers to C strings and
        // null-terminated arrays, as provided by the caller.
        let mut exec = unsafe { raw_exec.to_exec(allocator) };
        let pre_exec = handle_exec(&mut exec, config, &self.encoded_payload, |path| {
            self.send(AccessMode::Read, path);
        })?;
        Ok((RawExec::from_exec(exec), pre_exec))
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
        allocator: impl Allocator,
    ) -> anyhow::Result<()> {
        // SAFETY: mode contains a valid pointer (if ModeStr) or a plain value,
        // as provided by the caller.
        let mode = unsafe { mode.to_access_mode() };
        let Some(abs_path) = path.to_absolute_path(&allocator)? else {
            return Ok(());
        };
        self.send(mode, Path::new(OsStr::from_bytes(abs_path.as_units())));
        Ok(())
    }
}
