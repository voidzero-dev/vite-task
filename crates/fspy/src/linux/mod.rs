use std::{
    ffi::CString,
    io,
    os::{
        fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
        unix::ffi::OsStrExt as _,
    },
    path::{Path, PathBuf},
};

use fspy_shared::ipc::{IpcStr, PathAccess, channel::channel};
use fspy_shared_unix::linux::{BOOTSTRAP_ENV_NAME, Bootstrap, Target, encode_bootstrap};
use futures_util::FutureExt as _;
use linux_raw_sys::{
    prctl::PR_SET_NO_NEW_PRIVS,
    ptrace::{BPF_K, BPF_RET, SECCOMP_RET_ALLOW, SECCOMP_SET_MODE_FILTER, sock_filter, sock_fprog},
};
use syscalls::Sysno;
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;
use vt_materialized_artifact::{Artifact, artifact};

use crate::{
    ChildTermination, Command, TrackedChild, TrackingIncomplete, error::SpawnError,
    ipc::ChannelAccesses,
};

const PRELOAD_LINUX: Artifact = artifact!("fspy_preload", "CARGO_BIN_FILE_FSPY_PRELOAD_LINUX");

#[derive(Debug)]
pub struct SpyImpl {
    /// Materialized executable path to `fspy_preload_linux`, or `None` when the
    /// temp directory does not permit executing files (e.g. a `noexec` mount),
    /// in which case `spawn` falls back to untracked execution.
    preload_path: Option<Box<IpcStr>>,
    seccomp_available: bool,
}

impl SpyImpl {
    /// Materializes `fspy_preload_linux` into the user's temp directory and
    /// probes whether this environment permits installing a seccomp filter.
    pub fn init() -> io::Result<Self> {
        let seccomp_available = probe_seccomp_available()?;
        let preload_path = if seccomp_available {
            let materialized = materialize_executable_preload().map(|p| p.as_path().into());
            if materialized.is_none() {
                tracing::warn!(
                    "the temp directory does not allow executing files, so Linux task \
                     tracking is off and caching is disabled; set TMPDIR to a directory \
                     on a filesystem without `noexec` to enable it"
                );
            }
            materialized
        } else {
            None
        };
        Ok(Self { preload_path, seccomp_available })
    }

