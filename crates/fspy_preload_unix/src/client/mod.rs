pub mod convert;
pub mod raw_exec;

use std::{
    cell::Cell,
    ffi::OsStr,
    fmt::Debug,
    num::NonZeroUsize,
    os::unix::ffi::OsStrExt as _,
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use convert::{ToAbsolutePath, ToAccessMode};
use fspy_shared::ipc::{
    PathAccess,
    channel::{ClaimError, Sender},
};
use fspy_shared_unix::{
    exec::ExecResolveConfig,
    payload::EncodedPayload,
    spawn::{PreExec, handle_exec},
};
use raw_exec::RawExec;
use wincode::Serialize as _;

pub struct Client {
    encoded_payload: EncodedPayload,
    ipc_sender: Option<Sender>,
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
            Err(err) => {
                // this can happen if the process is started after the root target process has exited.
                // By that time the channel would have been closed in the receiver side.
                // In this case we just leave a message and skip sending any path accesses.
                eprintln!("fspy: failed to create ipc sender: {err}");
                None
            }
        };

        Self { encoded_payload, ipc_sender }
    }

    fn send(&self, mode: fspy_shared::ipc::AccessMode, path: &Path) -> anyhow::Result<()> {
        let Some(ipc_sender) = &self.ipc_sender else {
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

        let _in_flight = SendInFlight::begin();
        let mut frame = match ipc_sender.claim_frame(frame_size) {
            Ok(frame) => frame,
            // The channel was closed because the traced root process exited.
            // Accesses from whatever is left behind are dropped by design.
            Err(ClaimError::Closed) => return Ok(()),
            Err(ClaimError::Capacity) => {
                panic!("fspy: failed to claim frame in shared memory")
            }
        };
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
        let arena = sigsafe_alloc::arena();
        let Some(abs_path) = path.to_absolute_path(&arena)? else {
            return Ok(());
        };
        self.send(mode, Path::new(OsStr::from_bytes(abs_path.as_bytes())))
    }
}

static CLIENT: OnceLock<Client> = OnceLock::new();

/// Sends that are between claiming a frame and finishing it, on any thread.
static IN_FLIGHT_SENDS: AtomicUsize = AtomicUsize::new(0);

struct SendInFlight;

impl SendInFlight {
    fn begin() -> Self {
        IN_FLIGHT_SENDS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for SendInFlight {
    fn drop(&mut self) {
        IN_FLIGHT_SENDS.fetch_sub(1, Ordering::Release);
    }
}

/// Waits until no send is mid-frame on any thread of this process.
///
/// The exit interception calls this so a voluntary exit never abandons a
/// claimed frame, which would make the whole run's tracking incomplete. A send
/// lasts microseconds and never blocks, so this returns almost at once. The
/// deadline covers a thread that died mid-send without running its drop
/// (asynchronous `pthread_cancel` and the like): exit is delayed by at most
/// the deadline, and the runner treats the leaked count as an incomplete run.
pub fn drain_in_flight_sends() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
    while IN_FLIGHT_SENDS.load(Ordering::Acquire) != 0 {
        if std::time::Instant::now() > deadline {
            return;
        }
        std::thread::yield_now();
    }
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
