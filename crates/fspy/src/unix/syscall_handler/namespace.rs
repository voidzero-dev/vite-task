//! Syscalls that make an entry at a path without opening it: mkdir, symlink,
//! and rename and hard link, which put an existing file or folder at a new
//! path. The path of a mkdir is recorded as a mkdir, whether or not the
//! folder is there already; the new path of a rename as a rename target,
//! which may be a whole folder; and that of a symlink, and of a hard link,
//! always a file, as a write. The source of a rename needs no record, and
//! neither does the target of a symlink, which is stored as text, not
//! accessed.

use std::io;

use fspy_seccomp_unotify::supervisor::handler::arg::{CStrPtr, Caller, Fd, Ignored};
use fspy_shared::ipc::AccessMode;

use super::SyscallHandler;

impl SyscallHandler {
    #[cfg(target_arch = "x86_64")]
    pub(super) fn mkdir(&mut self, caller: Caller, (path,): (CStrPtr,)) -> io::Result<()> {
        self.handle_path(caller, Fd::cwd(), path, AccessMode::Mkdir)
    }

    pub(super) fn mkdirat(
        &mut self,
        caller: Caller,
        (dir_fd, path): (Fd, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, dir_fd, path, AccessMode::Mkdir)
    }

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

    #[cfg(target_arch = "x86_64")]
    pub(super) fn symlink(
        &mut self,
        caller: Caller,
        (_target, link): (Ignored, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, Fd::cwd(), link, AccessMode::Write)
    }

    pub(super) fn symlinkat(
        &mut self,
        caller: Caller,
        (_target, new_dir_fd, link): (Ignored, Fd, CStrPtr),
    ) -> io::Result<()> {
        self.handle_path(caller, new_dir_fd, link, AccessMode::Write)
    }
}
