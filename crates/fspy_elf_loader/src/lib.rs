//! Load a Linux ELF executable inside an existing process.
//!
//! For a dynamic executable, the kernel maps the executable and its ELF
//! interpreter, writes startup data onto a new stack, and enters the
//! interpreter. The interpreter (such as `ld-linux`) loads shared libraries,
//! fixes addresses through relocations, and enters the executable's startup
//! code. That startup code eventually calls the application's `main`.
//!
//! For a static executable there is no `PT_INTERP`. The kernel maps only the
//! executable and enters its `e_entry`. `AT_BASE` is 0. Static-PIE images are
//! `ET_DYN` without an interpreter; their CRT applies `RELATIVE` relocations.
//!
//! We implement the mapping, stack, `SIGSYS` virtualization, and entry steps
//! after the kernel has already executed our freestanding launcher. We leave
//! its mappings and signal handlers in place. Calling another kernel exec here
//! would discard them.
#![cfg(any(target_os = "linux", target_os = "none"))]
#![no_std]

#[cfg(test)]
extern crate std;

use core::{ptr, slice};

use elf::{abi, endian::LittleEndian, file::Class, segment::SegmentTable};
use fspy_nostd::{BorrowedFd, CStr, OwnedFd, Result, Thin, env::AuxEntry};
use linux_raw_sys::{
    auxvec::{AT_NULL, AT_PAGESZ, AT_PHDR, AT_PHENT, AT_PHNUM},
    prctl::PR_SET_NAME,
};
use syscalls::Sysno;

mod image;
mod mapping;
mod sigsys;
mod stack;

#[cfg(target_arch = "x86_64")]
const LAUNCHER_REGION: (usize, usize) = (0x5500_0000_0000, 0x5a00_0000_0000);
#[cfg(target_arch = "aarch64")]
const LAUNCHER_REGION: (usize, usize) = (0x0002_0000_0000, 0x0005_0000_0000);

/// Decoded syscall invocation passed to a [`SyscallInterceptor`].
#[derive(Clone, Copy, Debug)]
pub struct Syscall<'a> {
    pub sysno: Sysno,
    pub args: [usize; 6],
    raw_uc_sigmask: u64,
    cached_path: Option<(usize, &'a [u8])>,
}

impl<'a> Syscall<'a> {
    /// Returns the calling thread's virtualized signal mask prior to this
    /// syscall (including the virtualized `SIGSYS` bit).
    ///
    /// Computed lazily because only `execve`/`execveat` interceptors typically
    /// need it; avoids a `gettid` syscall on hot filesystem traps.
    ///
    /// Because the `SIGSYS` dispatcher masks asynchronous signals while running
    /// the interceptor, an interceptor returning [`SyscallAction::Execve`] must
    /// propagate or restore this mask in the new image.
    #[must_use]
    pub fn sigmask(&self) -> u64 {
        sigsys::virtual_sigmask_from(self.raw_uc_sigmask)
    }

    /// Returns `true` when executing inside a `CLONE_VM` (`vfork` /
    /// `posix_spawn`) child that still shares its address space with the parent
    /// process.
    ///
    /// Computed lazily to avoid a `getpid` syscall on hot filesystem traps.
    #[must_use]
    #[expect(clippy::unused_self, reason = "scopes the query to an active SIGSYS trap")]
    pub fn is_shared_vm_child(&self) -> bool {
        sigsys::is_shared_vm_child()
    }

    /// Returns the already copied C-string bytes (excluding the terminating
    /// NUL) if `addr` was prefetched by the loader's `/proc/*/exe` check during
    /// this trap, avoiding a second copy.
    #[must_use]
    pub fn cached_cstr(&self, addr: usize) -> Option<&'a [u8]> {
        self.cached_path.and_then(|(cached_addr, bytes)| (cached_addr == addr).then_some(bytes))
    }
}

/// Failure-cleanup callback and context pointer attached to
/// [`SyscallAction::Execve`].
pub type ExecveCleanup = (unsafe fn(*mut core::ffi::c_void), *mut core::ffi::c_void);

