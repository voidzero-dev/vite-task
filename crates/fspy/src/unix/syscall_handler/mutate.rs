//! Rename and delete, the syscalls that move a task's output into place.
//!
//! Without these a build that stages into a temporary directory and renames it
//! over the real one looks like it wrote nothing that still exists, so its
//! archive comes out empty. The preload backend intercepts the libc wrappers;
//! this backend has to see the syscalls themselves, which is what makes the two
//! agree on musl.
//!
//! Every flag here is derived from the call's arguments. This supervisor
//! responds with `SECCOMP_USER_NOTIF_FLAG_CONTINUE` and is notified before the
//! syscall runs, so it never learns whether the call succeeded.

use std::{ffi::c_int, io};

use fspy_seccomp_unotify::supervisor::handler::arg::{CStrPtr, Caller, Fd};

use super::SyscallHandler;

impl SyscallHandler {
    #[cfg(target_arch = "x86_64")]
    pub(super) fn rename(
        &mut self,
        caller: Caller,
        (old_path, new_path): (CStrPtr, CStrPtr),
    ) -> io::Result<()> {
        self.handle_rename(caller, (Fd::cwd(), old_path), (Fd::cwd(), new_path), false)
    }

    pub(super) fn renameat(
        &mut self,
        caller: Caller,
        (old_dir_fd, old_path, new_dir_fd, new_path): (Fd, CStrPtr, Fd, CStrPtr),
    ) -> io::Result<()> {
        self.handle_rename(caller, (old_dir_fd, old_path), (new_dir_fd, new_path), false)
    }

    pub(super) fn renameat2(
        &mut self,
        caller: Caller,
        (old_dir_fd, old_path, new_dir_fd, new_path, flags): (Fd, CStrPtr, Fd, CStrPtr, c_int),
    ) -> io::Result<()> {
        let exchange = flags & c_int::try_from(libc::RENAME_EXCHANGE).unwrap_or(0) != 0;
        self.handle_rename(caller, (old_dir_fd, old_path), (new_dir_fd, new_path), exchange)
    }

    #[cfg(target_arch = "x86_64")]
    pub(super) fn unlink(&mut self, caller: Caller, (path,): (CStrPtr,)) -> io::Result<()> {
        self.handle_delete(caller, Fd::cwd(), path, false)
    }

    pub(super) fn unlinkat(
        &mut self,
        caller: Caller,
        (dir_fd, path, flags): (Fd, CStrPtr, c_int),
    ) -> io::Result<()> {
        let is_dir = flags & libc::AT_REMOVEDIR != 0;
        self.handle_delete(caller, dir_fd, path, is_dir)
    }

    #[cfg(target_arch = "x86_64")]
    pub(super) fn rmdir(&mut self, caller: Caller, (path,): (CStrPtr,)) -> io::Result<()> {
        self.handle_delete(caller, Fd::cwd(), path, true)
    }
}
