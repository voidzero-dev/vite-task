//! Freestanding `SIGSYS` signal, `/proc/self/exe`, and custom syscall
//! interception runtime.

mod arch;
mod filter;
mod proc_exe;
mod signal;
mod stack_pool;
mod usermem;

use core::{
    cell::UnsafeCell,
    ffi::c_void,
    mem::size_of,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use fspy_nostd::{BorrowedFd, CStr, Error, Result, Thin};
#[cfg(target_arch = "x86_64")]
use linux_raw_sys::general::AT_FDCWD;
use linux_raw_sys::general::{
    CLONE_VFORK, CLONE_VM, O_NONBLOCK, O_PATH, SA_ONSTACK, SA_RESTORER, SA_SIGINFO, SIG_UNBLOCK,
    SIGSYS, SS_DISABLE, open_how, siginfo,
};
use syscalls::Sysno;

use self::arch::{
    HANDLER_BLOCK_MASK, KernelSigaction, SIGSYS_BIT, SyscallInvocation, UNBLOCKABLE_MASK, Ucontext,
    arg_i32, decode_syscall, replay, replay_unmasked, rt_sigaction, rt_sigprocmask,
    set_syscall_result, set_ucontext_sp, sigreturn_trampoline, sigsys_trampoline, syscall6,
    uc_sigmask_mut, ucontext_sp,
};
use crate::{Syscall, SyscallAction, SyscallInterceptor};

struct SyncCell<T>(UnsafeCell<T>);

// SAFETY: Written once during single-threaded `install` before arming seccomp
// and published with `INTERCEPTOR_INSTALLED` Release/Acquire ordering.
unsafe impl<T> Sync for SyncCell<T> {}

static INSTALLED: AtomicBool = AtomicBool::new(false);
static INTERCEPTOR_INSTALLED: AtomicBool = AtomicBool::new(false);
static INTERCEPTOR: SyncCell<Option<SyscallInterceptor>> = SyncCell(UnsafeCell::new(None));

/// Returns the runtime address of [`sigsys_trampoline`] for startup `PF_X`
/// bounds verification.
#[must_use]
pub fn handler_addr() -> usize {
    (sigsys_trampoline as *const ()).addr()
}

/// Borrows a user C string directly from `addr`.
///
/// # Safety
///
/// See [`usermem::user_cstr`].
pub unsafe fn user_cstr<'a>(addr: usize) -> Result<CStr<'a, Thin>> {
    // SAFETY: forwarded from caller contract.
    unsafe { usermem::user_cstr(addr) }
}

/// Copies a user `Copy` value.
///
/// # Safety
///
/// `T` must be valid for any bit pattern, and `addr` must satisfy
/// [`usermem::read_user`].
pub unsafe fn copy_user_value<T: Copy>(addr: usize) -> Result<T> {
    // SAFETY: forwarded from caller contract.
    unsafe { usermem::read_user(addr) }
}

/// Computes the calling thread's virtualized signal mask from `ucontext.uc_sigmask`.
pub fn virtual_sigmask_from(raw_uc_sigmask: u64) -> u64 {
    signal::virtual_sigmask_from(raw_uc_sigmask)
}

/// Returns `true` when executing inside a `CLONE_VM` child sharing its parent's
/// address space.
pub fn is_shared_vm_child() -> bool {
    signal::is_shared_vm_child()
}

/// Returns `true` when the target's virtualized `SIGSYS` disposition is `SIG_IGN`.
pub fn is_sigsys_ignored() -> bool {
    signal::is_sigsys_ignored()
}

/// Installs the `SIGSYS` runtime, `/proc/self/exe` virtualization, and
/// optional [`SyscallInterceptor`] for an open executable descriptor.
///
/// # Errors
///
/// Returns an error if the runtime is already installed, the executable path
/// cannot be stored, `SIGSYS` setup fails, or seccomp filter installation
/// fails.
///
/// # Safety
///
/// Must be called once in a fresh single-threaded process before entering the
/// target executable. `code_range` must be the launcher's own `PF_X` `PT_LOAD`
/// range `[code_start, code_end)` that remains mapped for the lifetime of the
/// process.
pub unsafe fn install(
    code_range: (u64, u64),
    fd: BorrowedFd<'_>,
    fallback_path: CStr<'_, Thin>,
    min_sigstksz: usize,
    page_size: usize,
    interceptor: Option<SyscallInterceptor>,
) -> Result<()> {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return Err(Error::INVAL);
    }

    // Prime cached page size and private SIGSYS stack pool (honoring
    // `AT_MINSIGSTKSZ`) before arming seccomp.
    let _ = fspy_nostd::param::page_size();
    stack_pool::init_and_warm(min_sigstksz, page_size);

    let bump = fspy_nostd_alloc::pooled_bump();
    let exe_path = match proc_exe::resolve_exe_path_for_fd(&bump, fd, fallback_path) {
        Ok(path) => path,
        Err(err) => {
            INSTALLED.store(false, Ordering::Release);
            return Err(err);
        }
    };
    if let Err(err) = proc_exe::init(&exe_path) {
        INSTALLED.store(false, Ordering::Release);
        return Err(err);
    }

    // Publish the interceptor before arming seccomp / entering target.
    // SAFETY: Single-threaded initialization prior to `SECCOMP_SET_MODE_FILTER`.
    unsafe { *INTERCEPTOR.0.get() = interceptor };
    INTERCEPTOR_INSTALLED.store(interceptor.is_some(), Ordering::Release);

    // Install our freestanding `SIGSYS` naked trampoline before arming seccomp,
    // preserving any inherited `SIGSYS` disposition (e.g. `SIG_IGN`) in the
    // virtualized table.
    //
    // `sa_mask` (with `SA_NODEFER` omitted) blocks all maskable signals
    // including `SIGSYS` while Phase 1 runs on `stack_pool` so neither async
    // signals (such as Go's `SIGURG`) nor async `SIGSYS` (`kill`/`tgkill`) can
    // preempt the handler while `SP` is switched onto `stack_pool`. Every
    // syscall executed during Phase 1 lives inside the launcher's allowlisted
    // `PF_X` range, and `SIGSYS` is unmasked before re-entering target code
    // (`deliver_sigsys_now`, `replay_unmasked`, `sigsys_masked_wait`, or
    // `rt_sigreturn`).
    let our_action = KernelSigaction {
        sa_handler: handler_addr(),
        sa_flags: (SA_SIGINFO | SA_RESTORER | SA_ONSTACK) as usize,
        sa_restorer: (sigreturn_trampoline as *const ()).addr(),
        sa_mask: HANDLER_BLOCK_MASK,
    };
    let mut prev_action = KernelSigaction::default();
    // SAFETY: `our_action` and `prev_action` are valid stack-local structs.
    let rc =
        unsafe { rt_sigaction(SIGSYS.cast_signed(), &raw const our_action, &raw mut prev_action) };
    if rc < 0 {
        INTERCEPTOR_INSTALLED.store(false, Ordering::Release);
        INSTALLED.store(false, Ordering::Release);
        #[expect(clippy::cast_possible_truncation, reason = "Linux errno fits in i32")]
        return Err(Error::from_raw_os_error((-rc) as i32));
    }

    // Ensure `SIGSYS` is unblocked in the kernel mask before arming seccomp,
    // while recording if the parent had passed `SIGSYS` blocked.
    let sigsys_mask = SIGSYS_BIT;
    let mut prev_mask: u64 = 0;
    // SAFETY: `sigsys_mask` and `prev_mask` are valid stack-local `u64`s.
    let rc = unsafe { rt_sigprocmask(SIG_UNBLOCK, &raw const sigsys_mask, &raw mut prev_mask) };
    if rc < 0 {
        // SAFETY: Best-effort restore of previous `SIGSYS` disposition on failure.
        let _ =
            unsafe { rt_sigaction(SIGSYS.cast_signed(), &raw const prev_action, ptr::null_mut()) };
        INTERCEPTOR_INSTALLED.store(false, Ordering::Release);
        INSTALLED.store(false, Ordering::Release);
        #[expect(clippy::cast_possible_truncation, reason = "Linux errno fits in i32")]
        return Err(Error::from_raw_os_error((-rc) as i32));
    }

    signal::init(prev_action, (prev_mask & SIGSYS_BIT) != 0);

    let extra_syscalls = interceptor.map_or(&[][..], |i| i.syscalls);
    if let Err(err) = filter::install_seccomp_filter(code_range.0, code_range.1, extra_syscalls) {
        // SAFETY: Best-effort restore of previous `SIGSYS` disposition on failure.
        let _ =
            unsafe { rt_sigaction(SIGSYS.cast_signed(), &raw const prev_action, ptr::null_mut()) };
        INTERCEPTOR_INSTALLED.store(false, Ordering::Release);
        INSTALLED.store(false, Ordering::Release);
        return Err(err);
    }

    Ok(())
}

