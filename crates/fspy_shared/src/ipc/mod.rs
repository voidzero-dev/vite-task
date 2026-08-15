#[cfg(not(target_env = "musl"))]
pub mod channel;
mod ipc_path;
use std::fmt::Debug;

use bitflags::bitflags;
pub use fspy_ipc_str::IpcStr;
pub use ipc_path::IpcPath;
use wincode::{SchemaRead, SchemaWrite};

#[derive(SchemaWrite, SchemaRead, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct AccessMode(u8);

bitflags! {
    impl AccessMode: u8 {
        const READ = 1;
        const WRITE = 1 << 1;
        const READ_DIR = 1 << 2;
    }
}

impl Debug for AccessMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct InternalAccessMode(AccessMode);
        impl Debug for InternalAccessMode {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                bitflags::parser::to_writer(&self.0, f)
            }
        }
        f.debug_tuple("AccessMode").field(&InternalAccessMode(*self)).finish()
    }
}

#[derive(SchemaWrite, SchemaRead, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathAccess<'a> {
    pub mode: AccessMode,
    pub path: &'a IpcPath,
    // TODO: add follow_symlinks (O_NOFOLLOW)
}

impl<'a> PathAccess<'a> {
    pub fn read(path: impl Into<&'a IpcPath>) -> Self {
        Self { mode: AccessMode::READ, path: path.into() }
    }

    pub fn read_dir(path: impl Into<&'a IpcPath>) -> Self {
        Self { mode: AccessMode::READ_DIR, path: path.into() }
    }
}
