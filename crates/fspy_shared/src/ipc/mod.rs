#[cfg(not(target_env = "musl"))]
pub mod channel;
mod ipc_path;
use std::fmt::Debug;

use bitflags::bitflags;
pub use fspy_ipc_str::IpcStr;
pub use ipc_path::IpcPath;
use wincode::{SchemaRead, SchemaWrite};

/// How much shared memory a channel gets, and how many records fit in it.
///
/// Both numbers come from the caller: this crate has no way to guess how
/// many records a workload makes. Both ends of one channel must agree on
/// the slot count, which the receiver passes to `channel` and every sender
/// reads back out of the `ChannelConf`.
#[derive(Clone, Copy, Debug)]
pub struct ChannelSize {
    /// Bytes of shared memory. The descriptor table takes the front of it
    /// and payloads take the rest.
    pub capacity: usize,
    /// Descriptor slots, one per record. A record past this many is
    /// refused just like one the payload area has no room for.
    pub slots: usize,
}

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