/// Entry target from [`sigsys_trampoline`] for non-seccomp and foreign-seccomp
/// `SIGSYS` deliveries.
pub unsafe extern "C" fn sigsys_foreign(signum: i32, info: *mut siginfo, ucontext: *mut Ucontext) {
    // SAFETY: Kernel-supplied signal frame pointers from `sigsys_trampoline`.
    unsafe { signal::forward_non_seccomp_sigsys(signum, info, ucontext) };
}

const PHASE2_NONE: usize = 0;
const PHASE2_DELIVER_PENDING_SIGSYS: usize = 1;
const PHASE2_REPLAY: usize = 2;
const PHASE2_REPLAY_UNMASKED: usize = 3;
const PHASE2_FORK_REPLAY: usize = 4;
const PHASE2_EXECVE: usize = 5;
const PHASE2_VFORK_ALTSTACK: usize = 6;

/// Compact 80-byte Phase 1 -> Phase 2 handoff packet allocated on the entry
/// stack by [`sigsys_trampoline`].
#[repr(C)]
pub struct Phase2Packet {
    inv: SyscallInvocation,
    /// `orig_mask` (`ReplayUnmasked`), `parent_blocked` (`ForkReplay`), or
    /// `cleanup_fn` address (`Execve`).
    aux0: usize,
    /// `cleanup_ctx` address (`Execve`).
    aux1: usize,
    /// One of `PHASE2_*`; offset 72 is checked directly by `sigsys_trampoline`.
    kind: usize,
}