/// Action returned by a [`SyscallInterceptor`] to tell the `SIGSYS` dispatcher
/// how to complete the trapped syscall after releasing the private `SIGSYS`
/// stack.
#[derive(Clone, Copy, Debug)]
pub enum SyscallAction {
    /// Replay the (possibly `/proc/*/exe`-rewritten) trapped syscall.
    Replay,
    /// Execute a replacement `SYS_execve(prog, argv, envp)` on the entry stack
    /// after releasing the private `SIGSYS` stack and clearing the calling
    /// thread's virtual `SIGSYS` mask entry.
    ///
    /// If `SYS_execve` fails, `cleanup` (when `Some`) is invoked with its
    /// context pointer before returning `-errno` to the caller.
    Execve {
        prog: *const u8,
        argv: *const *const u8,
        envp: *const *const u8,
        cleanup: Option<ExecveCleanup>,
    },
}

/// Optional caller-supplied syscall interceptor invoked from the loader's
/// `SIGSYS` dispatcher.
///
/// The listed `syscalls` are merged into the seccomp-BPF filter alongside the
/// loader's own virtualization syscalls. Signal-virtualization syscalls and
/// `/proc/*/exe` `readlink` calls are handled directly by the loader; for
/// path-following syscalls targeting `/proc/*/exe`, `handle` observes the
/// already-rewritten target executable path.
///
/// Returning `Ok(SyscallAction::Replay)` replays the syscall; returning
/// `Ok(SyscallAction::Execve { .. })` releases the private `SIGSYS` stack
/// before executing the replacement `SYS_execve`; returning `Err(error)`
/// short-circuits the syscall and returns `-error.raw_os_error()` directly to
/// the caller.
#[derive(Clone, Copy)]
pub struct SyscallInterceptor {
    pub syscalls: &'static [Sysno],
    pub handle: unsafe fn(&Syscall<'_>) -> Result<SyscallAction>,
}

/// Copies a NUL-terminated C string from a trapped syscall's user pointer into
/// `buf`.
///
/// Returns `Ok(None)` if no NUL byte is found within `buf.len()` bytes (all
/// `buf.len()` bytes of `buf` are initialized on `Ok(None)`).
///
/// # Errors
///
/// Returns [`fspy_nostd::Error::FAULT`] if `addr` is in the first page (such as
/// `NULL`).
///
/// # Safety
///
/// Any other `addr` must point to a readable NUL-terminated string (or at
/// least `buf.len()` readable bytes). Like the macOS and Windows preloads, the
/// runtime assumes the pointers programs pass to syscalls are valid; a bad one
/// crashes the process instead of failing with `EFAULT`.
pub unsafe fn copy_user_cstr(
    addr: usize,
    buf: &mut [core::mem::MaybeUninit<u8>],
) -> Result<Option<&[u8]>> {
    // SAFETY: forwarded from caller contract.
    unsafe { sigsys::copy_user_cstr(addr, buf) }
}

/// Copies a `Copy` value from a trapped syscall's user pointer.
///
/// # Errors
///
/// Returns [`fspy_nostd::Error::FAULT`] if `addr` is in the first page (such as
/// `NULL`).
///
/// # Safety
///
/// `T` must be valid for any bit pattern, and any other `addr` must be
/// readable for `size_of::<T>()` bytes (see [`copy_user_cstr`]).
pub unsafe fn copy_user_value<T: Copy>(addr: usize) -> Result<T> {
    // SAFETY: forwarded from caller contract.
    unsafe { sigsys::copy_user_value(addr) }
}

/// Validates that `fd` refers to a loadable ELF64 executable (and, if present,
/// a valid `PT_INTERP` dynamic interpreter) without mapping execution segments,
/// mutating process state, or closing `fd`.
///
/// # Errors
///
/// Returns [`fspy_nostd::Error::NOEXEC`] for malformed or unsupported ELF
/// images (including wrong architecture, non-ELF64, executable `PT_GNU_STACK`,
/// fixed-address `PT_INTERP`, or nested `PT_INTERP`), or the underlying OS
/// error if `fd` or its `PT_INTERP` cannot be inspected or opened.
pub fn validate_exec(fd: BorrowedFd<'_>) -> Result<()> {
    image::validate_exec_fd(fd, fspy_nostd::param::page_size())
}

/// Loads an ELF executable into the current process, installs `SIGSYS`
/// virtualization (plus any optional `interceptor`), and enters it.
///
/// Takes ownership of a private executable descriptor and closes it after
/// mapping. Loads the executable and, when present, its `PT_INTERP`. Sets the
/// process name (`PR_SET_NAME`) to the basename of `execfn`, sizes the new
/// stack from `RLIMIT_STACK` (clamped to `[8 MiB, 64 MiB]`), installs the
/// `SIGSYS` + `/proc/*/exe` + seccomp-BPF runtime, builds the initial stack,
/// and transfers control to the interpreter (or directly to the executable when
/// there is no interpreter).
///
/// `execfn` is the original execution name used for `AT_EXECFN` and
/// `PR_SET_NAME`; it need not match `argv[0]`. Arguments and environment
/// entries are copied to the new stack. `inherited_auxv` borrows entries from
/// the launcher's original kernel stack, excluding `AT_NULL`.
///
/// # Safety
///
/// Call only from a fresh, single-threaded freestanding launcher process linked
/// with this crate's `link_args` metadata (`--no-pie` at the high fixed base
/// address). The executable and interpreter must be trusted to run with the
/// process's current privileges. Their backing files must remain unchanged
/// throughout loading. The launcher must not have initialized libc or
/// thread-local runtime state. Auxiliary entries must describe this process;
/// any referenced memory must remain readable and unchanged until copied or
/// retained for the target. The caller must permit execution to abandon its
/// stack without running destructors. Any installed `interceptor.handle` must
/// be async-signal-safe and live inside the launcher's `PF_X` code segment.
///
/// # Panics
///
/// Panics when the launcher image violates its high fixed-address `PF_X`
/// invariants, `AT_PAGESZ` is missing or invalid, loading detects an
/// unsupported or malformed image, runs out of stack capacity, fails to map or
/// open a file, or fails to install the `SIGSYS`/seccomp runtime.
pub unsafe fn exec_elf<'a>(
    elf_file: OwnedFd,
    execfn: CStr<'a, Thin>,
    argv: impl Iterator<Item = CStr<'a, Thin>>,
    envp: impl Iterator<Item = CStr<'a, Thin>>,
    inherited_auxv: &[AuxEntry],
    interceptor: Option<SyscallInterceptor>,
) -> ! {
    let page_size = inherited_auxv
        .iter()
        .find(|entry| entry.key == usize::try_from(AT_PAGESZ).unwrap())
        .expect("missing AT_PAGESZ")
        .value;
    assert!(page_size.is_power_of_two(), "invalid AT_PAGESZ");

    // Verify the launcher's own fixed-address ELF layout and locate its `PF_X`
    // `PT_LOAD` range `[code_start, code_end)` for seccomp IP allowlisting.
    // SAFETY: caller supplies the kernel auxiliary vector for this process.
    let code_range = unsafe { locate_launcher_code_range(inherited_auxv) };
    if let Some(ref ic) = interceptor {
        let handle_addr = (ic.handle as *const ()).addr() as u64;
        assert!(
            code_range.0 <= handle_addr && handle_addr < code_range.1,
            "SyscallInterceptor::handle outside launcher PF_X range"
        );
    }

    let executable = image::File::open(elf_file.as_fd()).expect("invalid ELF executable");
    let executable_layout =
        executable.validate_segments(page_size).expect("invalid ELF executable segments");
    assert!(executable_layout.has_phdr, "program headers are not mapped by PT_LOAD");

    // PT_INTERP names a second ELF file. Loading it here lets us delegate
    // dynamic linking and libc startup to the program's own interpreter.
    // Static and static-PIE images omit it; the executable is the startup image.
    let interp_path = executable.interpreter().expect("invalid PT_INTERP");
    let interp_fd = interp_path.map(|path| {
        fspy_nostd::fs::openat(
            fspy_nostd::CWD,
            path,
            fspy_nostd::fs::OFlags::RDONLY | fspy_nostd::fs::OFlags::CLOEXEC,
            fspy_nostd::fs::Mode::empty(),
        )
        .expect("cannot open ELF interpreter")
    });
    let interpreter = interp_fd.as_ref().map(|fd| {
        let file = image::File::open(fd.as_fd()).expect("invalid ELF interpreter");
        assert!(file.is_dynamic(), "fixed-address PT_INTERP is unsupported");
        assert!(
            file.interpreter().expect("invalid nested PT_INTERP").is_none(),
            "nested PT_INTERP is unsupported"
        );
        let layout = file.validate_segments(page_size).expect("invalid ELF interpreter segments");
        (file, layout)
    });

    let executable_image = executable.map(page_size, &executable_layout);
    let interpreter_image = interpreter.as_ref().map(|(file, layout)| file.map(page_size, layout));

    // SAFETY: the caller supplies live auxiliary payloads from this process.
    let stack = unsafe {
        stack::build(
            execfn,
            argv,
            envp,
            inherited_auxv.iter().copied(),
            &executable_image,
            interpreter_image.as_ref(),
            page_size,
        )
    };

    // Match kernel execve's `/proc/self/comm` basename update.
    set_process_name(execfn);

    // Install SIGSYS + /proc/*/exe + seccomp virtualization while `elf_file`
    // is still open so `/proc/self/fd/<fd>` resolves the canonical target path.
    let min_sigstksz = inherited_auxv
        .iter()
        .find(|entry| entry.key == 51) // AT_MINSIGSTKSZ
        .map_or(0, |entry| entry.value);
    // SAFETY: fresh single-threaded launcher with validated resident `code_range`.
    unsafe {
        sigsys::install(code_range, elf_file.as_fd(), execfn, min_sigstksz, page_size, interceptor)
    }
    .expect("cannot install SIGSYS virtualization runtime");

    // With PT_INTERP, the interpreter runs first even though AT_ENTRY points
    // at the executable. It uses that value to hand off after linking.
    // Without PT_INTERP, AT_BASE is 0 and the executable's e_entry starts.
    let entry = interpreter_image.as_ref().map_or(executable_image.entry, |image| image.entry);

    // File mappings keep their backing storage alive after close. Release
    // only our parsing views and descriptors, then retain the execution maps
    // so Rust destructors cannot unmap code the target is about to use.
    drop(interpreter);
    drop(interp_fd);
    drop(executable);
    drop(elf_file);
    executable_image.retain();
    if let Some(image) = interpreter_image {
        image.retain();
    }
    // SAFETY: the images and stack are initialized and retained. The caller
    // permits abandoning the current stack and entering the trusted image.
    unsafe { enter(entry, stack) }
}

