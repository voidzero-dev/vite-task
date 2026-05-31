//! Wire types for the synchronous open/close callback channel.
//!
//! These travel on a separate request/response transport (a Unix-domain socket
//! on the preload backend, a named pipe on Windows) layered beside the one-way
//! [`crate::ipc`] shared-memory ring. The file descriptor / handle itself is
//! passed out-of-band (`SCM_RIGHTS` / `DuplicateHandle` / seccomp `ADDFD`), so
//! it is not part of the serialized request.

use wincode::{SchemaRead, SchemaWrite};

use crate::ipc::{AccessMode, NativePath};

/// Whether a callback event fires right after an open or right before a close.
#[derive(SchemaWrite, SchemaRead, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallbackKind(u8);

impl CallbackKind {
    /// Fired right before a file is closed; the fd/handle is still valid.
    pub const CLOSING: Self = Self(1);
    /// Fired right after a file was opened; the fd/handle is valid.
    pub const OPENED: Self = Self(0);
}

/// A single open/close callback request sent by a traced process to the
/// supervisor. The supervisor blocks the traced process until it writes back a
/// [`CALLBACK_ACK`] byte.
#[derive(SchemaWrite, SchemaRead, Debug)]
pub struct CallbackRequest<'a> {
    /// Whether this is a post-open or pre-close event.
    pub kind: CallbackKind,
    /// Access mode of the file (the resolved open mode).
    pub mode: AccessMode,
    /// Process id of the traced process that opened/closed the file.
    pub pid: u32,
    /// Absolute path of the file. Always present for [`CallbackKind::OPENED`];
    /// `None` for [`CallbackKind::CLOSING`] when it cannot be resolved from the
    /// fd/handle.
    pub path: Option<&'a NativePath>,
}

/// Single byte the supervisor writes back to release the blocked traced
/// process once the user callback has returned.
pub const CALLBACK_ACK: u8 = 0x01;