const _: () = {
    assert!(size_of::<Phase2Packet>() == 80);
    assert!(core::mem::offset_of!(Phase2Packet, kind) == 72);
};

/// Entry target from [`sigsys_trampoline`] for masked-wait syscalls, running
/// directly on the entry stack so any `SA_ONSTACK` signal sees
/// `on_sig_stack(SP) == true`.
pub unsafe extern "C" fn sigsys_masked_wait(ucontext_ptr: *mut Ucontext) {
    // SAFETY: `sigsys_trampoline` checked `ucontext_ptr` is non-null.
    let ucontext = unsafe { &mut *ucontext_ptr };
    let inv = decode_syscall(ucontext);
    match Sysno::new(inv.nr) {
        // SAFETY: Each helper matches the corresponding Linux syscall ABI and
        // completes `ucontext` (before delivering any pending `SIGSYS`).
        Some(Sysno::rt_sigsuspend) => unsafe {
            signal::handle_masked_wait(inv, ucontext, 0, 1, false);
        },
        // SAFETY: Matches Linux `ppoll` ABI.
        Some(Sysno::ppoll) => unsafe {
            signal::handle_masked_wait(inv, ucontext, 3, 4, true);
        },
        // SAFETY: Matches Linux `pselect6` ABI.
        Some(Sysno::pselect6) => unsafe {
            signal::handle_pselect6(inv, ucontext);
        },
        // SAFETY: Matches Linux `epoll_pwait` / `epoll_pwait2` ABI.
        Some(Sysno::epoll_pwait | Sysno::epoll_pwait2) => unsafe {
            signal::handle_masked_wait(inv, ucontext, 4, 5, true);
        },
        _ => unreachable!("sigsys_trampoline only routes masked-wait syscalls here"),
    }
}

