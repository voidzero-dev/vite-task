//! Traced-process side of the blocking open/close callback channel.
//!
//! For each matching open/close, the traced process connects to the
//! supervisor's named pipe, sends a length-prefixed [`CallbackRequest`]
//! followed by the raw file handle, and blocks until the supervisor writes
//! back a single [`CALLBACK_ACK`] byte.

use std::{cell::Cell, ptr};

use dashmap::DashMap;
use fspy_shared::{
    callback::{CALLBACK_ACK, CallbackKind, CallbackRequest},
    ipc::{AccessMode, NativePath},
};
use rustc_hash::FxBuildHasher;
use winapi::{
    shared::{minwindef::DWORD, ntdef::HANDLE, winerror::ERROR_PIPE_BUSY},
    um::{
        errhandlingapi::GetLastError,
        fileapi::{CreateFileW, OPEN_EXISTING, ReadFile, WriteFile},
        handleapi::{CloseHandle, INVALID_HANDLE_VALUE},
        processthreadsapi::GetCurrentProcessId,
        winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ, GENERIC_WRITE},
    },
};

use crate::windows::winapi_utils::get_path_name;

// `WaitNamedPipeW` is not exposed by the `winapi` crate.
unsafe extern "system" {
    fn WaitNamedPipeW(name: *const u16, timeout: DWORD) -> i32;
}

