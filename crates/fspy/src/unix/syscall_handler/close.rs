use std::{io, os::fd::RawFd, path::Path};

use fspy_seccomp_unotify::supervisor::handler::arg::{Caller, Fd};
use fspy_shared::ipc::AccessMode;
use libc::pid_t;

use super::{
    FileEventKind, FileEventPath, SyscallHandler, access_mode_from_flags, invoke_callback,
    is_ignored_callback_path,
};

impl SyscallHandler {
    /// Handles a `close` notification: runs the blocking pre-close callback
    /// while the target is still blocked in the syscall, then lets it proceed.
    #[expect(
        clippy::unnecessary_wraps,
        clippy::needless_pass_by_ref_mut,
        reason = "the `&mut self -> io::Result<()>` signature is required by the impl_handler! macro"
    )]
    pub(super) fn close(&mut self, caller: Caller<'_>, (fd,): (Fd,)) -> io::Result<()> {
        let Some(callback) = &self.callback else {
            return Ok(());
        };
        let Some(mode) = fd_access_mode(caller.pid(), fd.raw()) else {
            // Not a regular open file descriptor.
            return Ok(());
        };
        if !mode.intersects(callback.mask) {
            return Ok(());
        }
        let Ok(path_os) = fd.get_path(caller) else {
            return Ok(());
        };
        let path = Path::new(&path_os);
        if !path.is_absolute() || is_ignored_callback_path(path) {
            // Sockets, pipes and anonymous inodes resolve to non-absolute names.
            return Ok(());
        }
        // Open the file read-only purely so the callback can inspect it. The
        // target's own descriptor cannot be extracted under seccomp.
        let Ok(owned_fd) = super::open_in_supervisor(path, libc::O_RDONLY) else {
            return Ok(());
        };
        invoke_callback(
            callback,
            FileEventKind::Closing,
            caller,
            mode,
            FileEventPath::Close(Some(path)),
            &owned_fd,
        );
        // `pending_response` stays `Continue`: the target performs the close.
        Ok(())
    }
}

/// Reads the access mode of an open descriptor from `/proc/<pid>/fdinfo/<fd>`.
fn fd_access_mode(pid: pid_t, raw_fd: RawFd) -> Option<AccessMode> {
    let content = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{raw_fd}")).ok()?;
    let flags_field = content.lines().find_map(|line| line.strip_prefix("flags:"))?;
    let flags = std::ffi::c_int::from_str_radix(flags_field.trim(), 8).ok()?;
    Some(access_mode_from_flags(flags))
}
