use std::{
    io,
    mem::offset_of,
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
    path::Path,
    sync::RwLock,
};

use fspy_shared::ipc::{IpcStr, PathAccess, channel::channel};
use fspy_shared_unix::linux::{BOOTSTRAP_ENV_NAME, Bootstrap, Target, encode_bootstrap};
use futures_util::FutureExt as _;
use linux_raw_sys::{
    prctl::PR_SET_NO_NEW_PRIVS,
    ptrace::{
        BPF_ABS, BPF_JEQ, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W, SECCOMP_RET_ALLOW,
        SECCOMP_RET_TRAP, SECCOMP_SET_MODE_FILTER, seccomp_data, sock_filter, sock_fprog,
    },
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
    /// Materialized executable path to `fspy_preload_linux`, or `None` when
    /// seccomp is unavailable or materialization failed (in which case `spawn`
    /// falls back to untracked execution). Wrapped in `RwLock` so a file reaped
    /// from the temp directory (e.g. by `systemd-tmpfiles`) can be lazily
    /// re-materialized on `ENOENT`.
    preload_path: RwLock<Option<Box<IpcStr>>>,
    seccomp_available: bool,
}

impl SpyImpl {
    /// Materializes `fspy_preload_linux` into the user's temp directory and
    /// probes whether this environment permits installing a seccomp filter.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "matches cross-platform SpyImpl::init() -> io::Result<Self> signature"
    )]
    pub fn init() -> io::Result<Self> {
        let seccomp_available = probe_seccomp_available();
        let preload_path = if seccomp_available {
            let materialized = materialize_preload();
            if materialized.is_none() {
                tracing::warn!(
                    "failed to materialize the Linux task tracker binary, so Linux task \
                     tracking is off and caching is disabled"
                );
            }
            materialized
        } else {
            None
        };
        Ok(Self { preload_path: RwLock::new(preload_path), seccomp_available })
    }

    fn preload_path(&self) -> Option<Box<IpcStr>> {
        if !self.seccomp_available {
            return None;
        }
        self.preload_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            .map(clone_ipc_str)
    }

    fn rematerialize_preload(&self, failed_path: &IpcStr) -> Option<Box<IpcStr>> {
        let mut guard =
            self.preload_path.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = guard.as_deref()
            && (current.as_bytes() != failed_path.as_bytes()
                || Path::new(current.as_os_str()).exists())
        {
            return Some(clone_ipc_str(current));
        }
        let new_path = materialize_preload();
        *guard = new_path.as_deref().map(clone_ipc_str);
        new_path
    }

    pub(crate) async fn spawn(
        &self,
        mut command: Command,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        let Some(mut preload_path) = self.preload_path() else {
            return spawn_untracked(command, cancellation_token);
        };
        // If the cached launcher binary was removed mid-process (e.g. by
        // `systemd-tmpfiles`), re-materialize before building `tokio_command`
        // so both `Bootstrap` and `redirect_program` pick up any new fallback
        // directory path.
        if !Path::new(preload_path.as_os_str()).exists() {
            match self.rematerialize_preload(&preload_path) {
                Some(new_path) => preload_path = new_path,
                None => return spawn_untracked(command, cancellation_token),
            }
        }

        let ipc_receiver = channel(crate::ipc::shm_capacity(), allocator_api2::alloc::Global)
            .map_err(SpawnError::ChannelCreation)?;

        let (read_end, write_end) = create_error_pipe().map_err(SpawnError::OsSpawn)?;
        let pipe_reader = tokio::net::unix::pipe::Receiver::from_owned_fd(read_end)
            .map_err(SpawnError::OsSpawn)?;
        let write_raw_fd = write_end.as_raw_fd();

        let target_path: &IpcStr = command.target_program().into();
        let bootstrap = Bootstrap {
            ipc_channel_conf: ipc_receiver.conf(),
            preload_path: &preload_path,
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

        let mut child = match tokio_command.spawn() {
            Ok(child) => child,
            Err(err)
                if err.kind() == io::ErrorKind::NotFound
                    && !Path::new(preload_path.as_os_str()).exists() =>
            {
                match self.rematerialize_preload(&preload_path) {
                    Some(new_path) if new_path.as_bytes() == preload_path.as_bytes() => {
                        tokio_command.spawn().map_err(SpawnError::OsSpawn)?
                    }
                    _ => {
                        return Err(SpawnError::OsSpawn(io::Error::other(format!(
                            "fspy launcher vanished during spawn and could not be rematerialized at {}: {err}",
                            preload_path.as_os_str().to_string_lossy()
                        ))));
                    }
                }
            }
            Err(err) => return Err(SpawnError::OsSpawn(err)),
        };
        // Close the parent's copy of the write end immediately so EOF fires if
        // `fspy_preload_linux` terminates before writing a status code.
        drop(write_end);

        await_startup_handshake(pipe_reader, &mut child, &cancellation_token).await?;

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

async fn await_startup_handshake(
    mut pipe_reader: tokio::net::unix::pipe::Receiver,
    child: &mut tokio::process::Child,
    cancellation_token: &CancellationToken,
) -> Result<(), SpawnError> {
    let mut buf = [0u8; 4];
    let mut filled = 0usize;
    while filled < buf.len() {
        tokio::select! {
            read_res = pipe_reader.read(&mut buf[filled..]) => match read_res {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    drop(pipe_reader);
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(SpawnError::OsSpawn(err));
                }
            },
            () = cancellation_token.cancelled() => {
                drop(pipe_reader);
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(SpawnError::OsSpawn(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "spawn cancelled",
                )));
            }
        }
    }
    drop(pipe_reader);

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
    Ok(())
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

fn clone_ipc_str(s: &IpcStr) -> Box<IpcStr> {
    Path::new(s.as_os_str()).into()
}

fn materialize_preload() -> Option<Box<IpcStr>> {
    PRELOAD_LINUX
        .materialize()
        .executable()
        .at_user_temp_dir("vp_run")
        .ok()
        .map(|p| p.as_path().into())
}

/// Probes once on a dedicated short-lived OS thread whether
/// `PR_SET_NO_NEW_PRIVS` and `seccomp(SECCOMP_SET_MODE_FILTER)` with
/// `SECCOMP_RET_TRAP` are permitted in the current environment.
///
/// Both `PR_SET_NO_NEW_PRIVS` and `SECCOMP_SET_MODE_FILTER` (without
/// `SECCOMP_FILTER_FLAG_TSYNC`) are per-thread in Linux, so running the probe
/// on a dedicated thread avoids `fork()` in multi-threaded processes while
/// leaving every other thread completely unaffected.
fn probe_seccomp_available() -> bool {
    const SENTINEL_SYSNO: u32 = 0x7fff_ffff;

    let Ok(handle) = std::thread::Builder::new().name("fspy-seccomp-probe".into()).spawn(|| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "cBPF opcodes and seccomp_data offsets fit in u16/u32"
        )]
        let filter = [
            sock_filter {
                code: (BPF_LD | BPF_W | BPF_ABS) as u16,
                jt: 0,
                jf: 0,
                k: offset_of!(seccomp_data, nr) as u32,
            },
            sock_filter {
                code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
                jt: 0,
                jf: 1,
                k: SENTINEL_SYSNO,
            },
            sock_filter { code: (BPF_RET | BPF_K) as u16, jt: 0, jf: 0, k: SECCOMP_RET_TRAP },
            sock_filter { code: (BPF_RET | BPF_K) as u16, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW },
        ];
        let prog = sock_fprog { len: 4, filter: filter.as_ptr().cast_mut() };
        // SAFETY: installs a per-thread seccomp filter on this short-lived
        // probe thread using a valid stack-local cBPF program.
        unsafe {
            syscalls::syscall!(Sysno::prctl, PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0).is_ok()
                && syscalls::syscall!(Sysno::seccomp, SECCOMP_SET_MODE_FILTER, 0, &raw const prog)
                    .is_ok()
        }
    }) else {
        return false;
    };
    handle.join().unwrap_or(false)
}

/// Creates a `CLOEXEC` pipe whose read end is non-blocking (for tokio's async
/// pipe receiver) and whose write end stays blocking and at FD `>= 3` (so child
/// stdio `dup2` onto FD 0/1/2 never collides with `error_pipe_fd`).
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

    let write_end = if write_end.as_raw_fd() < 3 {
        // SAFETY: `write_end` is a valid open descriptor; duplicating to `>= 3`
        // prevents child stdio `dup2` from clobbering `error_pipe_fd`.
        let dup = unsafe { libc::fcntl(write_end.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if dup == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `F_DUPFD_CLOEXEC` returned a newly opened descriptor.
        unsafe { OwnedFd::from_raw_fd(dup) }
    } else {
        write_end
    };

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
            abi::EM_X86_64 => fspy_elf_loader::LAUNCHER_REGION_X86_64,
            abi::EM_AARCH64 => fspy_elf_loader::LAUNCHER_REGION_AARCH64,
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
