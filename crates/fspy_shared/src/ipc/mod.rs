pub mod channel;
mod ipc_path;
use std::iter;

use bumpalo::Bump;
pub use ipc_path::IpcPath;
pub use vt_ipc_str::IpcStr;
use wincode::{SchemaRead, SchemaWrite};

/// The kind of a single-path access, for building a record before its path
/// is resolved.
///
/// The clients choose it from an intercepted call's arguments, resolve the
/// path, then build the record with [`AccessMode::with_path`]. It is never
/// sent: [`PathAccess`] is the record, and accesses that touch more than one
/// path build it directly.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AccessMode {
    Read,
    Write,
    ReadWrite,
    ReadDir,
}

impl AccessMode {
    #[must_use]
    pub const fn with_path(self, path: &IpcPath) -> PathAccess<'_> {
        match self {
            Self::Read => PathAccess::Read(path),
            Self::Write => PathAccess::Write(path),
            Self::ReadWrite => PathAccess::ReadWrite(path),
            Self::ReadDir => PathAccess::ReadDir(path),
        }
    }
}

/// A file system access made by a traced process, with the paths it touched.
// TODO: add follow_symlinks (O_NOFOLLOW)
#[derive(SchemaWrite, SchemaRead, Debug, Clone, Copy, PartialEq, Eq)]
#[wincode(tag_encoding = "u8")]
pub enum PathAccess<'a> {
    Read(&'a IpcPath),
    Write(&'a IpcPath),
    ReadWrite(&'a IpcPath),
    ReadDir(&'a IpcPath),
}

impl<'a> PathAccess<'a> {
    /// The paths this access touched.
    pub fn paths(self) -> impl Iterator<Item = &'a IpcPath> {
        match self {
            PathAccess::Read(path)
            | PathAccess::Write(path)
            | PathAccess::ReadWrite(path)
            | PathAccess::ReadDir(path) => iter::once(path),
        }
    }

    #[must_use]
    pub fn clone_in<'bump>(&self, bump: &'bump Bump) -> PathAccess<'bump> {
        match self {
            PathAccess::Read(path) => PathAccess::Read(path.clone_in(bump)),
            PathAccess::Write(path) => PathAccess::Write(path.clone_in(bump)),
            PathAccess::ReadWrite(path) => PathAccess::ReadWrite(path.clone_in(bump)),
            PathAccess::ReadDir(path) => PathAccess::ReadDir(path.clone_in(bump)),
        }
    }
}