thread_local! {
    /// Set while the calling thread is inside a callback round-trip, so the
    /// open/close detours do not recurse into the pipe I/O.
    static IN_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread is currently inside a callback round-trip.
pub fn is_reentrant() -> bool {
    IN_CALLBACK.with(Cell::get)
}

struct ReentryGuard;

impl ReentryGuard {
    fn enter() -> Self {
        IN_CALLBACK.with(|flag| flag.set(true));
        Self
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        IN_CALLBACK.with(|flag| flag.set(false));
    }
}

/// Endpoint, access-mode mask, and open-file-handle tracking for the blocking
/// callback channel.
pub struct CallbackChannel {
    /// NUL-terminated wide pipe name for `CreateFileW`.
    pipe_name: Vec<u16>,
    mask: AccessMode,
    /// Lock-free set of currently-open file handles whose open mode matched
    /// the mask, mapped to that mode. Used to fire the close callback only for
    /// real tracked file handles (`NtClose` runs for every handle type).
    open_handles: DashMap<usize, AccessMode, FxBuildHasher>,
}

impl CallbackChannel {
    /// Builds the channel from the payload, or `None` when no callback is
    /// registered (empty pipe name).
    pub fn from_payload(pipe_name: &[u8], mask: AccessMode) -> Option<Self> {
        if pipe_name.is_empty() {
            return None;
        }
        let pipe_name_wide =
            pipe_name.iter().map(|&byte| u16::from(byte)).chain(std::iter::once(0)).collect();
        Some(Self {
            pipe_name: pipe_name_wide,
            mask,
            open_handles: DashMap::with_hasher(FxBuildHasher),
        })
    }

    /// Runs the post-open blocking callback for a freshly opened handle.
    pub fn run_open_callback(&self, handle: HANDLE, mode: AccessMode, path: &[u16]) {
        if is_reentrant() || !mode.intersects(self.mask) {
            return;
        }
        self.open_handles.insert(handle as usize, mode);
        let _guard = ReentryGuard::enter();
        self.round_trip(CallbackKind::OPENED, handle, mode, Some(path));
    }

    /// Runs the pre-close blocking callback for a still-valid handle, if it is
    /// a tracked file handle.
    pub fn run_close_callback(&self, handle: HANDLE) {
        if is_reentrant() {
            return;
        }
        let Some((_, mode)) = self.open_handles.remove(&(handle as usize)) else {
            // Not a tracked file handle (or its open mode did not match the mask).
            return;
        };
        let _guard = ReentryGuard::enter();
        // SAFETY: `handle` is still a valid file handle (the close has not run yet).
        let path = unsafe { get_path_name(handle) }.ok();
        self.round_trip(CallbackKind::CLOSING, handle, mode, path.as_deref());
    }

    #[expect(clippy::print_stderr, reason = "preload library uses stderr for debug diagnostics")]
    fn round_trip(
        &self,
        kind: CallbackKind,
        handle: HANDLE,
        mode: AccessMode,
        path: Option<&[u16]>,
    ) {
        if let Err(err) = self.round_trip_inner(kind, handle, mode, path) {
            eprintln!("fspy: open/close callback round-trip failed: {err}");
        }
    }

    fn round_trip_inner(
        &self,
        kind: CallbackKind,
        handle: HANDLE,
        mode: AccessMode,
        path: Option<&[u16]>,
    ) -> Result<(), String> {
        let native_path = path.map(NativePath::from_wide);
        // SAFETY: `GetCurrentProcessId` is always safe to call.
        let pid = unsafe { GetCurrentProcessId() };
        let request = CallbackRequest { kind, mode, pid, path: native_path };
        let body = wincode::serialize(&request).map_err(|err| err.to_string())?;
        let len = u32::try_from(body.len()).map_err(|err| err.to_string())?;

        let pipe = self.connect()?;
        let result = (|| {
            write_all(pipe, &len.to_le_bytes())?;
            write_all(pipe, &body)?;
            write_all(pipe, &(handle as usize as u64).to_le_bytes())?;
            let mut ack = [0u8; 1];
            read_exact(pipe, &mut ack)?;
            if ack[0] != CALLBACK_ACK {
                return Err("unexpected callback acknowledgement byte".to_owned());
            }
            Ok(())
        })();
        // SAFETY: `pipe` is a valid handle returned by `CreateFileW`.
        unsafe { CloseHandle(pipe) };
        result
    }

    /// Opens the supervisor's named pipe, waiting briefly if all instances are
    /// currently busy.
    fn connect(&self) -> Result<HANDLE, String> {
        for _ in 0..50 {
            // SAFETY: `pipe_name` is a valid NUL-terminated wide string.
            let pipe = unsafe {
                CreateFileW(
                    self.pipe_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    ptr::null_mut(),
                    OPEN_EXISTING,
                    0,
                    ptr::null_mut(),
                )
            };
            if pipe != INVALID_HANDLE_VALUE {
                return Ok(pipe);
            }
            // SAFETY: `GetLastError` is always safe to call.
            if unsafe { GetLastError() } != ERROR_PIPE_BUSY {
                return Err("failed to open the callback pipe".to_owned());
            }
            // SAFETY: `pipe_name` is a valid NUL-terminated wide string.
            unsafe { WaitNamedPipeW(self.pipe_name.as_ptr(), 100) };
        }
        Err("callback pipe stayed busy".to_owned())
    }
}

fn write_all(pipe: HANDLE, mut buf: &[u8]) -> Result<(), String> {
    while !buf.is_empty() {
        let mut written: DWORD = 0;
        let len = DWORD::try_from(buf.len()).unwrap_or(DWORD::MAX);
        // SAFETY: `pipe` is valid and `buf` is a valid readable slice.
        let ok =
            unsafe { WriteFile(pipe, buf.as_ptr().cast(), len, &raw mut written, ptr::null_mut()) };
        if ok == 0 || written == 0 {
            return Err("failed to write to the callback pipe".to_owned());
        }
        buf = &buf[written as usize..];
    }
    Ok(())
}

fn read_exact(pipe: HANDLE, mut buf: &mut [u8]) -> Result<(), String> {
    while !buf.is_empty() {
        let mut read: DWORD = 0;
        let len = DWORD::try_from(buf.len()).unwrap_or(DWORD::MAX);
        // SAFETY: `pipe` is valid and `buf` is a valid writable slice.
        let ok =
            unsafe { ReadFile(pipe, buf.as_mut_ptr().cast(), len, &raw mut read, ptr::null_mut()) };
        if ok == 0 || read == 0 {
            return Err("failed to read from the callback pipe".to_owned());
        }
        buf = &mut buf[read as usize..];
    }
    Ok(())
}