/// Parses `inherited_auxv` (`AT_PHDR`, `AT_PHENT`, `AT_PHNUM`) to locate and
/// validate the launcher's own `PF_X` `PT_LOAD` range `[code_start, code_end)`.
///
/// # Safety
///
/// `inherited_auxv` must be the live kernel auxiliary vector for the current
/// process.
unsafe fn locate_launcher_code_range(inherited_auxv: &[AuxEntry]) -> (u64, u64) {
    let mut phdr_addr = None;
    let mut phent = None;
    let mut phnum = None;
    for entry in inherited_auxv {
        let Ok(key) = u32::try_from(entry.key) else {
            continue;
        };
        match key {
            AT_NULL => break,
            AT_PHDR => phdr_addr = Some(entry.value),
            AT_PHENT => phent = Some(entry.value),
            AT_PHNUM => phnum = Some(entry.value),
            _ => {}
        }
    }

    let phdr_addr = phdr_addr.expect("missing AT_PHDR");
    let phent = phent.expect("missing AT_PHENT");
    let phnum = phnum.expect("missing AT_PHNUM");
    assert_eq!(phent, image::PHDR_SIZE, "unexpected launcher AT_PHENT");
    assert!(phnum > 0, "empty launcher AT_PHNUM");

    let (region_base, region_limit) = LAUNCHER_REGION;
    assert!(
        phdr_addr >= region_base && phdr_addr < region_limit,
        "launcher AT_PHDR outside expected high fixed region"
    );
    let table_len = phnum.checked_mul(phent).expect("launcher program-header table overflow");
    // SAFETY: `AT_PHDR` points to `AT_PHNUM` contiguous `Elf64_Phdr` entries
    // mapped into this process by the kernel.
    let raw_phdrs =
        unsafe { slice::from_raw_parts(ptr::with_exposed_provenance::<u8>(phdr_addr), table_len) };
    let segments = SegmentTable::new(LittleEndian, Class::ELF64, raw_phdrs);

    let mut code_range: Option<(usize, usize)> = None;
    for seg in segments.iter() {
        assert!(
            !matches!(seg.p_type, abi::PT_INTERP | abi::PT_DYNAMIC | abi::PT_TLS),
            "launcher must not contain PT_INTERP, PT_DYNAMIC, or PT_TLS"
        );
        if seg.p_type == abi::PT_PHDR {
            assert_eq!(
                usize::try_from(seg.p_vaddr).ok(),
                Some(phdr_addr),
                "launcher must be loaded without relocation bias"
            );
        }
        if seg.p_type == abi::PT_LOAD && seg.p_memsz != 0 {
            let start = usize::try_from(seg.p_vaddr).expect("launcher PT_LOAD vaddr overflow");
            let memsz = usize::try_from(seg.p_memsz).expect("launcher PT_LOAD memsz overflow");
            let end = start.checked_add(memsz).expect("launcher PT_LOAD end overflow");
            assert!(
                start >= region_base && end <= region_limit,
                "launcher PT_LOAD outside expected high fixed region"
            );
            if (seg.p_flags & abi::PF_X) != 0 {
                assert!(code_range.is_none(), "launcher must have a single executable PT_LOAD");
                code_range = Some((start, end));
            }
        }
    }

    let (code_start, code_end) = code_range.expect("launcher missing executable PT_LOAD");
    let enter_addr = (enter as *const ()).addr();
    let handler_addr = sigsys::handler_addr();
    assert!(
        code_start <= enter_addr && enter_addr < code_end,
        "exec_elf entry stub outside launcher PF_X range"
    );
    assert!(
        code_start <= handler_addr && handler_addr < code_end,
        "SIGSYS handler outside launcher PF_X range"
    );
    (code_start as u64, code_end as u64)
}