    pub(crate) async fn spawn(
        &self,
        mut command: Command,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        let Some(preload_path) =
            (self.seccomp_available).then_some(self.preload_path.as_deref()).flatten()
        else {
            return spawn_untracked(command, cancellation_token);
        };

        let ipc_receiver = channel(crate::ipc::shm_capacity(), allocator_api2::alloc::Global)
            .map_err(SpawnError::ChannelCreation)?;

        let (read_end, write_end) = create_error_pipe().map_err(SpawnError::OsSpawn)?;
        let write_raw_fd = write_end.as_raw_fd();

        let target_path: &IpcStr = command.target_program().into();
        let bootstrap = Bootstrap {
            ipc_channel_conf: ipc_receiver.conf(),
            preload_path,
            target: Target::Unresolved { path: target_path, error_pipe_fd: write_raw_fd },
        };
        let bump = bumpalo::Bump::new();
        let encoded = encode_bootstrap(&bootstrap, &bump);

        command.redirect_program(preload_path.as_os_str());
        command.env("FSPY", "1");
        command.env(BOOTSTRAP_ENV_NAME, encoded);

        let mut tokio_command = command.into_tokio_command();
        // SAFETY: `fcntl(F_SETFD, 0)` is async-signal-safe, and `write_raw_fd`
        // remains open in the parent until `spawn()` finishes.
        unsafe {
            tokio_command.pre_exec(move || {
                if libc::fcntl(write_raw_fd, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = tokio_command.spawn().map_err(SpawnError::OsSpawn)?;
        // Close the parent's copy of the write end immediately so EOF fires if
        // `fspy_preload_linux` terminates before writing a status code.
        drop(write_end);

        let mut pipe_reader = tokio::net::unix::pipe::Receiver::from_owned_fd(read_end)
            .map_err(SpawnError::OsSpawn)?;
        let mut buf = [0u8; 4];
        let mut filled = 0usize;
        let mut cancelled = false;
        while filled < buf.len() {
            tokio::select! {
                read_res = pipe_reader.read(&mut buf[filled..]) => match read_res {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                    Err(err) => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        return Err(SpawnError::OsSpawn(err));
                    }
                },
                () = cancellation_token.cancelled() => {
                    cancelled = true;
                    break;
                }
            }
        }
        drop(pipe_reader);

        if !cancelled {
            if filled < buf.len() {
                let _ = child.wait().await;
                return Err(SpawnError::OsSpawn(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "launcher exited before reporting exec status",
                )));
            }
            let code = i32::from_ne_bytes(buf);
            if code != 0 {
                let _ = child.wait().await;
                return Err(SpawnError::OsSpawn(io::Error::from_raw_os_error(code)));
            }
        }

        Ok(TrackedChild {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            wait_handle: tokio::spawn(async move {
                let status = tokio::select! {
                    status = child.wait() => status?,
                    () = cancellation_token.cancelled() => {
                        child.start_kill()?;
                        child.wait().await?
                    }
                };

                let path_accesses = ChannelAccesses::try_from(ipc_receiver)
                    .map(|ipc_accesses| PathAccessIterable { ipc_accesses });
                io::Result::Ok(ChildTermination { status, path_accesses })
            })
            .map(|f| f?)
            .boxed(),
        })
    }
}

fn spawn_untracked(
    command: Command,
    cancellation_token: CancellationToken,
) -> Result<TrackedChild, SpawnError> {
    let mut tokio_command = command.into_tokio_command();
    let mut child = tokio_command.spawn().map_err(SpawnError::OsSpawn)?;
    Ok(TrackedChild {
        stdin: child.stdin.take(),
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
        wait_handle: tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status?,
                () = cancellation_token.cancelled() => {
                    child.start_kill()?;
                    child.wait().await?
                }
            };
            io::Result::Ok(ChildTermination { status, path_accesses: Err(TrackingIncomplete) })
        })
        .map(|f| f?)
        .boxed(),
    })
}

/// Materializes `fspy_preload_linux` into the user's temp directory, or
/// returns `None` when that directory cannot hold it or sits on a `noexec`
/// mount (such as a CIS-hardened `/tmp`), so `SpyImpl::spawn` falls back to
/// untracked execution.
fn materialize_executable_preload() -> Option<PathBuf> {
    let path = PRELOAD_LINUX.materialize().executable().at_user_temp_dir("vp_run").ok()?;
    is_path_executable(&path).then_some(path)
}

/// Returns `true` when `path` is executable (`access(X_OK) == 0`) and not on a
/// `noexec` mount (`statvfs.f_flag & ST_NOEXEC == 0`).
fn is_path_executable(path: &Path) -> bool {
    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `cpath` is a valid NUL-terminated path.
    if unsafe { libc::access(cpath.as_ptr(), libc::X_OK) } != 0 {
        return false;
    }
    // SAFETY: `libc::statvfs` is a plain C struct valid when zero-initialized.
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `cpath` is NUL-terminated and `vfs` is valid writable storage.
    if unsafe { libc::statvfs(cpath.as_ptr(), &raw mut vfs) } != 0 {
        return false;
    }
    (vfs.f_flag & libc::ST_NOEXEC) == 0
}

