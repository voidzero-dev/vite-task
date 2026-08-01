#[cfg(not(target_env = "musl"))]
pub mod channel;
mod native_path;
use std::fmt::Debug;

use bitflags::bitflags;
pub use native_path::NativePath;
pub use native_str::NativeStr;
use wincode::{SchemaRead, SchemaWrite};

#[derive(SchemaWrite, SchemaRead, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct AccessMode(u16);

bitflags! {
    /// What a process asked the kernel to do with one path.
    ///
    /// Every flag here is derived from the call's *arguments*, never from its
    /// result. A tracer that reported outcomes could not behave the same on all
    /// backends: the Linux seccomp supervisor is notified before the syscall
    /// runs and never learns whether it worked. Restricting the vocabulary to
    /// pre-call information keeps every backend equivalent, and makes a trace a
    /// function of the arguments alone rather than of filesystem state at trace
    /// time.
    ///
    /// `READ`, `WRITE` and `READ_DIR` are intent. The rest qualify it, and
    /// consumers need them because `WRITE` on its own is only capability:
    /// truncating, exclusively creating, or being a rename destination is what
    /// makes an access a mutation.
    impl AccessMode: u16 {
        /// Opened for reading, or its metadata was inspected.
        const READ = 1;
        /// Opened for writing. On its own this is capability, not mutation.
        const WRITE = 1 << 1;
        /// Directory entries were listed.
        const READ_DIR = 1 << 2;
        /// The open carried `O_CREAT`.
        const CREATE = 1 << 3;
        /// The open carried `O_TRUNC`, so any previous contents are discarded.
        const TRUNCATE = 1 << 4;
        /// The open carried `O_EXCL`, so the path is new by construction and had
        /// no previous contents to read.
        const EXCLUSIVE = 1 << 5;
        /// The path was named as the source of a rename.
        const RENAME_FROM = 1 << 6;
        /// The path was named as the destination of a rename, which replaces
        /// whatever is there.
        const RENAME_TO = 1 << 7;
        /// Removal was requested through `unlink`, `remove` or `rmdir`.
        const DELETED = 1 << 8;
        /// The path is a directory. Set on rename and removal events so a
        /// consumer can re-attribute writes recorded beneath a renamed
        /// directory.
        const IS_DIR = 1 << 9;
    }
}

impl AccessMode {
    /// Whether this access changes the path's contents.
    ///
    /// Write capability alone does not qualify. A descriptor opened `O_RDWR` and
    /// never written leaves the file untouched, which is what Biome does to a
    /// clean source file on a warm run, and what every `flock`-only lock file
    /// looks like.
    #[must_use]
    pub const fn is_mutation(self) -> bool {
        if self.contains(Self::RENAME_TO) {
            return true;
        }
        self.contains(Self::WRITE)
            && self.intersects(Self::TRUNCATE.union(Self::CREATE.union(Self::EXCLUSIVE)))
    }

    /// Whether this access can observe existing content, making the path a
    /// genuine dependency.
    ///
    /// An exclusive create observes nothing, because the path is new by
    /// definition. That is how Go's `os.CreateTemp` opens atomic-write
    /// temporaries, which would otherwise look read-first.
    #[must_use]
    pub const fn is_content_read(self) -> bool {
        if self.contains(Self::EXCLUSIVE) {
            return false;
        }
        self.intersects(Self::READ.union(Self::READ_DIR))
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
    pub path: &'a NativePath,
    // TODO: add follow_symlinks (O_NOFOLLOW)
}

impl<'a> PathAccess<'a> {
    pub fn read(path: impl Into<&'a NativePath>) -> Self {
        Self { mode: AccessMode::READ, path: path.into() }
    }

    pub fn read_dir(path: impl Into<&'a NativePath>) -> Self {
        Self { mode: AccessMode::READ_DIR, path: path.into() }
    }
}
