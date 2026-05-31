//! Optional blocking open/close callbacks.
//!
//! When a callback is registered on a [`Command`](crate::Command), the traced
//! process blocks and round-trips to the supervisor right after a file is
//! opened and right before a file is closed. The callback receives a file
//! descriptor / handle that is usable inside the supervisor's own process.

// The Unix-domain-socket supervisor is only used by the preload backend, which
// itself is excluded on musl targets (those use seccomp instead).
#[cfg(all(unix, not(target_env = "musl")))]
pub mod unix;
#[cfg(windows)]
pub mod windows;

#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd};
#[cfg(windows)]
use std::os::windows::io::{AsHandle, BorrowedHandle};
use std::{fs::File, mem::ManuallyDrop, path::Path, sync::Arc};

use fspy_shared::ipc::AccessMode;

/// Whether a [`FileEvent`] fires right after an open or right before a close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEventKind {
    /// The file has just been opened; the fd/handle is valid.
    Opened,
    /// The file is about to be closed; the fd/handle is still valid.
    Closing,
}

/// The path carried by a [`FileEvent`].
#[derive(Debug, Clone, Copy)]
pub enum FileEventPath<'a> {
    /// An [`FileEventKind::Opened`] event always carries the absolute path.
    Open(&'a Path),
    /// A [`FileEventKind::Closing`] event resolves the path from the
    /// fd/handle; resolution may fail (anonymous fd, deleted file,
    /// unrecoverable handle).
    Close(Option<&'a Path>),
}

impl<'a> FileEventPath<'a> {
    /// The path, if known.
    #[must_use]
    pub const fn get(self) -> Option<&'a Path> {
        match self {
            Self::Open(path) => Some(path),
            Self::Close(path) => path,
        }
    }
}

/// A file descriptor / handle borrowed for the duration of a callback.
///
/// It is valid and usable inside the supervisor's own process. The supervisor
/// owns the underlying descriptor and closes it as soon as the callback
/// returns, so it must not be stored beyond the callback.
#[derive(Debug, Clone, Copy)]
pub struct BorrowedFile<'a> {
    #[cfg(unix)]
    fd: BorrowedFd<'a>,
    #[cfg(windows)]
    handle: BorrowedHandle<'a>,
}

impl<'a> BorrowedFile<'a> {
    #[cfg(unix)]
    #[must_use]
    pub const fn new(fd: BorrowedFd<'a>) -> Self {
        Self { fd }
    }

    #[cfg(windows)]
    #[must_use]
    pub const fn new(handle: BorrowedHandle<'a>) -> Self {
        Self { handle }
    }

    /// Borrow this fd/handle as a [`std::fs::File`] for `read`/`seek`.
    ///
    /// The returned value is wrapped in [`ManuallyDrop`] and must not be
    /// unwrapped and dropped — the supervisor owns and closes the descriptor.
    #[must_use]
    pub fn as_file(self) -> ManuallyDrop<File> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            // SAFETY: the fd is valid for the borrow; `ManuallyDrop` ensures
            // the `File` destructor never runs, so the fd is not closed here.
            ManuallyDrop::new(unsafe { File::from_raw_fd(self.fd.as_raw_fd()) })
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::{AsRawHandle, FromRawHandle};
            // SAFETY: the handle is valid for the borrow; `ManuallyDrop`
            // ensures the `File` destructor never runs.
            ManuallyDrop::new(unsafe { File::from_raw_handle(self.handle.as_raw_handle()) })
        }
    }
}

#[cfg(unix)]
impl AsFd for BorrowedFile<'_> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd
    }
}

#[cfg(windows)]
impl AsHandle for BorrowedFile<'_> {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle
    }
}

/// An open/close event delivered to a registered file callback.
///
/// While the callback runs, the traced process is blocked.
#[derive(Debug)]
pub struct FileEvent<'a> {
    /// Whether the file was just opened or is about to be closed.
    pub kind: FileEventKind,
    /// Process id of the traced process that opened/closed the file.
    pub pid: u32,
    /// Access mode of the file.
    pub mode: AccessMode,
    /// Path of the file. Always present for [`FileEventKind::Opened`].
    pub path: FileEventPath<'a>,
    /// A file descriptor / handle usable inside the supervisor process.
    pub fd: BorrowedFile<'a>,
}

/// A boxed user callback invoked for matching open/close events.
pub type FileCallbackFn = dyn Fn(FileEvent<'_>) + Send + Sync + 'static;

/// The registered callback together with its access-mode mask.
#[derive(Clone)]
pub struct FileCallback {
    /// An event fires the callback only if its mode intersects this mask.
    pub mask: AccessMode,
    /// The user callback.
    pub callback: Arc<FileCallbackFn>,
}