/// Phase 1 entry called by [`sigsys_trampoline`] on a private 64 KiB stack
/// while maskable asynchronous signals are blocked by `our_action.sa_mask`.
pub unsafe extern "C" fn sigsys_phase1(ucontext_ptr: *mut Ucontext, packet_ptr: *mut Phase2Packet) {
    // SAFETY: `sigsys_trampoline` guarantees both pointers are valid and writable.
    let ucontext = unsafe { &mut *ucontext_ptr };
    // SAFETY: `packet_ptr` points to the 80-byte slot on the entry stack.
    let packet = unsafe { &mut *packet_ptr };
    let inv = decode_syscall(ucontext);
    // SAFETY: Dispatches Phase 1 on the private stack.
    unsafe { dispatch_phase1(inv, ucontext, packet) };
}

/// Phase 2 entry called by [`sigsys_trampoline`] back on the entry stack AFTER
/// the private 64 KiB stack has been released, only when `packet.kind != 0`.
#[inline(never)]
pub unsafe extern "C" fn sigsys_phase2(
    ucontext_ptr: *mut Ucontext,
    packet_ptr: *const Phase2Packet,
    kind: usize,
) {
    // SAFETY: `packet_ptr` points to the populated `Phase2Packet` on the entry stack.
    let packet = unsafe { &*packet_ptr };
    let result = match kind {
        PHASE2_DELIVER_PENDING_SIGSYS => {
            // SAFETY: Delivering pending `SIGSYS` on entry stack after unmasking.
            unsafe { signal::deliver_pending_sigsys(&mut *ucontext_ptr) };
            return;
        }
        PHASE2_REPLAY => {
            // SAFETY: Replays non-blocking syscall directly from `&packet.inv`
            // without copying `SyscallInvocation` onto the entry stack.
            unsafe {
                syscall6(
                    packet.inv.nr,
                    packet.inv.args[0],
                    packet.inv.args[1],
                    packet.inv.args[2],
                    packet.inv.args[3],
                    packet.inv.args[4],
                    packet.inv.args[5],
                )
            }
        }
        PHASE2_REPLAY_UNMASKED => {
            // SAFETY: Replays blocking syscall with caller's mask restored.
            unsafe {
                replay_unmasked(&packet.inv, packet.aux0 as u64, HANDLER_BLOCK_MASK, &*ucontext_ptr)
            }
        }
        PHASE2_FORK_REPLAY => {
            // SAFETY: Replays `fork`/`clone` (new `mm`) and updates child state on `rc == 0`.
            unsafe { finish_fork_replay(&mut *ucontext_ptr, &packet.inv, packet.aux0 != 0) }
        }
        PHASE2_EXECVE => {
            // SAFETY: Runs replacement `SYS_execve` after releasing `stack_pool`.
            unsafe { finish_execve(packet) }
        }
        PHASE2_VFORK_ALTSTACK => {
            // SAFETY: Passes raw `ucontext_ptr` so full kernel `rt_sigframe`
            // copies preserve provenance past the Rust `Ucontext` prefix.
            unsafe { finish_vfork_altstack(ucontext_ptr.cast_const(), packet) }
        }
        _ => return,
    };

    // SAFETY: `sigsys_trampoline` guarantees `ucontext_ptr` is valid and writable.
    let ucontext = unsafe { &mut *ucontext_ptr };
    set_syscall_result(ucontext, result);
    *uc_sigmask_mut(ucontext) &= !SIGSYS_BIT;
}

