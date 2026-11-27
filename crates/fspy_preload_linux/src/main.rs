#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(not(target_os = "none"))]
const fn main() {}

#[cfg(target_os = "none")]
mod runtime {
    use core::{
        alloc::Layout,
        cell::UnsafeCell,
        ffi::c_void,
        mem::{MaybeUninit, size_of},
        ptr::{self, NonNull},
        slice,
        sync::atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering},
    };

    use allocator_api2::{alloc::Allocator, vec::Vec};
    use bstr::BStr;
    use fspy_elf_loader::{Syscall, SyscallAction, SyscallInterceptor};
    use fspy_nostd::{
        BorrowedFd, CStr, CWD, Error, OwnedFd, RawFd, Result, Thin,
        env::InitialStack,
        fs::{Mode, OFlags, PATH_MAX, fcntl_dupfd, openat},
    };
    use fspy_nostd_alloc::{PageAllocator, PageBump, fs::fd_path, page_bump, pooled_bump};
    use fspy_shared::ipc::{
        AccessMode, IpcPath, IpcStr,
        channel::{ChannelConf, Sender},
    };
    use fspy_shared_unix::{
        Env, Exec, RawExec,
        linux::{
            BOOTSTRAP_ENV_NAME, Bootstrap, ExecAtFlags, Target, encode_bootstrap,
            ensure_bootstrap_env, resolve_exec,
        },
    };
    use linux_raw_sys::{
        errno::{E2BIG, ENOSYS, EPERM, ESRCH},
        general::{AT_EMPTY_PATH, O_ACCMODE, O_RDWR, O_WRONLY, SIG_SETMASK, open_how},
    };
    use syscalls::Sysno;

    fspy_nostd::main!(run);
    fspy_nostd::no_global_alloc!();

    /// Linux 6.14+ `execveat(2)` flag that checks executability without replacing
    /// the process image.
    const AT_EXECVE_CHECK: u32 = 0x10000;

    /// Linux `kcmp(2)` resource type comparing virtual memory (`mm_struct`).
    const KCMP_VM: usize = 1;

    /// Upper bound on a single `argv`/`envp` string copied from user memory
    /// (matches Linux `MAX_ARG_STRLEN` up to 64 KiB pages).
    const MAX_EXEC_STRING_LEN: usize = 2 * 1024 * 1024;

    /// Number of reusable exec scratch slots for reclaiming `CLONE_VM`
    /// (`vfork` / `posix_spawn`) exec buffers once the child detaches.
    const MAX_EXEC_SLOTS: usize = 64;
    const SLOT_FREE: u64 = 0;
    const SLOT_BUSY: u64 = 1;

    struct SyncCell<T>(UnsafeCell<T>);

    // SAFETY: `STARTUP_BUMP` and `RUNTIME` are initialized once in the fresh,
    // single-threaded launcher process before `exec_elf` arms seccomp and enters
    // the target; after that `RUNTIME` is strictly read-only.
    unsafe impl<T> Sync for SyncCell<T> {}

    struct Runtime {
        ipc_channel_conf: ChannelConf<'static>,
        preload_path: &'static IpcStr,
        sender: Option<Sender>,
    }

    struct ExecSlot {
        /// `0` = free, `1` = exclusively owned in-flight/reclaiming,
        /// `(gen << 32) | child_pid` (`gen >= 1`) = parked by a `CLONE_VM`
        /// child calling `execve`.
        state: AtomicU64,
        /// Registered with `set_tid_address` right before `SYS_execve` in a
        /// `CLONE_VM` child: the Linux kernel's `mm_release()` atomically writes
        /// `0` here the instant `execve` detaches the child's `mm_struct` (or
        /// if the child dies), allowing zero-syscall reclamation even when
        /// `kcmp` is denied by Docker/seccomp and without PID-reuse hazards.
        active_tid: AtomicI32,
        arena: UnsafeCell<Option<PageBump>>,
        duped_fd: UnsafeCell<RawFd>,
    }

    // SAFETY: Access to `arena` and `duped_fd` is serialized via `state` CAS
    // transitions (`SLOT_FREE -> SLOT_BUSY` and `parked(gen, pid) -> SLOT_BUSY`).
    unsafe impl Sync for ExecSlot {}

    impl ExecSlot {
        const fn new() -> Self {
            Self {
                state: AtomicU64::new(SLOT_FREE),
                active_tid: AtomicI32::new(0),
                arena: UnsafeCell::new(None),
                duped_fd: UnsafeCell::new(-1),
            }
        }
    }

    /// Overflow scratch node used when all [`MAX_EXEC_SLOTS`] fixed slots are
    /// simultaneously busy. For `CLONE_VM` children, pushed onto
    /// [`OVERFLOW_PARKED_HEAD`] and reclaimed once `active_tid` becomes `0`
    /// (written by the kernel's `mm_release()` on exec/exit, or by failure
    /// cleanup) or `kcmp` reports VM detachment.
    struct OverflowScratch {
        next: AtomicPtr<Self>,
        active_tid: AtomicI32,
        child_pid: i32,
        parked: bool,
        arena: UnsafeCell<Option<PageBump>>,
        duped_fd: UnsafeCell<RawFd>,
    }

    // SAFETY: `arena` and `duped_fd` are mutated exclusively by the owning
    // thread before `active_tid` transitions to `0` (`Release`), and only
    // dropped by a reclaimer that observes `active_tid == 0` (`Acquire`) or
    // kernel VM detachment after detaching the node from `OVERFLOW_PARKED_HEAD`.
    unsafe impl Sync for OverflowScratch {}

    static STARTUP_BUMP: SyncCell<PageBump> = SyncCell(UnsafeCell::new(page_bump()));
    static RUNTIME: SyncCell<Option<Runtime>> = SyncCell(UnsafeCell::new(None));
    static EXEC_SLOTS: [ExecSlot; MAX_EXEC_SLOTS] = [const { ExecSlot::new() }; MAX_EXEC_SLOTS];
    static PARK_GEN: AtomicU64 = AtomicU64::new(1);
    static OVERFLOW_PARKED_HEAD: AtomicPtr<OverflowScratch> = AtomicPtr::new(ptr::null_mut());

    #[cfg(target_arch = "x86_64")]
    const TRACKED_SYSCALLS: &[Sysno] = &[
        Sysno::open,
        Sysno::openat,
        Sysno::openat2,
        Sysno::getdents,
        Sysno::getdents64,
        Sysno::stat,
        Sysno::lstat,
        Sysno::newfstatat,
        Sysno::statx,
        Sysno::access,
        Sysno::faccessat,
        Sysno::faccessat2,
        Sysno::mkdir,
        Sysno::mkdirat,
        Sysno::rename,
        Sysno::renameat,
        Sysno::renameat2,
        Sysno::link,
        Sysno::linkat,
        Sysno::symlink,
        Sysno::symlinkat,
        Sysno::execve,
        Sysno::execveat,
    ];

    #[cfg(target_arch = "aarch64")]
    const TRACKED_SYSCALLS: &[Sysno] = &[
        Sysno::openat,
        Sysno::openat2,
        Sysno::getdents64,
        Sysno::fstatat,
        Sysno::statx,
        Sysno::faccessat,
        Sysno::faccessat2,
        Sysno::mkdirat,
        Sysno::renameat,
        Sysno::renameat2,
        Sysno::linkat,
        Sysno::symlinkat,
        Sysno::execve,
        Sysno::execveat,
    ];

    static INTERCEPTOR: SyscallInterceptor =
        SyscallInterceptor { syscalls: TRACKED_SYSCALLS, handle: handle_syscall };

    #[inline]
    fn runtime() -> &'static Runtime {
        // SAFETY: `RUNTIME` is initialized once during single-threaded startup
        // before any `SIGSYS` handler or helper calls `runtime()`.
        unsafe { (*RUNTIME.0.get()).as_ref() }.expect("runtime not initialized")
    }

    #[expect(clippy::needless_pass_by_value, reason = "signature required by fspy_nostd::main!")]
    fn run(stack: InitialStack<'_>) -> ! {
        // SAFETY: `run` executes once in a fresh single-threaded process.
        let bump: &'static PageBump = unsafe { &*STARTUP_BUMP.0.get() };
        let bootstrap = Bootstrap::from_env(stack.envs(), bump);
        let sender = bootstrap.ipc_channel_conf.sender(bump);

        // SAFETY: single-threaded startup before arming seccomp.
        unsafe {
            *RUNTIME.0.get() = Some(Runtime {
                ipc_channel_conf: bootstrap.ipc_channel_conf,
                preload_path: bootstrap.preload_path,
                sender,
            });
        }

        let filtered_envs = stack.envs().iter().copied().filter(|&entry| !is_bootstrap_env(entry));

        match bootstrap.target {
            Target::Unresolved { path, error_pipe_fd } => {
                let mut exec_args = Vec::with_capacity_in(stack.args().len(), bump);
                for arg in stack.args() {
                    exec_args.push(copy_bytes(arg.count().as_units(), bump));
                }
                let mut exec = Exec {
                    program: copy_bytes(path.as_bytes(), bump),
                    args: exec_args,
                    envs: Vec::new_in(bump),
                };

                let resolved = resolve_exec(&mut exec, Some(CWD), ExecAtFlags::empty(), |p| {
                    send_path(AccessMode::Read, p);
                })
                .and_then(|resolved| {
                    fspy_elf_loader::validate_exec(resolved.file.as_fd())?;
                    Ok(resolved)
                });

                let resolved = match resolved {
                    Ok(resolved) => resolved,
                    Err(err) => {
                        write_error_pipe(error_pipe_fd, err.raw_os_error());
                        exit_group(127);
                    }
                };

                write_error_pipe(error_pipe_fd, 0);
                // Close the error pipe before entering the target so it never
                // leaks an open descriptor into the child process.
                if error_pipe_fd >= 0 {
                    // SAFETY: the parent opened `error_pipe_fd` exclusively for
                    // this startup handshake and transferred ownership to us.
                    drop(unsafe { OwnedFd::from_raw_fd(error_pipe_fd) });
                }

                let execfn = leak_cstr(&resolved.execfn, bump);
                let mut argv = Vec::with_capacity_in(exec.args.len(), bump);
                for arg in &exec.args {
                    argv.push(leak_cstr(arg, bump));
                }

                // SAFETY: fresh single-threaded freestanding launcher; `exec_elf`
                // copies strings onto the new stack before abandoning ours.
                unsafe {
                    fspy_elf_loader::exec_elf(
                        resolved.file,
                        execfn,
                        argv.into_iter(),
                        filtered_envs,
                        stack.auxv(),
                        Some(INTERCEPTOR),
                    )
                }
            }
            Target::ResolvedPath { path, execfn, sigmask } => {
                let path_cstr =
                    path.to_os_c_string_in(bump).expect("resolved target path contains NUL");
                let execfn_cstr =
                    execfn.to_os_c_string_in(bump).expect("resolved execfn contains NUL");
                let file = openat(
                    CWD,
                    path_cstr.as_c_str().as_thin(),
                    OFlags::RDONLY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .expect("cannot open resolved target");

                restore_sigmask(sigmask);

                // SAFETY: fresh single-threaded freestanding launcher; `exec_elf`
                // copies strings onto the new stack before abandoning ours.
                unsafe {
                    fspy_elf_loader::exec_elf(
                        file,
                        execfn_cstr.as_c_str().as_thin(),
                        stack.args().iter().copied(),
                        filtered_envs,
                        stack.auxv(),
                        Some(INTERCEPTOR),
                    )
                }
            }
            Target::ResolvedFd { fd, execfn, sigmask } => {
                let execfn_cstr =
                    execfn.to_os_c_string_in(bump).expect("resolved execfn contains NUL");
                // SAFETY: parent duped `fd` specifically for this handoff.
                let file = unsafe { OwnedFd::from_raw_fd(fd) };

                restore_sigmask(sigmask);

                // SAFETY: fresh single-threaded freestanding launcher; `exec_elf`
                // copies strings onto the new stack before abandoning ours.
                unsafe {
                    fspy_elf_loader::exec_elf(
                        file,
                        execfn_cstr.as_c_str().as_thin(),
                        stack.args().iter().copied(),
                        filtered_envs,
                        stack.auxv(),
                        Some(INTERCEPTOR),
                    )
                }
            }
        }
    }

    fn is_bootstrap_env(entry: CStr<'_, Thin>) -> bool {
        let bytes = entry.count().as_units();
        bytes
            .strip_prefix(BOOTSTRAP_ENV_NAME.as_bytes())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"="))
    }

    fn copy_bytes<A: Allocator>(bytes: &[u8], allocator: A) -> Vec<u8, A> {
        let mut out = Vec::with_capacity_in(bytes.len(), allocator);
        out.extend_from_slice(bytes);
        out
    }

    fn leak_cstr<'a, A: Allocator + 'a>(bytes: &[u8], allocator: A) -> CStr<'a, Thin> {
        let mut owned = Vec::with_capacity_in(bytes.len() + 1, allocator);
        owned.extend_from_slice(bytes);
        owned.push(0);
        let leaked: &'a [u8] = owned.leak();
        // SAFETY: `leaked` ends with a NUL byte and lives for `'a`.
        unsafe { CStr::<Thin>::from_ptr(leaked.as_ptr()) }
    }

    fn write_error_pipe(fd: RawFd, code: i32) {
        if fd < 0 {
            return;
        }
        // SAFETY: `fd` is the parent-provided non-negative pipe descriptor.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let bytes = code.to_ne_bytes();
        let mut written = 0usize;
        while written < bytes.len() {
            match fspy_nostd::io::write(borrowed, &bytes[written..]) {
                Ok(0) => break,
                Ok(n) => written += n,
                Err(Error::INTR) => {}
                Err(_) => break,
            }
        }
    }

    fn restore_sigmask(sigmask: u64) {
        // SAFETY: `sigmask` is a valid stack-local 64-bit Linux `sigset_t`.
        // Executing from inside the launcher's `PF_X` allowlist bypasses the
        // inherited seccomp filter, and `exec_elf`'s `sigsys::install` will
        // subsequently record and unblock `SIGSYS` if it was set in `sigmask`.
        let _ = unsafe {
            syscalls::syscall!(
                Sysno::rt_sigprocmask,
                SIG_SETMASK,
                &raw const sigmask,
                0usize,
                size_of::<u64>()
            )
        };
    }

    fn exit_group(code: i32) -> ! {
        // SAFETY: `exit_group` terminates the process immediately and never returns.
        let _ = unsafe { syscalls::syscall!(Sysno::exit_group, code) };
        loop {
            core::hint::spin_loop();
        }
    }

    #[expect(clippy::cast_possible_truncation, reason = "Linux pid_t is i32")]
    fn getpid() -> i32 {
        // SAFETY: `getpid` takes no arguments and cannot fail.
        unsafe { syscalls::raw::syscall0(Sysno::getpid as usize).cast_signed() as i32 }
    }

    fn send_path(mode: AccessMode, path: &BStr) {
        let bytes: &[u8] = path.as_ref();
        if bytes.starts_with(b"/dev/")
            || bytes.starts_with(b"/proc/")
            || bytes.starts_with(b"/sys/")
        {
            return;
        }
        if let Some(sender) = runtime().sender.as_ref() {
            sender.send(&mode.with_path(IpcPath::from_bytes(bytes)));
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Linux file descriptors are 32-bit signed integers"
    )]
    const fn arg_i32(raw: usize) -> i32 {
        raw as i32
    }

    #[expect(clippy::cast_possible_truncation, reason = "Linux flags live in low 32 bits")]
    const fn arg_u32(raw: usize) -> u32 {
        raw as u32
    }

    const fn open_mode(flags: u32) -> AccessMode {
        match flags & O_ACCMODE {
            O_RDWR => AccessMode::ReadWrite,
            O_WRONLY => AccessMode::Write,
            _ => AccessMode::Read,
        }
    }

    /// Reports a path-based syscall to the IPC channel. `path_ptr` is the
    /// trapped syscall's own argument, which `fspy_elf_loader` assumes valid.
    fn report_path_at(
        call: &Syscall<'_>,
        dirfd: Option<BorrowedFd<'_>>,
        path_ptr: usize,
        mode: AccessMode,
        allow_empty_path: bool,
    ) {
        if path_ptr == 0 {
            if allow_empty_path && let Some(fd) = dirfd {
                report_fd(fd, mode);
            }
            return;
        }
        let mut buf = [MaybeUninit::<u8>::uninit(); PATH_MAX];
        let raw: &[u8] = if let Some(cached) = call.cached_cstr(path_ptr) {
            cached
        } else {
            // SAFETY: `path_ptr` is the trapped syscall's path argument.
            let Ok(Some(bytes)) = (unsafe { fspy_elf_loader::copy_user_cstr(path_ptr, &mut buf) })
            else {
                return;
            };
            bytes
        };
        if raw.is_empty() {
            if allow_empty_path && let Some(fd) = dirfd {
                report_fd(fd, mode);
            }
            return;
        }
        if raw.starts_with(b"/") {
            send_path(mode, BStr::new(raw));
            return;
        }
        let Some(dirfd) = dirfd else {
            return;
        };
        let arena = pooled_bump();
        let Ok(Some(base)) = fd_path(&arena, dirfd) else {
            return;
        };
        let base_bytes = base.as_units();
        let mut joined = Vec::new_in(&arena);
        if joined.try_reserve_exact(base_bytes.len() + 1 + raw.len()).is_err() {
            return;
        }
        joined.extend_from_slice(base_bytes);
        if !base_bytes.ends_with(b"/") {
            joined.push(b'/');
        }
        joined.extend_from_slice(raw);
        send_path(mode, BStr::new(&joined));
    }

    fn report_fd(fd: BorrowedFd<'_>, mode: AccessMode) {
        let arena = pooled_bump();
        if let Ok(Some(resolved)) = fd_path(&arena, fd) {
            send_path(mode, BStr::new(resolved.as_units()));
        }
    }

    /// `SIGSYS` interceptor callback invoked by `fspy_elf_loader`.
    ///
    /// # Safety
    ///
    /// `call` holds the live (and `/proc/*/exe`-rewritten) arguments of a
    /// trapped syscall listed in [`TRACKED_SYSCALLS`].
    unsafe fn handle_syscall(call: &Syscall<'_>) -> Result<SyscallAction> {
        // SAFETY: `dirfd` arguments come directly from the trapped syscall; `-1`
        // maps to `None` and invalid fds return `Ok(None)` from `fd_path`.
        let borrow_dirfd = |raw: usize| unsafe { BorrowedFd::try_borrow_raw(arg_i32(raw)) };

        match call.sysno {
            Sysno::execve => handle_exec(
                Some(CWD),
                call.args[0],
                call.args[1],
                call.args[2],
                ExecAtFlags::empty(),
                call.sigmask(),
                call.is_shared_vm_child(),
            ),
            Sysno::execveat => {
                let raw_flags = arg_u32(call.args[4]);
                // Linux 6.14+ `AT_EXECVE_CHECK` validates executability without
                // replacing the process image; let the kernel answer directly.
                if (raw_flags & AT_EXECVE_CHECK) != 0 {
                    return Ok(SyscallAction::Replay);
                }
                let Some(flags) = ExecAtFlags::from_bits(raw_flags) else {
                    return Err(Error::INVAL);
                };
                handle_exec(
                    borrow_dirfd(call.args[0]),
                    call.args[1],
                    call.args[2],
                    call.args[3],
                    flags,
                    call.sigmask(),
                    call.is_shared_vm_child(),
                )
            }
            _ => {
                handle_fs_syscall(call);
                Ok(SyscallAction::Replay)
            }
        }
    }

    /// Reports a tracked filesystem syscall to the IPC channel.
    fn handle_fs_syscall(call: &Syscall<'_>) {
        // SAFETY: `dirfd` arguments come directly from the trapped syscall; `-1`
        // maps to `None` and invalid fds return `Ok(None)` from `fd_path`.
        let dirfd = |idx: usize| unsafe { BorrowedFd::try_borrow_raw(arg_i32(call.args[idx])) };
        let a = call.args;

        match call.sysno {
            #[cfg(target_arch = "x86_64")]
            Sysno::open => report_path_at(call, Some(CWD), a[0], open_mode(arg_u32(a[1])), false),
            Sysno::openat => report_path_at(call, dirfd(0), a[1], open_mode(arg_u32(a[2])), false),
            Sysno::openat2 => report_openat2(call, dirfd(0), a[1], a[2], a[3]),
            #[cfg(target_arch = "x86_64")]
            Sysno::getdents => {
                if let Some(fd) = dirfd(0) {
                    report_fd(fd, AccessMode::ReadDir);
                }
            }
            Sysno::getdents64 => {
                if let Some(fd) = dirfd(0) {
                    report_fd(fd, AccessMode::ReadDir);
                }
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::stat | Sysno::lstat | Sysno::access => {
                report_path_at(call, Some(CWD), a[0], AccessMode::Read, false);
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::newfstatat => {
                let empty_ok = (arg_u32(a[3]) & AT_EMPTY_PATH) != 0;
                report_path_at(call, dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            #[cfg(target_arch = "aarch64")]
            Sysno::fstatat => {
                let empty_ok = (arg_u32(a[3]) & AT_EMPTY_PATH) != 0;
                report_path_at(call, dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            Sysno::statx => {
                let empty_ok = (arg_u32(a[2]) & AT_EMPTY_PATH) != 0;
                report_path_at(call, dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            Sysno::faccessat => {
                report_path_at(call, dirfd(0), a[1], AccessMode::Read, false);
            }
            Sysno::faccessat2 => {
                let empty_ok = (arg_u32(a[3]) & AT_EMPTY_PATH) != 0;
                report_path_at(call, dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::mkdir => report_path_at(call, Some(CWD), a[0], AccessMode::Mkdir, false),
            Sysno::mkdirat => report_path_at(call, dirfd(0), a[1], AccessMode::Mkdir, false),
            #[cfg(target_arch = "x86_64")]
            Sysno::rename => report_path_at(call, Some(CWD), a[1], AccessMode::RenameTarget, false),
            Sysno::renameat | Sysno::renameat2 => {
                report_path_at(call, dirfd(2), a[3], AccessMode::RenameTarget, false);
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::link | Sysno::symlink => {
                report_path_at(call, Some(CWD), a[1], AccessMode::Write, false);
            }
            Sysno::linkat => report_path_at(call, dirfd(2), a[3], AccessMode::Write, false),
            Sysno::symlinkat => report_path_at(call, dirfd(1), a[2], AccessMode::Write, false),
            _ => {}
        }
    }

    fn report_openat2(
        call: &Syscall<'_>,
        dirfd: Option<BorrowedFd<'_>>,
        path_ptr: usize,
        how_ptr: usize,
        size: usize,
    ) {
        if how_ptr == 0 || size < size_of::<open_how>() {
            return;
        }
        // SAFETY: `open_how` is a `#[repr(C)]` struct of `u64` fields valid for
        // any bit pattern, and `how_ptr` is the trapped syscall's argument.
        let Ok(how) = (unsafe { fspy_elf_loader::copy_user_value::<open_how>(how_ptr) }) else {
            return;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "O_ACCMODE bits live in low 32 bits of open_how.flags"
        )]
        let mode = open_mode(how.flags as u32);
        report_path_at(call, dirfd, path_ptr, mode, false);
    }

    /// Copies an arbitrarily long NUL-terminated string from a trapped
    /// syscall's argument into `allocator`.
    fn copy_user_cstr_vec<A: Allocator>(addr: usize, allocator: A) -> Result<Vec<u8, A>> {
        if addr == 0 {
            return Err(Error::FAULT);
        }
        let mut out = Vec::new_in(allocator);
        let mut chunk = [MaybeUninit::<u8>::uninit(); 4096];
        let mut offset = 0usize;
        loop {
            let cur = addr.checked_add(offset).ok_or(Error::FAULT)?;
            // SAFETY: `addr` is a string pointer from the trapped syscall, and
            // `cur` stays before its NUL.
            if let Some(bytes) = unsafe { fspy_elf_loader::copy_user_cstr(cur, &mut chunk) }? {
                out.try_reserve_exact(bytes.len()).map_err(|_| Error::NOMEM)?;
                out.extend_from_slice(bytes);
                return Ok(out);
            }
            // SAFETY: `copy_user_cstr` initialized all `chunk.len()` bytes on `Ok(None)`.
            let filled = unsafe { slice::from_raw_parts(chunk.as_ptr().cast::<u8>(), chunk.len()) };
            out.try_reserve(filled.len()).map_err(|_| Error::NOMEM)?;
            out.extend_from_slice(filled);
            offset = offset.checked_add(chunk.len()).ok_or(Error::FAULT)?;
            if offset > MAX_EXEC_STRING_LEN {
                return Err(Error::from_raw_os_error(E2BIG.cast_signed()));
            }
        }
    }

    fn copy_user_cstr_array<T, A: Allocator + Clone>(
        arr_ptr: usize,
        allocator: A,
        mut map_fn: impl FnMut(Vec<u8, A>) -> T,
    ) -> Result<Vec<T, A>> {
        if arr_ptr == 0 {
            return Ok(Vec::new_in(allocator));
        }
        let mut out = Vec::new_in(allocator.clone());
        let mut idx = 0usize;
        loop {
            let byte_off = idx.checked_mul(size_of::<usize>()).ok_or(Error::FAULT)?;
            let elem_addr = arr_ptr.checked_add(byte_off).ok_or(Error::FAULT)?;
            // SAFETY: `usize` is valid for any bit pattern, and `elem_addr` is
            // inside the trapped syscall's NULL-terminated pointer array.
            let str_ptr = unsafe { fspy_elf_loader::copy_user_value::<usize>(elem_addr) }?;
            if str_ptr == 0 {
                return Ok(out);
            }
            let bytes = copy_user_cstr_vec(str_ptr, allocator.clone())?;
            out.try_reserve(1).map_err(|_| Error::NOMEM)?;
            out.push(map_fn(bytes));
            idx += 1;
        }
    }

    fn copy_user_exec<A: Allocator + Clone>(
        prog_ptr: usize,
        argv_ptr: usize,
        envp_ptr: usize,
        allocator: A,
    ) -> Result<Exec<A>> {
        let program = copy_user_cstr_vec(prog_ptr, allocator.clone())?;
        let args = copy_user_cstr_array(argv_ptr, allocator.clone(), |arg| arg)?;
        let envs: Vec<Env<A>, A> = copy_user_cstr_array(envp_ptr, allocator.clone(), |mut env| {
            if let Some(eq_pos) = env.iter().position(|&b| b == b'=') {
                let value = copy_bytes(&env[eq_pos + 1..], allocator.clone());
                env.truncate(eq_pos);
                (env, Some(value))
            } else {
                (env, None)
            }
        })?;
        Ok(Exec { program, args, envs })
    }

    #[inline]
    fn park_state(pid: i32) -> u64 {
        loop {
            let generation = PARK_GEN.fetch_add(1, Ordering::Relaxed) & 0xffff_ffff;
            if generation != 0 {
                return (generation << 32) | u64::from(pid.cast_unsigned());
            }
        }
    }

    #[inline]
    #[expect(clippy::cast_possible_truncation, reason = "Linux pid_t fits in lower 32 bits")]
    const fn parked_pid(state: u64) -> i32 {
        (state as u32).cast_signed()
    }

    #[inline]
    fn set_tid_address(addr: *const AtomicI32) {
        // SAFETY: `addr` is either null or points to a live `AtomicI32` that
        // remains mapped while the calling `CLONE_VM` child is attached to the
        // current `mm_struct`.
        let _ = unsafe { syscalls::raw::syscall1(Sysno::set_tid_address as usize, addr.addr()) };
    }

    /// Returns `true` if `child_pid` no longer shares our virtual memory
    /// (`mm_struct`) or has exited, so its parked `CLONE_VM` exec scratch
    /// mapping is guaranteed unreachable and safe to `munmap`.
    fn is_vm_detached(my_pid: i32, child_pid: i32) -> bool {
        if child_pid <= 0 || child_pid == my_pid {
            return false;
        }
        // SAFETY: `kcmp(my_pid, child_pid, KCMP_VM, 0, 0)` compares the two
        // processes' `mm_struct` pointers (`0` = same VM, `> 0` = different VM).
        let rc = unsafe {
            syscalls::raw::syscall5(
                Sysno::kcmp as usize,
                my_pid.cast_unsigned() as usize,
                child_pid.cast_unsigned() as usize,
                KCMP_VM,
                0,
                0,
            )
            .cast_signed()
        };
        if rc == 0 {
            return false;
        }
        if rc > 0 {
            return true;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Linux negative syscall errno fits in u32"
        )]
        let errno = (-rc).cast_unsigned() as u32;
        if errno == ESRCH {
            return true;
        }
        if errno == EPERM || errno == ENOSYS {
            // Fallback if `kcmp` is denied and `clear_child_tid` was not yet
            // cleared (e.g. child killed before `set_tid_address`): probe liveness.
            // SAFETY: `kill(child_pid, 0)` probes liveness without sending a signal.
            let krc = unsafe {
                syscalls::raw::syscall2(Sysno::kill as usize, child_pid.cast_unsigned() as usize, 0)
                    .cast_signed()
            };
            return krc == -(ESRCH.cast_signed() as isize);
        }
        false
    }

    unsafe fn push_overflow_parked(node: *mut OverflowScratch) {
        let mut head = OVERFLOW_PARKED_HEAD.load(Ordering::Acquire);
        loop {
            // SAFETY: Caller owns `node` exclusively until CAS publishes it.
            unsafe { (*node).next.store(head, Ordering::Relaxed) };
            match OVERFLOW_PARKED_HEAD.compare_exchange_weak(
                head,
                node,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => head = actual,
            }
        }
    }

    fn reclaim_overflow_scratch(my_pid: &mut i32) {
        if OVERFLOW_PARKED_HEAD.load(Ordering::Acquire).is_null() {
            return;
        }
        let mut cur = OVERFLOW_PARKED_HEAD.swap(ptr::null_mut(), Ordering::AcqRel);
        while !cur.is_null() {
            // SAFETY: `swap(null)` transferred exclusive ownership of this chain.
            let next = unsafe { (*cur).next.load(Ordering::Relaxed) };
            // SAFETY: `cur` is a live `OverflowScratch` node.
            let active_tid = unsafe { (*cur).active_tid.load(Ordering::Acquire) };
            // SAFETY: `child_pid` is immutable after initialization.
            let child_pid = unsafe { (*cur).child_pid };
            let detached = if active_tid == 0 {
                true
            } else {
                if *my_pid == 0 {
                    *my_pid = getpid();
                }
                is_vm_detached(*my_pid, child_pid)
            };
            if detached {
                // SAFETY: Child detached (`mm_release` cleared `active_tid` or
                // `kcmp` confirmed detachment) or failure cleanup already
                // detached `set_tid_address` and cleared `active_tid`.
                unsafe {
                    *(*cur).duped_fd.get() = -1;
                    *(*cur).arena.get() = None;
                    ptr::drop_in_place(cur);
                    PageAllocator.deallocate(
                        NonNull::new_unchecked(cur).cast(),
                        Layout::new::<OverflowScratch>(),
                    );
                }
            } else {
                // SAFETY: `cur` is still attached; return it to the parked list.
                unsafe { push_overflow_parked(cur) };
            }
            cur = next;
        }
    }

    /// Reclaims any parked `CLONE_VM` exec scratch slots and overflow nodes
    /// whose child has already detached (`execve` succeeded) or exited, keeping
    /// parent RSS bounded across arbitrarily many `vfork`/`posix_spawn` spawns
    /// even when `kcmp` is denied or more than 64 children are live at once.
    fn reclaim_detached_exec_slots() {
        let mut my_pid = 0_i32;
        for slot in &EXEC_SLOTS {
            let state = slot.state.load(Ordering::Acquire);
            if state <= SLOT_BUSY {
                continue;
            }
            let detached = if slot.active_tid.load(Ordering::Acquire) == 0
                && slot.state.load(Ordering::Acquire) == state
            {
                true
            } else {
                if my_pid == 0 {
                    my_pid = getpid();
                }
                is_vm_detached(my_pid, parked_pid(state))
            };
            if !detached {
                continue;
            }
            if slot
                .state
                .compare_exchange(state, SLOT_BUSY, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // SAFETY: Exclusive ownership via `SLOT_BUSY` and `child_pid`
                // has detached from our VM. Do NOT close `duped_fd` — it
                // belonged to the child's fd table.
                unsafe {
                    *slot.duped_fd.get() = -1;
                    *slot.arena.get() = None;
                }
                slot.active_tid.store(0, Ordering::Release);
                slot.state.store(SLOT_FREE, Ordering::Release);
            }
        }
        reclaim_overflow_scratch(&mut my_pid);
    }

    fn acquire_exec_slot() -> Option<&'static ExecSlot> {
        EXEC_SLOTS.iter().find(|slot| {
            slot.state
                .compare_exchange(SLOT_FREE, SLOT_BUSY, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        })
    }

    /// Failure cleanup invoked by `fspy_elf_loader` when `SYS_execve` returns
    /// an error on a pooled [`ExecSlot`].
    unsafe fn cleanup_exec_slot(raw: *mut c_void) {
        // SAFETY: `raw` is a `&'static ExecSlot` owned by this failed `execve`.
        let slot = unsafe { &*(raw.cast::<ExecSlot>()) };
        // Detach `clear_child_tid` FIRST while `active_tid != 0` and our thread
        // is still attached to the VM (so no reclaimer can claim this slot).
        if slot.state.load(Ordering::Acquire) > SLOT_BUSY {
            set_tid_address(ptr::null());
        }
        slot.state.store(SLOT_BUSY, Ordering::Release);
        // SAFETY: Exclusive ownership of `slot` in the same process that opened
        // `duped_fd` and allocated `arena`.
        unsafe {
            let fd = *slot.duped_fd.get();
            *slot.duped_fd.get() = -1;
            if fd >= 0 {
                drop(OwnedFd::from_raw_fd(fd));
            }
            *slot.arena.get() = None;
        }
        slot.active_tid.store(0, Ordering::Release);
        slot.state.store(SLOT_FREE, Ordering::Release);
    }

    /// Failure cleanup invoked by `fspy_elf_loader` when `SYS_execve` returns
    /// an error on an [`OverflowScratch`] allocation.
    unsafe fn cleanup_overflow_scratch(raw: *mut c_void) {
        let Some(ptr) = NonNull::new(raw.cast::<OverflowScratch>()) else {
            return;
        };
        // SAFETY: `ptr` is valid and still live because either it was never
        // parked, or our thread is still attached to the VM with `active_tid != 0`.
        let parked = unsafe { (*ptr.as_ptr()).parked };
        if parked {
            set_tid_address(ptr::null());
            // SAFETY: No reclaimer accesses `duped_fd`/`arena` while `active_tid != 0`.
            unsafe {
                let fd = *(*ptr.as_ptr()).duped_fd.get();
                *(*ptr.as_ptr()).duped_fd.get() = -1;
                if fd >= 0 {
                    drop(OwnedFd::from_raw_fd(fd));
                }
                *(*ptr.as_ptr()).arena.get() = None;
                (*ptr.as_ptr()).active_tid.store(0, Ordering::Release);
            }
            let mut my_pid = 0;
            reclaim_overflow_scratch(&mut my_pid);
        } else {
            // SAFETY: Not pushed onto `OVERFLOW_PARKED_HEAD`; exclusively owned.
            unsafe {
                let fd = *(*ptr.as_ptr()).duped_fd.get();
                if fd >= 0 {
                    drop(OwnedFd::from_raw_fd(fd));
                }
                ptr::drop_in_place(ptr.as_ptr());
                PageAllocator.deallocate(ptr.cast(), Layout::new::<OverflowScratch>());
            }
        }
    }

    /// Resolves, validates, and prepares an `execve` / `execveat` call through
    /// `fspy_preload_linux`, returning [`SyscallAction::Execve`] so
    /// `fspy_elf_loader` releases the private `SIGSYS` stack before executing
    /// the replacement `SYS_execve`.
    fn handle_exec(
        dirfd: Option<BorrowedFd<'_>>,
        prog_ptr: usize,
        argv_ptr: usize,
        envp_ptr: usize,
        flags: ExecAtFlags,
        sigmask: u64,
        is_shared_vm_child: bool,
    ) -> Result<SyscallAction> {
        reclaim_detached_exec_slots();

        if let Some(slot) = acquire_exec_slot() {
            // SAFETY: We hold exclusive `SLOT_BUSY` ownership of `slot`.
            unsafe {
                *slot.duped_fd.get() = -1;
                *slot.arena.get() = Some(page_bump());
            }
            // SAFETY: `slot.arena` was just initialized to `Some` and stays
            // live until `cleanup_exec_slot` or post-detach reclamation drops it.
            let arena: &'static PageBump = unsafe { (*slot.arena.get()).as_ref().unwrap() };

            if is_shared_vm_child {
                let child_pid = getpid();
                slot.active_tid.store(child_pid, Ordering::Release);
                // Publish parked state and arm `set_tid_address` BEFORE
                // `prepare_exec_in` so `SIGKILL` at any point (including during
                // path resolution or ELF validation) leaves the slot reclaimable
                // once the kernel's `mm_release()` clears `active_tid`.
                slot.state.store(park_state(child_pid), Ordering::Release);
                set_tid_address(&raw const slot.active_tid);
            }

            match prepare_exec_in(dirfd, prog_ptr, argv_ptr, envp_ptr, flags, sigmask, arena) {
                Ok((raw_out, duped_fd)) => {
                    // SAFETY: While this thread is alive and attached to the VM
                    // (`active_tid != 0` for `CLONE_VM` children, or `SLOT_BUSY`
                    // otherwise), no reclaimer can claim `slot`.
                    unsafe { *slot.duped_fd.get() = duped_fd };
                    return Ok(SyscallAction::Execve {
                        prog: raw_out.prog.cast(),
                        argv: raw_out.argv.cast(),
                        envp: raw_out.envp.cast(),
                        cleanup: Some((cleanup_exec_slot, ptr::from_ref(slot).cast_mut().cast())),
                    });
                }
                Err(err) => {
                    // SAFETY: `slot` is still owned by this live thread; release it on error.
                    unsafe { cleanup_exec_slot(ptr::from_ref(slot).cast_mut().cast()) };
                    return Err(err);
                }
            }
        }

        // Overflow fallback when more than `MAX_EXEC_SLOTS` concurrent `vfork`
        // children are mid-exec simultaneously.
        let layout = Layout::new::<OverflowScratch>();
        let mem = PageAllocator.allocate(layout).map_err(|_| Error::NOMEM)?;
        let ptr: NonNull<OverflowScratch> = mem.cast();
        let child_pid = if is_shared_vm_child { getpid() } else { 0 };
        // SAFETY: `ptr` is freshly page-allocated with `Layout::new::<OverflowScratch>()`.
        unsafe {
            ptr.as_ptr().write(OverflowScratch {
                next: AtomicPtr::new(ptr::null_mut()),
                active_tid: AtomicI32::new(child_pid),
                child_pid,
                parked: is_shared_vm_child,
                arena: UnsafeCell::new(Some(page_bump())),
                duped_fd: UnsafeCell::new(-1),
            });
            if is_shared_vm_child {
                // Publish onto `OVERFLOW_PARKED_HEAD` and arm `set_tid_address`
                // BEFORE `prepare_exec_in` so `SIGKILL` at any point leaves the
                // overflow scratch node reclaimable once `mm_release()` clears
                // `active_tid`.
                push_overflow_parked(ptr.as_ptr());
                set_tid_address(&raw const (*ptr.as_ptr()).active_tid);
            }
        }
        // SAFETY: `ptr` remains live until `cleanup_overflow_scratch` or post-detach sweep.
        let arena: &'static PageBump = unsafe { (*(*ptr.as_ptr()).arena.get()).as_ref().unwrap() };
        match prepare_exec_in(dirfd, prog_ptr, argv_ptr, envp_ptr, flags, sigmask, arena) {
            Ok((raw_out, duped_fd)) => {
                // SAFETY: While this thread is alive and attached (`active_tid != 0`
                // for `CLONE_VM` children, or unparked otherwise), no reclaimer
                // touches `duped_fd`.
                unsafe { *(*ptr.as_ptr()).duped_fd.get() = duped_fd };
                Ok(SyscallAction::Execve {
                    prog: raw_out.prog.cast(),
                    argv: raw_out.argv.cast(),
                    envp: raw_out.envp.cast(),
                    cleanup: Some((cleanup_overflow_scratch, ptr.as_ptr().cast())),
                })
            }
            Err(err) => {
                // SAFETY: `ptr` is still owned by this live thread; release it on error.
                unsafe { cleanup_overflow_scratch(ptr.as_ptr().cast()) };
                Err(err)
            }
        }
    }

    fn prepare_exec_in(
        dirfd: Option<BorrowedFd<'_>>,
        prog_ptr: usize,
        argv_ptr: usize,
        envp_ptr: usize,
        flags: ExecAtFlags,
        sigmask: u64,
        arena: &'static PageBump,
    ) -> Result<(RawExec, RawFd)> {
        let mut exec = copy_user_exec(prog_ptr, argv_ptr, envp_ptr, arena)?;
        let resolved = resolve_exec(&mut exec, dirfd, flags, |p| send_path(AccessMode::Read, p))?;
        fspy_elf_loader::validate_exec(resolved.file.as_fd())?;

        // Keep `duped_fd` open across `SYS_execve` (`fcntl_dupfd` clears
        // `FD_CLOEXEC`). If `prepare_exec_in` fails below, `OwnedFd` closes it
        // automatically; once handed back as a raw fd, `cleanup_*` closes it on
        // `SYS_execve` failure while a successful exec transfers it to the child.
        let duped_fd =
            if resolved.fd_backed { Some(fcntl_dupfd(resolved.file.as_fd(), 3)?) } else { None };

        let rt = runtime();
        let target = if let Some(ref fd) = duped_fd {
            Target::ResolvedFd {
                fd: fd.as_raw_fd(),
                execfn: IpcStr::from_bytes(&resolved.execfn),
                sigmask,
            }
        } else {
            Target::ResolvedPath {
                path: IpcStr::from_bytes(&exec.program),
                execfn: IpcStr::from_bytes(&resolved.execfn),
                sigmask,
            }
        };
        let bootstrap = Bootstrap {
            ipc_channel_conf: rt.ipc_channel_conf,
            preload_path: rt.preload_path,
            target,
        };
        let encoded = encode_bootstrap(&bootstrap, arena);
        ensure_bootstrap_env(&mut exec.envs, encoded)?;

        exec.program = copy_bytes(rt.preload_path.as_bytes(), arena);
        let raw_out = RawExec::from_exec(exec);
        let duped_raw = duped_fd.map_or(-1, |fd| {
            let raw = fd.as_raw_fd();
            core::mem::forget(fd);
            raw
        });
        Ok((raw_out, duped_raw))
    }
}