/// Probes once in a short-lived forked child whether `PR_SET_NO_NEW_PRIVS` and
/// `seccomp(SECCOMP_SET_MODE_FILTER)` are permitted in the current environment.
fn probe_seccomp_available() -> io::Result<bool> {
    // SAFETY: `fork()` is safe to call here; the child performs only
    // async-signal-safe raw syscalls and exits immediately with `_exit`.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        #[expect(clippy::cast_possible_truncation, reason = "cBPF opcode fits in u16")]
        let filter =
            [sock_filter { code: (BPF_RET | BPF_K) as u16, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW }];
        let prog = sock_fprog { len: 1, filter: filter.as_ptr().cast_mut() };
        // SAFETY: async-signal-safe raw syscalls on stack-local BPF program.
        let ok = unsafe {
            syscalls::syscall!(Sysno::prctl, PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0).is_ok()
                && syscalls::syscall!(Sysno::seccomp, SECCOMP_SET_MODE_FILTER, 0, &raw const prog)
                    .is_ok()
        };
        // SAFETY: terminates the forked child immediately without running destructors.
        unsafe { libc::_exit(i32::from(!ok)) };
    }

    let mut status = 0;
    loop {
        // SAFETY: `pid` is our child process and `status` is valid writable storage.
        let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
        if waited < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        return Ok(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    }
}

/// Creates a `CLOEXEC` pipe whose read end is non-blocking (for tokio's async
/// pipe receiver) and whose write end stays blocking for the 4-byte startup
/// status write from `fspy_preload_linux`.
fn create_error_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: `fds` is a valid 2-element array for `pipe2`.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe2` returned two newly opened descriptors.
    let read_end = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // SAFETY: `pipe2` returned two newly opened descriptors.
    let write_end = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    // SAFETY: `read_end` is a valid open descriptor.
    let flags = unsafe { libc::fcntl(read_end.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: setting `O_NONBLOCK` on our owned pipe read end.
    if unsafe { libc::fcntl(read_end.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok((read_end, write_end))
}

pub struct PathAccessIterable {
    ipc_accesses: ChannelAccesses,
}

impl PathAccessIterable {
    /// Iterates over the path accesses in the order they were made.
    ///
    /// Accesses made at the same time by different threads or processes
    /// appear in an unspecified order relative to each other.
    pub fn iter(&self) -> impl Iterator<Item = PathAccess<'_>> {
        self.ipc_accesses.iter_path_accesses()
    }
}

#[cfg(test)]
mod tests {
    use elf::{ElfBytes, abi, endian::LittleEndian, file::Class};

    const PRELOAD_LINUX_BYTES: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_FSPY_PRELOAD_LINUX"));

    #[test]
    fn preload_linux_image_uses_high_fixed_addresses_without_runtime_relocations() {
        let image = ElfBytes::<LittleEndian>::minimal_parse(PRELOAD_LINUX_BYTES).unwrap();
        assert_eq!(image.ehdr.class, Class::ELF64);
        assert_eq!(image.ehdr.e_type, abi::ET_EXEC);
        let (base, limit) = match image.ehdr.e_machine {
            abi::EM_X86_64 => (0x5500_0000_0000, 0x5a00_0000_0000),
            abi::EM_AARCH64 => (0x0002_0000_0000, 0x0005_0000_0000),
            machine => panic!("unsupported preload machine: {machine}"),
        };
        let segments: Vec<_> = image.segments().unwrap().iter().collect();
        assert!(
            !segments
                .iter()
                .any(|ph| matches!(ph.p_type, abi::PT_INTERP | abi::PT_DYNAMIC | abi::PT_TLS))
        );
        let first_load = segments.iter().find(|ph| ph.p_type == abi::PT_LOAD).unwrap();
        assert_eq!((first_load.p_vaddr, first_load.p_offset), (base, 0));
        for segment in segments.iter().filter(|ph| ph.p_type == abi::PT_LOAD) {
            assert!(segment.p_vaddr >= base);
            assert!(segment.p_vaddr.checked_add(segment.p_memsz).unwrap() <= limit);
        }
        assert!(segments.iter().any(|ph| {
            ph.p_type == abi::PT_LOAD
                && ph.p_flags & abi::PF_X != 0
                && ph.p_vaddr <= image.ehdr.e_entry
                && image.ehdr.e_entry < ph.p_vaddr + ph.p_memsz
        }));
        assert!(image.dynamic().unwrap().is_none());
    }
}
