mod execve;
mod getdents;
mod mutate;
mod open;
mod stat;

use std::{
    borrow::Cow,
    ffi::{OsStr, c_int},
    io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use fspy_seccomp_unotify::{
    impl_handler,
    supervisor::handler::arg::{CStrPtr, Caller, Fd},
};
use fspy_shared::ipc::{AccessMode, PathAccess};

use crate::arena::PathAccessArena;

const PATH_MAX: usize = libc::PATH_MAX as usize;

/// Whether a path is a directory right now.
///
/// Read before the syscall runs, so this is existing filesystem state and not
/// the call's result. A path that cannot be stated is reported as a file, which
/// only costs the subtree re-attribution a directory rename would have got.
fn is_existing_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

#[derive(Debug)]
pub struct SyscallHandler {
    arena: PathAccessArena,
    path_read_buf: [u8; PATH_MAX],
}

impl Default for SyscallHandler {
    fn default() -> Self {
        Self { arena: PathAccessArena::default(), path_read_buf: [0; PATH_MAX] }
    }
}

impl SyscallHandler {
    pub fn into_arena(self) -> PathAccessArena {
        self.arena
    }

    /// Reads a syscall's path argument and makes it absolute.
    ///
    /// Returns `None` when the path does not fit in `PATH_MAX`, matching the
    /// rest of this handler: such a path is dropped rather than truncated.
    ///
    /// The result is owned because callers that handle two paths at once, like
    /// rename, cannot both borrow the single read buffer.
    fn resolve_path(
        &mut self,
        caller: Caller,
        dir_fd: Fd,
        path_ptr: CStrPtr,
    ) -> io::Result<Option<PathBuf>> {
        let Some(path_len) = path_ptr.read(caller, &mut self.path_read_buf)? else {
            return Ok(None);
        };
        let path = Path::new(OsStr::from_bytes(&self.path_read_buf[..path_len]));
        if path.is_absolute() {
            return Ok(Some(path.to_path_buf()));
        }
        let mut resolved_path = PathBuf::from(dir_fd.get_path(caller)?);
        if !nix::NixPath::is_empty(path) {
            resolved_path.push(path);
        }
        Ok(Some(resolved_path))
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
        // This supervisor is notified before the syscall runs and responds with
        // SECCOMP_USER_NOTIF_FLAG_CONTINUE, so it never learns the outcome.
        // Only pre-call information is available here: intent plus the creation
        // and truncation flags. Outcome-dependent flags (FAILED, DELETED,
        // CREATED_DIR, RENAME_*) are deliberately not emitted, because guessing
        // them would be worse than omitting them — a failed mkdir would claim a
        // directory this run did not create. See DECISIONS.md D7.
        let mut mode = match flags & libc::O_ACCMODE {
            libc::O_RDWR => AccessMode::READ | AccessMode::WRITE,
            libc::O_WRONLY => AccessMode::WRITE,
            _ => AccessMode::READ,
        };
        if flags & libc::O_CREAT != 0 {
            mode |= AccessMode::CREATE;
        }
        if flags & libc::O_TRUNC != 0 {
            mode |= AccessMode::TRUNCATE;
        }
        if flags & libc::O_EXCL != 0 {
            mode |= AccessMode::EXCLUSIVE;
        }
        self.arena.add(PathAccess { mode, path: path.as_os_str().into() });
        Ok(())
    }

    /// Records a rename as a delete of the source and a write of the
    /// destination.
    ///
    /// Both halves come from the call's arguments. Whether the source is a
    /// directory is read from the filesystem before the syscall runs, which is
    /// current state rather than the call's outcome, and it matters because a
    /// directory rename re-attributes every write recorded under the old
    /// subtree.
    ///
    /// `exchange` covers `RENAME_EXCHANGE`, where the source is replaced by the
    /// destination instead of disappearing, so it is written rather than deleted.
    fn handle_rename(
        &mut self,
        caller: Caller,
        (old_dir_fd, old_path_ptr): (Fd, CStrPtr),
        (new_dir_fd, new_path_ptr): (Fd, CStrPtr),
        exchange: bool,
    ) -> io::Result<()> {
        let Some(source) = self.resolve_path(caller, old_dir_fd, old_path_ptr)? else {
            return Ok(());
        };
        let Some(dest) = self.resolve_path(caller, new_dir_fd, new_path_ptr)? else {
            return Ok(());
        };

        let dir_flag =
            if is_existing_dir(&source) { AccessMode::IS_DIR } else { AccessMode::empty() };

        let source_mode = if exchange {
            AccessMode::RENAME_TO | AccessMode::WRITE | dir_flag
        } else {
            AccessMode::RENAME_FROM | AccessMode::DELETED | dir_flag
        };
        self.arena.add(PathAccess { mode: source_mode, path: source.as_os_str().into() });
        self.arena.add(PathAccess {
            mode: AccessMode::RENAME_TO | AccessMode::WRITE | dir_flag,
            path: dest.as_os_str().into(),
        });
        Ok(())
    }

    /// Records a delete from the call's arguments.
    fn handle_delete(
        &mut self,
        caller: Caller,
        dir_fd: Fd,
        path_ptr: CStrPtr,
        is_dir: bool,
    ) -> io::Result<()> {
        let Some(path) = self.resolve_path(caller, dir_fd, path_ptr)? else {
            return Ok(());
        };
        let dir_flag = if is_dir { AccessMode::IS_DIR } else { AccessMode::empty() };
        self.arena.add(PathAccess {
            mode: AccessMode::DELETED | dir_flag,
            path: path.as_os_str().into(),
        });
        Ok(())
    }

    fn handle_open_dir(&mut self, caller: Caller, fd: Fd) -> io::Result<()> {
        let path = fd.get_path(caller)?;
        self.arena.add(PathAccess {
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

    #[cfg(target_arch = "x86_64")] rename,
    renameat,
    renameat2,
    #[cfg(target_arch = "x86_64")] unlink,
    unlinkat,
    #[cfg(target_arch = "x86_64")] rmdir,

    execve,
    execveat,
);
