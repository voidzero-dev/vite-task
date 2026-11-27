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
        mem::size_of,
        ptr::{self, NonNull},
        sync::atomic::{AtomicI32, AtomicPtr, Ordering},
    };

    use allocator_api2::{alloc::Allocator, vec::Vec};
    use bstr::BStr;
    use fspy_elf_loader::{Syscall, SyscallAction, SyscallInterceptor};
    use fspy_nostd::{
        BorrowedFd, CStr, CWD, Error, OwnedFd, RawFd, Result, Thin, env::InitialStack,
        fs::fcntl_dupfd,
    };
    use fspy_nostd_alloc::{
        PageAllocator, PageBump,
        fs::{fd_path, resolve_path_at},
        page_bump, pooled_bump,
    };
    use fspy_shared::ipc::{
        AccessMode, IpcPath, IpcStr,
        channel::{ChannelConf, Sender},
    };
    use fspy_shared_unix::{
        Exec, RawExec,
        linux::{
            BOOTSTRAP_ENV_NAME, Bootstrap, ExecAtFlags, Target, encode_bootstrap,
            ensure_bootstrap_env, resolve_exec,
        },
    };
    use linux_raw_sys::general::{
        AT_EMPTY_PATH, O_ACCMODE, O_RDWR, O_WRONLY, SIG_SETMASK, open_how,
    };
    use syscalls::Sysno;

    fspy_nostd::main!(run);
    fspy_nostd::no_global_alloc!();

    /// Linux 6.14+ `execveat(2)` flag that checks executability without replacing
    /// the process image.
    const AT_EXECVE_CHECK: u32 = 0x10000;

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

    /// Scratch memory for one `execve` re-routed through the launcher: the
    /// arena holding the rewritten `argv`/`envp`, and any descriptor kept open
    /// for the new image.
    ///
    /// A successful exec in a `CLONE_VM` child (`vfork` / `posix_spawn`)
    /// leaves the node in its parent's memory, so such nodes are pushed onto
    /// [`PARKED_EXEC_SCRATCH`] with `active_tid` registered through
    /// `set_tid_address`. The kernel clears it once the child execs or exits,
    /// and a later exec trap frees the node.
    struct ExecScratch {
        next: AtomicPtr<Self>,
        active_tid: AtomicI32,
        parked: bool,
        arena: UnsafeCell<Option<PageBump>>,
        duped_fd: UnsafeCell<RawFd>,
    }

    // SAFETY: `arena` and `duped_fd` are mutated only by the exec'ing thread
    // before `active_tid` becomes `0` (`Release`), and dropped by a reclaimer
    // only after it observes `0` (`Acquire`) on a node it detached from
    // `PARKED_EXEC_SCRATCH`.
    unsafe impl Sync for ExecScratch {}

    static STARTUP_BUMP: SyncCell<PageBump> = SyncCell(UnsafeCell::new(page_bump()));
    static RUNTIME: SyncCell<Option<Runtime>> = SyncCell(UnsafeCell::new(None));
    static PARKED_EXEC_SCRATCH: AtomicPtr<ExecScratch> = AtomicPtr::new(ptr::null_mut());

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
        // SAFETY: `path_ptr` is the trapped syscall's path argument.
        let Ok(c_path) = (unsafe { fspy_elf_loader::user_cstr(path_ptr) }) else {
            return;
        };
        let raw = c_path.count().as_units();
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
        let arena = pooled_bump();
        if let Ok(Some(resolved)) = resolve_path_at(&arena, dirfd, c_path) {
            send_path(mode, BStr::new(resolved.as_units()));
        }
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
    unsafe fn handle_syscall(call: &Syscall) -> Result<SyscallAction> {
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
                // replacing the process image; record the path lookup like
                // `faccessat2` and let the kernel answer directly.
                if (raw_flags & AT_EXECVE_CHECK) != 0 {
                    let empty_ok = (raw_flags & AT_EMPTY_PATH) != 0;
                    report_path_at(
                        borrow_dirfd(call.args[0]),
                        call.args[1],
                        AccessMode::Read,
                        empty_ok,
                    );
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
    fn handle_fs_syscall(call: &Syscall) {
        // SAFETY: `dirfd` arguments come directly from the trapped syscall; `-1`
        // maps to `None` and invalid fds return `Ok(None)` from `fd_path`.
        let dirfd = |idx: usize| unsafe { BorrowedFd::try_borrow_raw(arg_i32(call.args[idx])) };
        let a = call.args;

        match call.sysno {
            #[cfg(target_arch = "x86_64")]
            Sysno::open => report_path_at(Some(CWD), a[0], open_mode(arg_u32(a[1])), false),
            Sysno::openat => report_path_at(dirfd(0), a[1], open_mode(arg_u32(a[2])), false),
            Sysno::openat2 => report_openat2(dirfd(0), a[1], a[2], a[3]),
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
                report_path_at(Some(CWD), a[0], AccessMode::Read, false);
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::newfstatat => {
                let empty_ok = (arg_u32(a[3]) & AT_EMPTY_PATH) != 0;
                report_path_at(dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            #[cfg(target_arch = "aarch64")]
            Sysno::fstatat => {
                let empty_ok = (arg_u32(a[3]) & AT_EMPTY_PATH) != 0;
                report_path_at(dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            Sysno::statx => {
                let empty_ok = (arg_u32(a[2]) & AT_EMPTY_PATH) != 0;
                report_path_at(dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            Sysno::faccessat => {
                report_path_at(dirfd(0), a[1], AccessMode::Read, false);
            }
            Sysno::faccessat2 => {
                let empty_ok = (arg_u32(a[3]) & AT_EMPTY_PATH) != 0;
                report_path_at(dirfd(0), a[1], AccessMode::Read, empty_ok);
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::mkdir => report_path_at(Some(CWD), a[0], AccessMode::Mkdir, false),
            Sysno::mkdirat => report_path_at(dirfd(0), a[1], AccessMode::Mkdir, false),
            #[cfg(target_arch = "x86_64")]
            Sysno::rename => report_path_at(Some(CWD), a[1], AccessMode::RenameTarget, false),
            Sysno::renameat | Sysno::renameat2 => {
                report_path_at(dirfd(2), a[3], AccessMode::RenameTarget, false);
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::link | Sysno::symlink => {
                report_path_at(Some(CWD), a[1], AccessMode::Write, false);
            }
            Sysno::linkat => report_path_at(dirfd(2), a[3], AccessMode::Write, false),
            Sysno::symlinkat => report_path_at(dirfd(1), a[2], AccessMode::Write, false),
            _ => {}
        }
    }

    fn report_openat2(dirfd: Option<BorrowedFd<'_>>, path_ptr: usize, how_ptr: usize, size: usize) {
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
        report_path_at(dirfd, path_ptr, mode, false);
    }

    #[inline]
    fn set_tid_address(addr: *const AtomicI32) {
        // SAFETY: `addr` is either null or points to a live `AtomicI32` that
        // remains mapped while the calling `CLONE_VM` child is attached to the
        // current `mm_struct`.
        let _ = unsafe { syscalls::raw::syscall1(Sysno::set_tid_address as usize, addr.addr()) };
    }

    unsafe fn push_parked_exec_scratch(node: *mut ExecScratch) {
        let mut head = PARKED_EXEC_SCRATCH.load(Ordering::Acquire);
        loop {
            // SAFETY: Caller owns `node` exclusively until CAS publishes it.
            unsafe { (*node).next.store(head, Ordering::Relaxed) };
            match PARKED_EXEC_SCRATCH.compare_exchange_weak(
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

    /// Drops `node`'s arena and returns its page to the allocator.
    ///
    /// # Safety
    ///
    /// `node` must be a live `ExecScratch` that nothing else references.
    unsafe fn free_exec_scratch(node: *mut ExecScratch) {
        // SAFETY: forwarded from the caller contract.
        unsafe {
            ptr::drop_in_place(node);
            PageAllocator
                .deallocate(NonNull::new_unchecked(node).cast(), Layout::new::<ExecScratch>());
        }
    }

    /// Frees parked `CLONE_VM` exec scratch whose child has exec'd or exited,
    /// keeping parent memory bounded across any number of `vfork` /
    /// `posix_spawn` children.
    fn reclaim_exec_scratch() {
        if PARKED_EXEC_SCRATCH.load(Ordering::Acquire).is_null() {
            return;
        }
        let mut cur = PARKED_EXEC_SCRATCH.swap(ptr::null_mut(), Ordering::AcqRel);
        while !cur.is_null() {
            // SAFETY: `swap(null)` transferred exclusive ownership of this chain.
            let next = unsafe { (*cur).next.load(Ordering::Relaxed) };
            // SAFETY: `cur` is a live node of the detached chain.
            if unsafe { (*cur).active_tid.load(Ordering::Acquire) } == 0 {
                // SAFETY: The child left this memory (or failure cleanup
                // released the node), so nothing references `cur`. Its
                // `duped_fd` belonged to the child's descriptor table.
                unsafe { free_exec_scratch(cur) };
            } else {
                // SAFETY: `cur` is still in use by its child; park it again.
                unsafe { push_parked_exec_scratch(cur) };
            }
            cur = next;
        }
    }

    /// Failure cleanup invoked by `fspy_elf_loader` when `SYS_execve` returns
    /// an error.
    unsafe fn cleanup_exec_scratch(raw: *mut c_void) {
        let node = raw.cast::<ExecScratch>();
        // SAFETY: `node` is the scratch of this failed exec, still owned by
        // the calling thread, and `duped_fd` is open in its descriptor table.
        unsafe {
            let fd = *(*node).duped_fd.get();
            *(*node).duped_fd.get() = -1;
            if fd >= 0 {
                drop(OwnedFd::from_raw_fd(fd));
            }
            if (*node).parked {
                // Already published: release its arena and let a later
                // reclaim free the node.
                set_tid_address(ptr::null());
                *(*node).arena.get() = None;
                (*node).active_tid.store(0, Ordering::Release);
            } else {
                free_exec_scratch(node);
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
        reclaim_exec_scratch();

        let mem = PageAllocator.allocate(Layout::new::<ExecScratch>()).map_err(|_| Error::NOMEM)?;
        let node = mem.cast::<ExecScratch>().as_ptr();
        // SAFETY: `node` is freshly page-allocated for an `ExecScratch`.
        unsafe {
            node.write(ExecScratch {
                next: AtomicPtr::new(ptr::null_mut()),
                active_tid: AtomicI32::new(0),
                parked: false,
                arena: UnsafeCell::new(Some(page_bump())),
                duped_fd: UnsafeCell::new(-1),
            });
        }
        // SAFETY: `arena` was just set and lives until the node is freed.
        let arena: &'static PageBump = unsafe { (*(*node).arena.get()).as_ref().unwrap() };
        let (raw_out, duped_fd) =
            match prepare_exec_in(dirfd, prog_ptr, argv_ptr, envp_ptr, flags, sigmask, arena) {
                Ok(prepared) => prepared,
                Err(err) => {
                    // SAFETY: the node was never published.
                    unsafe { free_exec_scratch(node) };
                    return Err(err);
                }
            };
        // SAFETY: the node is owned by this thread until published below.
        unsafe {
            *(*node).duped_fd.get() = duped_fd;
            if is_shared_vm_child {
                (*node).parked = true;
                (*node).active_tid.store(1, Ordering::Release);
                push_parked_exec_scratch(node);
                set_tid_address(&raw const (*node).active_tid);
            }
        }
        Ok(SyscallAction::Execve {
            prog: raw_out.prog.cast(),
            argv: raw_out.argv.cast(),
            envp: raw_out.envp.cast(),
            cleanup: Some((cleanup_exec_scratch, node.cast())),
        })
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
        let raw = RawExec {
            prog: ptr::with_exposed_provenance(prog_ptr),
            argv: ptr::with_exposed_provenance(argv_ptr),
            envp: ptr::with_exposed_provenance(envp_ptr),
        };
        // SAFETY: the pointers are the trapped syscall's own `execve` arguments.
        let mut exec = unsafe { raw.to_exec(arena) }?;
        let resolved = resolve_exec(&mut exec, dirfd, flags, |p| send_path(AccessMode::Read, p))?;
        fspy_elf_loader::validate_exec(resolved.file.as_fd())?;

        // Hand the validated descriptor itself to the new image instead of a
        // pathname to reopen: the file may be replaced or unlinked meanwhile,
        // and `/proc/self/fd/N` targets close with `FD_CLOEXEC` on exec. Keep
        // `duped_fd` open across `SYS_execve` (`fcntl_dupfd` clears
        // `FD_CLOEXEC`). If `prepare_exec_in` fails below, `OwnedFd` closes it
        // automatically; once handed back as a raw fd, `cleanup_*` closes it on
        // `SYS_execve` failure while a successful exec transfers it to the child.
        let duped_fd = fcntl_dupfd(resolved.file.as_fd(), 3)?;

        let rt = runtime();
        let target = Target::ResolvedFd {
            fd: duped_fd.as_raw_fd(),
            execfn: IpcStr::from_bytes(&resolved.execfn),
            sigmask,
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
        let duped_raw = duped_fd.as_raw_fd();
        core::mem::forget(duped_fd);
        Ok((raw_out, duped_raw))
    }
}