#[inline(never)]
unsafe fn finish_vfork_altstack(ucontext_ptr: *const Ucontext, packet: &Phase2Packet) -> isize {
    let Some(altstack_top) = stack_pool::acquire_stack() else {
        return arch::neg_errno(linux_raw_sys::errno::ENOMEM);
    };
    let orig_child_stack = packet.inv.args[1];
    let uc_addr = ucontext_ptr.addr();
    let child_sigmask = packet.aux0 as u64;
    let altstack_size = stack_pool::stack_size();
    // SAFETY: `ucontext_ptr` is the live kernel signal frame and `altstack_top`
    // owns `altstack_size` writable bytes below it exclusively until `clone`
    // returns in the parent.
    let Some(child_sigreturn_sp) = (unsafe {
        arch::prepare_vfork_child_sigframe(
            ucontext_ptr,
            altstack_top,
            altstack_size,
            orig_child_stack,
            child_sigmask,
        )
    }) else {
        stack_pool::release_stack(altstack_top);
        return arch::neg_errno(linux_raw_sys::errno::ENOMEM);
    };
    // SAFETY: `ucontext_ptr` is a valid kernel `Ucontext`.
    let ucontext = unsafe { &*ucontext_ptr };
    let uc_sp = ucontext_sp(ucontext);
    // If our live `SIGSYS` frame sits on a separate user `sigaltstack` that
    // does not contain `orig_child_stack`, the child cannot overwrite it.
    let on_separate_altstack = {
        let ss_base = ucontext.uc_stack.ss_sp.addr();
        let ss_len = ucontext.uc_stack.ss_size;
        let ss_enabled = (ucontext.uc_stack.ss_flags & SS_DISABLE.cast_signed()) == 0 && ss_len > 0;
        let ss_end = ss_base.saturating_add(ss_len);
        ss_enabled
            && uc_addr >= ss_base
            && uc_addr < ss_end
            && !(orig_child_stack > ss_base && orig_child_stack <= ss_end)
    };
    // When `orig_child_stack` sits directly above our live `SIGSYS` frame
    // (e.g. glibc `AArch64` `vfork()` passing `child_stack = parent_sp`),
    // acquire a second pooled stack to preserve and restore the parent's frame.
    let save_top =
        if !on_separate_altstack && orig_child_stack <= uc_sp && uc_addr < orig_child_stack {
            let Some(top) = stack_pool::acquire_stack() else {
                stack_pool::release_stack(altstack_top);
                return arch::neg_errno(linux_raw_sys::errno::ENOMEM);
            };
            top
        } else {
            0
        };
    // SAFETY: `packet.inv` holds the trapped `clone(CLONE_VM | CLONE_VFORK,
    // non_null_stack)` arguments; `child_sigreturn_sp` points to our prepared
    // `rt_sigframe` inside `altstack_top`, and optional `save_top` owns
    // `altstack_size` writable bytes until `clone` returns in the parent.
    let rc = unsafe {
        arch::replay_vfork_with_altstack(
            &raw const packet.inv,
            child_sigreturn_sp,
            altstack_size,
            save_top,
        )
    };
    if save_top != 0 {
        stack_pool::release_stack(save_top);
    }
    stack_pool::release_stack(altstack_top);
    if rc > 0 {
        #[expect(clippy::cast_possible_truncation, reason = "Linux pid_t fits in i32")]
        let child_pid = rc as i32;
        signal::clear_pid_slots(child_pid);
        if INTERCEPTOR_INSTALLED.load(Ordering::Acquire) {
            // SAFETY: `INTERCEPTOR` was written once before `Release` store.
            let hook = unsafe { (*INTERCEPTOR.0.get()).and_then(|ic| ic.on_vfork_return) };
            if let Some(on_vfork_return) = hook {
                // SAFETY: `on_vfork_return` was validated inside `PF_X`, and `rc > 0`
                // means the vfork child has already left the parent's `mm_struct`.
                unsafe { on_vfork_return(child_pid) };
            }
        }
    }
    rc
}

