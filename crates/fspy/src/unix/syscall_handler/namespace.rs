//! Syscalls that put an existing file or folder at a new path without opening
//! it: rename and hard link. The new path of a rename is recorded as a rename
//! target, which may be a whole folder, and that of a hard link, always a
//! file, as a write. The source of a rename needs no record.

use std::io;

use fspy_seccomp_unotify::supervisor::handler::arg::{CStrPtr, Caller, Fd, Ignored};
use fspy_shared::ipc::AccessMode;

use super::SyscallHandler;

impl SyscallHandler {
    #[cfg(target_arch = "x86_64")]
    pub(super) fn rename(
        &mut self,
        caller: Caller,
        (_old, new): (Ignored, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, Fd::cwd(), new, AccessMode::RenameTarget)
    }

    pub(super) fn renameat(
        &mut self,
        caller: Caller,
        (_old_dir_fd, _old, new_dir_fd, new): (Ignored, Ignored, Fd, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, new_dir_fd, new, AccessMode::RenameTarget)
    }

    /// With `RENAME_EXCHANGE` both paths change; only the destination is
    /// recorded, as for a plain rename.
    pub(super) fn renameat2(
        &mut self,
        caller: Caller,
        (_old_dir_fd, _old, new_dir_fd, new): (Ignored, Ignored, Fd, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, new_dir_fd, new, AccessMode::RenameTarget)
    }

    #[cfg(target_arch = "x86_64")]
    pub(super) fn link(
        &mut self,
        caller: Caller,
        (_old, new): (Ignored, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, Fd::cwd(), new, AccessMode::Write)
    }

    pub(super) fn linkat(
        &mut self,
        caller: Caller,
        (_old_dir_fd, _old, new_dir_fd, new): (Ignored, Ignored, Fd, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, new_dir_fd, new, AccessMode::Write)
    }
}
