mod execve;
mod getdents;
mod open;
mod stat;

use std::{
    borrow::Cow,
    ffi::{OsStr, c_int},
    io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use fspy_seccomp_unotify::{
    impl_handler,
    supervisor::handler::arg::{CStrPtr, Caller, Fd},
};
use fspy_shared::ipc::{AccessMode, PathAccess, channel::Sender};

const PATH_MAX: usize = libc::PATH_MAX as usize;

/// Records the accesses of intercepted syscalls into the IPC channel.
///
/// The supervisor lets a syscall continue only after its handler returns, so
/// every record is published before the access it describes, as the channel
/// requires of its senders.
pub struct SyscallHandler {
    ipc_sender: Arc<Sender>,
    path_read_buf: [u8; PATH_MAX],
}

impl SyscallHandler {
    pub const fn new(ipc_sender: Arc<Sender>) -> Self {
        Self { ipc_sender, path_read_buf: [0; PATH_MAX] }
    }

    fn handle_open(
        &mut self,
        caller: Caller,
        dir_fd: Fd,
        path_ptr: CStrPtr,
        flags: c_int,
    ) -> io::Result<()> {
        let Some(path_len) = path_ptr.read(caller, &mut self.path_read_buf)? else {
            // Ignore paths that are too long to fit in PATH_MAX
            return Ok(());
        };
        let mut path = Cow::Borrowed(Path::new(OsStr::from_bytes(&self.path_read_buf[..path_len])));
        if !path.is_absolute() {
            let mut resolved_path = PathBuf::from(dir_fd.get_path(caller)?);
            if !nix::NixPath::is_empty(path.as_ref()) {
                resolved_path.push(&path);
            }
            path = Cow::Owned(resolved_path);
        }
        self.ipc_sender.send(&PathAccess {
            mode: match flags & libc::O_ACCMODE {
                libc::O_RDWR => AccessMode::READ | AccessMode::WRITE,
                libc::O_WRONLY => AccessMode::WRITE,
                _ => AccessMode::READ,
            },
            path: path.as_os_str().into(),
        });
        Ok(())
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "same receiver as `handle_open`, which writes `path_read_buf`"
    )]
    fn handle_open_dir(&mut self, caller: Caller, fd: Fd) -> io::Result<()> {
        let path = fd.get_path(caller)?;
        self.ipc_sender.send(&PathAccess {
            mode: AccessMode::READ_DIR,
            path: OsStr::from_bytes(path.as_bytes()).into(),
        });
        Ok(())
    }
}

impl_handler!(
    SyscallHandler:

    #[cfg(target_arch = "x86_64")] open,
    openat,
    openat2,

    #[cfg(target_arch = "x86_64")] getdents,
    getdents64,

    #[cfg(target_arch = "x86_64")] stat,
    #[cfg(target_arch = "x86_64")] lstat,
    #[cfg(target_arch = "x86_64")] newfstatat,
    #[cfg(target_arch = "aarch64")] fstatat,
    statx,

    #[cfg(target_arch = "x86_64")] access,
    faccessat,
    faccessat2,

    execve,
    execveat,
);