#[inline(never)]
unsafe fn finish_fork_replay(
    ucontext: &mut Ucontext,
    inv: &SyscallInvocation,
    parent_blocked: bool,
) -> isize {
    // For `clone` without `CLONE_VM` that supplied a custom `child_stack != 0`,
    // replay with `child_stack = 0` so the child starts on a private COW copy
    // of our entry stack, runs `on_fork_child`, and then has `ucontext` SP set
    // to `orig_child_stack` so `rt_sigreturn` atomically switches to the
    // caller's requested child stack with every register and TLS intact.
    let orig_child_stack = if inv.nr == Sysno::clone as usize { inv.args[1] } else { 0 };
    let mut replay_inv = *inv;
    if orig_child_stack != 0 {
        replay_inv.args[1] = 0;
    }
    // SAFETY: Replays `fork` / `clone` without `CLONE_VM`.
    let rc = unsafe { replay(replay_inv) };
    if rc == 0 {
        signal::on_fork_child(parent_blocked);
        if orig_child_stack != 0 {
            set_ucontext_sp(ucontext, orig_child_stack);
        }
    }
    rc
}

#[inline(never)]
unsafe fn finish_execve(packet: &Phase2Packet) -> isize {
    // Clear our TID slot before `SYS_execve` so a successful `vfork` /
    // `posix_spawn` exec leaves ZERO entries in the parent's `TID_SLOTS` table
    // (and `stack_pool` has already been released above!).
    let saved_tid = signal::take_current_tid_entry();
    // SAFETY: `packet.inv` holds `SYS_execve(prog, argv, envp)` pointers valid in
    // caller-owned scratch storage until `cleanup` runs on failure.
    let rc = unsafe {
        syscall6(
            Sysno::execve as usize,
            packet.inv.args[0],
            packet.inv.args[1],
            packet.inv.args[2],
            0,
            0,
            0,
        )
    };
    // `SYS_execve` failed: restore TID entry and release scratch/fd.
    signal::restore_current_tid_entry(saved_tid);
    if packet.aux0 != 0 {
        // SAFETY: `aux0` was stored from `SyscallAction::Execve`'s cleanup fn pointer.
        let cleanup_fn: unsafe fn(*mut c_void) =
            unsafe { core::mem::transmute::<usize, unsafe fn(*mut c_void)>(packet.aux0) };
        let mut cb = CleanupContext {
            cleanup_fn,
            cleanup_ctx: ptr::with_exposed_provenance_mut(packet.aux1),
        };
        // SAFETY: Run interceptor cleanup on the private stack so complex
        // destructors never overflow a tight `vfork` stack.
        unsafe {
            stack_pool::run_on_private_stack((&raw mut cb).cast(), run_cleanup_on_stack);
        }
    }
    rc
}

struct CleanupContext {
    cleanup_fn: unsafe fn(*mut c_void),
    cleanup_ctx: *mut c_void,
}

extern "C" fn run_cleanup_on_stack(raw: *mut c_void) {
    // SAFETY: `finish_execve` passes a valid `*mut CleanupContext`.
    let cb = unsafe { &*(raw.cast::<CleanupContext>()) };
    // SAFETY: Invokes the interceptor's failure cleanup callback once.
    unsafe { (cb.cleanup_fn)(cb.cleanup_ctx) };
}

#[inline]
const fn finish_in_phase1(ucontext: &mut Ucontext, packet: &mut Phase2Packet, rc: isize) {
    set_syscall_result(ucontext, rc);
    *uc_sigmask_mut(ucontext) &= !SIGSYS_BIT;
    packet.kind = PHASE2_NONE;
}

