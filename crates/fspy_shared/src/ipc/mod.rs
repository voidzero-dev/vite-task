pub mod channel;
mod ipc_path;
use std::iter;

pub use ipc_path::IpcPath;
pub use vt_ipc_str::IpcStr;
use wincode::{SchemaRead, SchemaWrite};

#[derive(SchemaWrite, SchemaRead, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
#[wincode(tag_encoding = "u8")]
pub enum AccessMode {
    Read,
    Write,
    ReadWrite,
    ReadDir,
}

#[derive(SchemaWrite, SchemaRead, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathAccess<'a> {
    pub mode: AccessMode,
    pub path: &'a IpcPath,
    // TODO: add follow_symlinks (O_NOFOLLOW)
}

impl<'a> PathAccess<'a> {
    pub fn read(path: impl Into<&'a IpcPath>) -> Self {
        Self { mode: AccessMode::Read, path: path.into() }
    }

    pub fn read_dir(path: impl Into<&'a IpcPath>) -> Self {
        Self { mode: AccessMode::ReadDir, path: path.into() }
    }

    /// The paths this access touched.
    pub fn paths(self) -> impl Iterator<Item = &'a IpcPath> {
        iter::once(self.path)
    }
}