/// Sets `/proc/self/comm` via `prctl(PR_SET_NAME)` to the basename of `execfn`
/// truncated to 15 bytes + NUL.
fn set_process_name(execfn: CStr<'_, Thin>) {
    let bytes = execfn.count().as_units();
    let basename = bytes.rsplit(|&b| b == b'/').next().unwrap_or(bytes);
    let mut comm = [0_u8; 16];
    let len = basename.len().min(15);
    comm[..len].copy_from_slice(&basename[..len]);
    // SAFETY: `PR_SET_NAME` reads a NUL-terminated buffer of up to 16 bytes.
    let _ = unsafe { syscalls::syscall!(Sysno::prctl, PR_SET_NAME, comm.as_ptr(), 0, 0, 0) };
}

// No Rust code may execute after replacing SP. The dynamic interpreter is
// entered like a program, not called like a function with a return address.
// #[naked] prevents a compiler-generated stack frame. Each stub starts with
// C function arguments in registers, installs the process-entry stack, and
// jumps. A normal call would put an unwanted return address on x86-64's stack.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn enter(_entry: usize, _stack: *mut usize) -> ! {
    // X0 holds entry and X1 holds the new SP. Match Linux start_thread by
    // zeroing GPRs before entry. X16 keeps the entry for BTI-compatible BR.
    core::arch::naked_asm!(
        "mov x16, x0",
        "mov sp, x1",
        "mov x0, xzr",
        "mov x1, xzr",
        "mov x2, xzr",
        "mov x3, xzr",
        "mov x4, xzr",
        "mov x5, xzr",
        "mov x6, xzr",
        "mov x7, xzr",
        "mov x8, xzr",
        "mov x9, xzr",
        "mov x10, xzr",
        "mov x11, xzr",
        "mov x12, xzr",
        "mov x13, xzr",
        "mov x14, xzr",
        "mov x15, xzr",
        "mov x17, xzr",
        "mov x18, xzr",
        "mov x19, xzr",
        "mov x20, xzr",
        "mov x21, xzr",
        "mov x22, xzr",
        "mov x23, xzr",
        "mov x24, xzr",
        "mov x25, xzr",
        "mov x26, xzr",
        "mov x27, xzr",
        "mov x28, xzr",
        "mov x29, xzr",
        "mov x30, xzr",
        "br x16",
    );
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn enter(_entry: usize, _stack: *mut usize) -> ! {
    // RDI holds entry and RSI holds the new RSP. RDX is the optional
    // dynamic-linker finalizer (an exit callback); there is none yet.
    // Match Linux ELF_PLAT_INIT by zeroing GPRs; RDI keeps the entry for JMP.
    core::arch::naked_asm!(
        "mov rsp, rsi",
        "xor eax, eax",
        "xor ebx, ebx",
        "xor ecx, ecx",
        "xor edx, edx",
        "xor esi, esi",
        "xor ebp, ebp",
        "xor r8d, r8d",
        "xor r9d, r9d",
        "xor r10d, r10d",
        "xor r11d, r11d",
        "xor r12d, r12d",
        "xor r13d, r13d",
        "xor r14d, r14d",
        "xor r15d, r15d",
        "jmp rdi"
    );
}
