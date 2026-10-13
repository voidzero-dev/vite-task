use std::{
    borrow::Cow,
    ffi::OsStr,
    io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use fspy_seccomp_unotify::supervisor::handler::arg::{CStrPtr, Caller, Fd};
use fspy_shared::ipc::PathAccess;
use fspy_shared_unix::exec::{ParseShebangOptions, parse_shebang};
use nix::{
    fcntl::{OFlag, open},
    sys::stat::Mode,
};

use super::{SyscallHandler, read_path};

/// Reads the start of the file at `path`.
///
/// `O_NONBLOCK` keeps the supervisor from waiting on a FIFO, which the kernel
/// refuses to execute anyway.
fn peek_file(path: &Path, buf: &mut [u8]) -> nix::Result<usize> {
    let fd = open(path, OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK, Mode::empty())?;
    nix::unistd::read(&fd, buf)
}

impl SyscallHandler {
    fn handle_execve(&mut self, caller: Caller, fd: Fd, path_ptr: CStrPtr) -> io::Result<()> {
        let Some(path) = read_path(&mut self.path_read_buf, caller, fd, path_ptr)? else {
            // Ignore paths that are too long to fit in PATH_MAX
            return Ok(());
        };
        self.ipc_sender.send(&PathAccess::Read(path.as_os_str().into()));

        // The kernel opens a script's interpreter itself, without a syscall to
        // intercept. Errors are left for the exec to report.
        let Ok(Some(shebang)) = parse_shebang(peek_file, &path, ParseShebangOptions::linux())
        else {
            return Ok(());
        };
        let interpreter = Path::new(OsStr::from_bytes(&shebang.interpreter));
        // The kernel resolves a relative interpreter against the cwd, even for
        // `execveat`.
        let interpreter = if interpreter.is_absolute() {
            Cow::Borrowed(interpreter)
        } else {
            Cow::Owned(PathBuf::from(Fd::cwd().get_path(caller)?).join(interpreter))
        };
        self.ipc_sender.send(&PathAccess::Read(interpreter.as_os_str().into()));
        Ok(())
    }

    pub(super) fn execveat(
        &mut self,
        caller: Caller,
        (fd, path_ptr): (Fd, CStrPtr),
    ) -> io::Result<()> {
        self.handle_execve(caller, fd, path_ptr)
    }

    pub(super) fn execve(&mut self, caller: Caller, (path_ptr,): (CStrPtr,)) -> io::Result<()> {
        self.handle_execve(caller, Fd::cwd(), path_ptr)
    }
}