/// Runs Phase 1 of trapped-syscall dispatch on the private `SIGSYS` stack while
/// maskable signals are blocked, either completing the syscall in `ucontext`
/// (`PHASE2_NONE`) or populating `packet` for Phase 2 on the entry stack.
///
/// # Safety
///
/// `inv` must be decoded from the live `SECCOMP_RET_TRAP` `ucontext`.
#[expect(
    clippy::too_many_lines,
    reason = "Phase 1 dispatch keeps signal, /proc/self/exe, interceptor, and replay stages in a single linear pipeline"
)]
unsafe fn dispatch_phase1(
    mut inv: SyscallInvocation,
    ucontext: &mut Ucontext,
    packet: &mut Phase2Packet,
) {
    let Some(sysno) = Sysno::new(inv.nr) else {
        packet.inv = inv;
        packet.kind = PHASE2_REPLAY;
        return;
    };

    // 1. Signal and fork virtualization (not forwarded to `interceptor`).
    // SAFETY: Arguments match the corresponding Linux syscall ABIs.
    unsafe {
        match sysno {
            Sysno::rt_sigaction => {
                finish_in_phase1(ucontext, packet, signal::handle_rt_sigaction(inv));
                return;
            }
            Sysno::rt_sigprocmask => {
                match signal::handle_rt_sigprocmask(inv, ucontext) {
                    signal::SigprocmaskOutcome::Done(rc) => {
                        finish_in_phase1(ucontext, packet, rc);
                    }
                    signal::SigprocmaskOutcome::DeliverPendingSigsys(rc) => {
                        set_syscall_result(ucontext, rc);
                        *uc_sigmask_mut(ucontext) &= !SIGSYS_BIT;
                        packet.kind = PHASE2_DELIVER_PENDING_SIGSYS;
                    }
                }
                return;
            }
            Sysno::rt_sigpending => {
                finish_in_phase1(ucontext, packet, signal::handle_rt_sigpending(inv));
                return;
            }
            Sysno::clone => {
                const CLONE_VM_VFORK: usize = (CLONE_VM | CLONE_VFORK) as usize;
                if (inv.args[0] & CLONE_VM_VFORK) == CLONE_VM_VFORK && inv.args[1] != 0 {
                    let child_mask = ucontext.uc_sigmask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
                    packet.inv = inv;
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "64-bit Linux usize == u64"
                    )]
                    {
                        packet.aux0 = child_mask as usize;
                    }
                    packet.kind = PHASE2_VFORK_ALTSTACK;
                    return;
                }
                packet.inv = inv;
                packet.aux0 = usize::from(signal::is_current_tid_blocked());
                packet.kind = PHASE2_FORK_REPLAY;
                return;
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::fork => {
                packet.inv = inv;
                packet.aux0 = usize::from(signal::is_current_tid_blocked());
                packet.kind = PHASE2_FORK_REPLAY;
                return;
            }
            _ => {}
        }
    }

    // 2. `readlink`/`readlinkat` matching `/proc/*/exe` -> handled directly.
    // SAFETY: Arguments match the corresponding Linux readlink syscall ABI.
    unsafe {
        match sysno {
            Sysno::readlinkat => {
                if let Some(rc) = proc_exe::try_handle_readlink(
                    arg_i32(inv.args[0]),
                    inv.args[1],
                    inv.args[2],
                    inv.args[3],
                ) {
                    finish_in_phase1(ucontext, packet, rc);
                    return;
                }
            }
            #[cfg(target_arch = "x86_64")]
            Sysno::readlink => {
                if let Some(rc) =
                    proc_exe::try_handle_readlink(AT_FDCWD, inv.args[0], inv.args[1], inv.args[2])
                {
                    finish_in_phase1(ucontext, packet, rc);
                    return;
                }
            }
            _ => {}
        }
    }

    // 3. Path-following syscalls -> rewrite `/proc/*/exe` in place in `inv`,
    //    retaining any already-read `open_how` for the unmasked-replay check.
    // SAFETY: Arguments come from the trapped syscall for `sysno`.
    let prefetched_open_how = unsafe { proc_exe::rewrite_path_arg(&mut inv, sysno) };

    // 4. Optional caller interceptor (sees `/proc/*/exe`-rewritten `inv`, with
    //    lazy access to virtualized `sigmask` and `is_shared_vm_child`).
    if INTERCEPTOR_INSTALLED.load(Ordering::Acquire) {
        // SAFETY: `INTERCEPTOR` was written once before `Release` store.
        if let Some(interceptor) = unsafe { *INTERCEPTOR.0.get() }
            && interceptor.syscalls.contains(&sysno)
        {
            let call = Syscall { sysno, args: inv.args, raw_uc_sigmask: ucontext.uc_sigmask };
            // SAFETY: `call` holds the live (and rewritten) trapped arguments.
            match unsafe { (interceptor.handle)(&call) } {
                Ok(SyscallAction::Replay) => {}
                Ok(SyscallAction::Return(rc)) => {
                    finish_in_phase1(ucontext, packet, rc);
                    return;
                }
                Ok(SyscallAction::Execve { prog, argv, envp, cleanup }) => {
                    packet.inv = SyscallInvocation {
                        nr: Sysno::execve as usize,
                        args: [prog.addr(), argv.addr(), envp.addr(), 0, 0, 0],
                    };
                    if let Some((cleanup_fn, cleanup_ctx)) = cleanup {
                        packet.aux0 = (cleanup_fn as *const ()).addr();
                        packet.aux1 = cleanup_ctx.addr();
                    } else {
                        packet.aux0 = 0;
                        packet.aux1 = 0;
                    }
                    packet.kind = PHASE2_EXECVE;
                    return;
                }
                Err(err) => {
                    finish_in_phase1(ucontext, packet, -(err.raw_os_error() as isize));
                    return;
                }
            }
        }
    }

    // 5. Return to the entry stack to replay the (possibly rewritten) syscall.
    //    Unmask signals for potentially blocking `open*` (e.g. FIFO without
    //    `O_NONBLOCK`) and directly-replayed `execve`/`execveat`, unless all
    //    maskable signals were already blocked by the caller.
    packet.inv = inv;
    // SAFETY: Inspects `openat2`'s `open_how` safely via `usermem` if not already prefetched.
    if unsafe { needs_unmasked_replay(sysno, &inv, prefetched_open_how) } {
        let orig_mask = if matches!(sysno, Sysno::execve | Sysno::execveat) {
            // Preserve the caller's exact virtualized signal mask (including
            // `SIGSYS`) across direct `execve`/`execveat`; `SYS_execve*` runs
            // from allowlisted `PF_X`, so blocking `SIGSYS` cannot self-trap,
            // and `replay_unmasked` restores `HANDLER_BLOCK_MASK` on failure.
            signal::virtual_sigmask_from(ucontext.uc_sigmask)
        } else {
            ucontext.uc_sigmask & !(SIGSYS_BIT | UNBLOCKABLE_MASK)
        };
        if orig_mask != HANDLER_BLOCK_MASK {
            #[expect(clippy::cast_possible_truncation, reason = "64-bit Linux usize == u64")]
            {
                packet.aux0 = orig_mask as usize;
            }
            packet.kind = PHASE2_REPLAY_UNMASKED;
            return;
        }
    }
    packet.kind = PHASE2_REPLAY;
}

/// Returns `true` if replaying `sysno` on the entry stack should temporarily
/// restore the caller's signal mask (`open`/`openat`/`openat2` without
/// `O_NONBLOCK`/`O_PATH`, or `execve`/`execveat`).
///
/// # Safety
///
/// `inv` must contain the arguments from a trapped `sysno` syscall.
#[expect(clippy::cast_possible_truncation, reason = "Linux open flags live in low 32 bits")]
unsafe fn needs_unmasked_replay(
    sysno: Sysno,
    inv: &SyscallInvocation,
    prefetched_open_how: Option<open_how>,
) -> bool {
    const NON_BLOCKING_FLAGS: u32 = O_NONBLOCK | O_PATH;
    match sysno {
        #[cfg(target_arch = "x86_64")]
        Sysno::open => ((inv.args[1] as u32) & NON_BLOCKING_FLAGS) == 0,
        Sysno::openat => ((inv.args[2] as u32) & NON_BLOCKING_FLAGS) == 0,
        Sysno::openat2 => {
            let how = if let Some(how) = prefetched_open_how {
                how
            } else {
                let how_ptr = inv.args[2];
                let size = inv.args[3];
                if how_ptr == 0 || size < size_of::<open_how>() {
                    return false;
                }
                // SAFETY: `open_how` is a plain `#[repr(C)]` struct of `u64` fields.
                let Ok(how) = (unsafe { usermem::read_user::<open_how>(how_ptr) }) else {
                    return false;
                };
                how
            };
            (how.flags & u64::from(NON_BLOCKING_FLAGS)) == 0
        }
        Sysno::execve | Sysno::execveat => true,
        _ => false,
    }
}
