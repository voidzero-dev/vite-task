//! Architecture-specific Linux ABI primitives for `SIGSYS` interception.
//!
//! Provides:
//! - The freestanding `rt_sigreturn` restorer trampoline (`SA_RESTORER`).
//! - Kernel `ucontext_t` / `sigaction` layouts and register accessors for
//!   x86-64 and `AArch64`.
//! - Raw 6-argument syscall replay and signal/PID helpers executed directly
//!   from the launcher's allowlisted `PF_X` code range.

use core::ffi::c_void;

use linux_raw_sys::general::{SIGKILL, SIGSTOP, SIGSYS};
use syscalls::Sysno;

/// Bitmask for `SIGSYS` (signal 31) in a 64-bit Linux kernel `sigset_t`.
pub const SIGSYS_BIT: u64 = 1_u64 << (SIGSYS - 1);

/// Unblockable signals stripped by the Linux kernel from `sa_mask` and
/// signal masks (`SIGKILL` and `SIGSTOP`).
pub const UNBLOCKABLE_MASK: u64 = (1_u64 << (SIGKILL - 1)) | (1_u64 << (SIGSTOP - 1));

/// Mask blocking all maskable signals (including `SIGSYS`) while the
/// dispatcher runs on a private pooled stack or modifies virtualized signal
/// state.
pub const HANDLER_BLOCK_MASK: u64 = !UNBLOCKABLE_MASK;

/// Size of the Linux kernel `sigset_t` on 64-bit architectures (8 bytes = 64 signals).
pub const KERNEL_SIGSET_SIZE: usize = core::mem::size_of::<u64>();

/// Native seccomp audit architecture constant (`seccomp_data.arch`).
#[cfg(target_arch = "x86_64")]
pub const AUDIT_ARCH_NATIVE: u32 = linux_raw_sys::ptrace::AUDIT_ARCH_X86_64;

/// Native seccomp audit architecture constant (`seccomp_data.arch`).
#[cfg(target_arch = "aarch64")]
pub const AUDIT_ARCH_NATIVE: u32 = linux_raw_sys::ptrace::AUDIT_ARCH_AARCH64;

/// Linux kernel `struct sigaction` layout used by `rt_sigaction` on both
/// x86-64 and `AArch64`.
#[expect(clippy::struct_field_names, reason = "matches Linux UAPI struct sigaction field names")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct KernelSigaction {
    pub sa_handler: usize,
    pub sa_flags: usize,
    pub sa_restorer: usize,
    pub sa_mask: u64,
}

const _: () = {
    use core::mem::{offset_of, size_of};

    use linux_raw_sys::general::siginfo;
    assert!(size_of::<KernelSigaction>() == 32);
    assert!(offset_of!(KernelSigaction, sa_handler) == 0);
    assert!(offset_of!(KernelSigaction, sa_flags) == 8);
    assert!(offset_of!(KernelSigaction, sa_restorer) == 16);
    assert!(offset_of!(KernelSigaction, sa_mask) == 24);
    // `sigsys_trampoline` reads `si_errno` at +4 and `si_code` at +8 directly.
    assert!(offset_of!(siginfo, __bindgen_anon_1.__bindgen_anon_1.si_signo) == 0);
    assert!(offset_of!(siginfo, __bindgen_anon_1.__bindgen_anon_1.si_errno) == 4);
    assert!(offset_of!(siginfo, __bindgen_anon_1.__bindgen_anon_1.si_code) == 8);
};

/// Decoded syscall invocation captured from a `SIGSYS` `ucontext_t`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct SyscallInvocation {
    pub nr: usize,
    pub args: [usize; 6],
}

const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(size_of::<SyscallInvocation>() == 56);
    assert!(offset_of!(SyscallInvocation, nr) == 0);
    assert!(offset_of!(SyscallInvocation, args) == 8);
};

/// Linux `stack_t` (`struct sigaltstack`) inside `ucontext_t`.
#[repr(C)]
pub struct StackT {
    pub ss_sp: *mut c_void,
    pub ss_flags: i32,
    pub _pad: i32,
    pub ss_size: usize,
}

#[cfg(target_arch = "x86_64")]
mod imp {
    use core::{arch::naked_asm, ffi::c_void, mem::size_of, ptr};

    use linux_raw_sys::{
        errno::ENOMEM,
        general::{SIG_SETMASK, SYS_SECCOMP},
    };
    use syscalls::Sysno;

    use super::{StackT, SyscallInvocation};
    use crate::sigsys::filter::TRAP_TAG;

    pub const REG_R8: usize = 0;
    pub const REG_R9: usize = 1;
    pub const REG_R10: usize = 2;
    pub const REG_RDI: usize = 8;
    pub const REG_RSI: usize = 9;
    pub const REG_RDX: usize = 12;
    pub const REG_RAX: usize = 13;
    pub const REG_RSP: usize = 15;

    #[repr(C)]
    pub struct Mcontext {
        pub gregs: [u64; 23],
        pub fpregs: *mut c_void,
        pub _reserved: [u64; 8],
    }

    #[expect(clippy::struct_field_names, reason = "matches Linux UAPI struct ucontext field names")]
    #[repr(C)]
    pub struct Ucontext {
        pub uc_flags: u64,
        pub uc_link: *mut Self,
        pub uc_stack: StackT,
        pub uc_mcontext: Mcontext,
        pub uc_sigmask: u64,
    }

    const UC_STACK_OFFSET: usize = 16;
    const UC_GREGS_OFFSET: usize = 40;
    const UC_FPREGS_OFFSET: usize = UC_GREGS_OFFSET + 23 * 8;
    const UC_SIGMASK_OFFSET: usize = 296;
    const UC_NR_OFFSET: usize = UC_GREGS_OFFSET + REG_RAX * 8;

    const _: () = {
        use core::mem::offset_of;
        assert!(offset_of!(Ucontext, uc_flags) == 0);
        assert!(offset_of!(Ucontext, uc_link) == 8);
        assert!(offset_of!(Ucontext, uc_stack) == UC_STACK_OFFSET);
        assert!(offset_of!(Ucontext, uc_mcontext) == UC_GREGS_OFFSET);
        assert!(
            offset_of!(Ucontext, uc_mcontext) + offset_of!(Mcontext, fpregs) == UC_FPREGS_OFFSET
        );
        assert!(offset_of!(Ucontext, uc_sigmask) == UC_SIGMASK_OFFSET);
        assert!(UC_NR_OFFSET == offset_of!(Ucontext, uc_mcontext) + REG_RAX * 8);
    };

    #[unsafe(naked)]
    pub unsafe extern "C" fn sigsys_trampoline() {
        // Entry from kernel `SA_SIGINFO` delivery:
        //   rdi = signum, rsi = *mut siginfo, rdx = *mut Ucontext,
        //   [rsp] = sigreturn_trampoline (rsp % 16 == 8).
        //
        // Performs foreign-SIGSYS and masked-wait checks with ZERO stack bytes,
        // then allocates only an 88-byte frame (80-byte `Phase2Packet` + align)
        // on the entry stack and switches immediately to a private 64 KiB stack
        // before running Rust Phase 1. Kernel `rt_sigreturn` restores all GPRs
        // from `ucontext`, so `rbx`/`r12`/`r13` need not be saved.
        naked_asm!(
            "test rsi, rsi",
            "jz 2f",
            "cmp dword ptr [rsi + 8], {sys_seccomp}",
            "jne 2f",
            "cmp dword ptr [rsi + 4], {trap_tag}",
            "jne 2f",
            "test rdx, rdx",
            "jz 3f",
            "mov rax, [rdx + {uc_nr_off}]",
            "cmp rax, {nr_rt_sigsuspend}",
            "je 4f",
            "cmp rax, {nr_ppoll}",
            "je 4f",
            "cmp rax, {nr_pselect6}",
            "je 4f",
            "cmp rax, {nr_epoll_pwait}",
            "je 4f",
            "cmp rax, {nr_epoll_pwait2}",
            "je 4f",
            // Entry stack frame: 88 bytes (`rsp % 16 == 0` before `call`).
            "sub rsp, 88",
            "mov rbx, rdx",
            "mov r13, rsp",
            // Lock-free pop from `STACK_POOL` with ZERO extra entry-stack bytes.
            "lea r8, [rip + {stack_pool}]",
            "mov ecx, {pool_slots}",
            "10:",
            "cmp qword ptr [r8], 0",
            "je 11f",
            "xor eax, eax",
            "xchg [r8], rax",
            "test rax, rax",
            "jnz 12f",
            "11:",
            "add r8, 8",
            "dec ecx",
            "jnz 10b",
            "call {alloc_stack}",
            "12:",
            "mov r12, rax",
            "test r12, r12",
            "jz 6f",
            // Switch RSP to private 64 KiB stack (`r12 - 16`, 16-byte aligned).
            "lea rsp, [r12 - 16]",
            "mov rdi, rbx",
            "mov rsi, r13",
            "call {handle_phase1}",
            "mov rsp, r13",
            // Lock-free push back to `STACK_POOL` with ZERO extra entry-stack bytes.
            "lea r8, [rip + {stack_pool}]",
            "mov ecx, {pool_slots}",
            "13:",
            "cmp qword ptr [r8], 0",
            "jne 14f",
            "xor eax, eax",
            "lock cmpxchg [r8], r12",
            "je 7f",
            "14:",
            "add r8, 8",
            "dec ecx",
            "jnz 13b",
            "mov rdi, r12",
            "call {free_stack}",
            "jmp 7f",
            "6:",
            // OOM fallback: run Phase 1 on current stack.
            "mov rdi, rbx",
            "mov rsi, r13",
            "call {handle_phase1}",
            "7:",
            "mov rdx, [r13 + 72]",
            "test rdx, rdx",
            "jz 5f",
            "mov rdi, rbx",
            "mov rsi, r13",
            "call {finish_phase2}",
            "5:",
            "add rsp, 88",
            "3:",
            "ret",
            "4:",
            "mov rdi, rdx",
            "jmp {masked_wait}",
            "2:",
            "jmp {foreign_sigsys}",
            sys_seccomp = const SYS_SECCOMP.cast_signed(),
            trap_tag = const TRAP_TAG as i32,
            uc_nr_off = const UC_NR_OFFSET,
            nr_rt_sigsuspend = const Sysno::rt_sigsuspend as usize,
            nr_ppoll = const Sysno::ppoll as usize,
            nr_pselect6 = const Sysno::pselect6 as usize,
            nr_epoll_pwait = const Sysno::epoll_pwait as usize,
            nr_epoll_pwait2 = const Sysno::epoll_pwait2 as usize,
            pool_slots = const crate::sigsys::stack_pool::STACK_POOL_SLOTS,
            stack_pool = sym crate::sigsys::stack_pool::STACK_POOL,
            alloc_stack = sym crate::sigsys::stack_pool::alloc_stack_raw,
            free_stack = sym crate::sigsys::stack_pool::free_stack_raw,
            handle_phase1 = sym crate::sigsys::sigsys_phase1,
            finish_phase2 = sym crate::sigsys::sigsys_phase2,
            masked_wait = sym crate::sigsys::sigsys_masked_wait,
            foreign_sigsys = sym crate::sigsys::sigsys_foreign,
        );
    }

    #[unsafe(naked)]
    pub unsafe extern "C" fn sigreturn_trampoline() -> ! {
        naked_asm!(
            "mov rax, {nr}",
            "syscall",
            "ud2",
            nr = const Sysno::rt_sigreturn as usize,
        );
    }

    #[unsafe(naked)]
    pub unsafe extern "C" fn call_on_stack(
        stack_top: usize,
        arg: *mut c_void,
        entry: extern "C" fn(*mut c_void),
    ) {
        naked_asm!(
            "lea r11, [rdi - 16]",
            "mov [r11], rsp",
            "mov rsp, r11",
            "mov rdi, rsi",
            "call rdx",
            "mov rsp, [rsp]",
            "ret",
        );
    }

    #[unsafe(naked)]
    unsafe extern "C" fn replay_unmasked_raw(
        inv: *const SyscallInvocation,
        orig_mask: *const u64,
        blocked_mask: *const u64,
    ) -> isize {
        // rdi = inv, rsi = orig_mask, rdx = blocked_mask.
        // Linux x86-64 `syscall` preserves rbx and r12..r15.
        naked_asm!(
            "push rbx",
            "push r12",
            "push r13",
            "mov rbx, rdi",
            "mov r12, rdx",
            // Unmask `orig_mask` for the duration of the replayed syscall.
            "mov eax, {nr_rt_sigprocmask}",
            "mov edi, {sig_setmask}",
            "xor edx, edx",
            "mov r10d, 8",
            "syscall",
            // Replay the trapped syscall directly from `inv`.
            "mov rax, [rbx + 0]",
            "mov rdi, [rbx + 8]",
            "mov rsi, [rbx + 16]",
            "mov rdx, [rbx + 24]",
            "mov r10, [rbx + 32]",
            "mov r8,  [rbx + 40]",
            "mov r9,  [rbx + 48]",
            "syscall",
            "mov r13, rax",
            // Re-block maskable signals before returning to Rust.
            "mov eax, {nr_rt_sigprocmask}",
            "mov edi, {sig_setmask}",
            "mov rsi, r12",
            "xor edx, edx",
            "mov r10d, 8",
            "syscall",
            "mov rax, r13",
            "pop r13",
            "pop r12",
            "pop rbx",
            "ret",
            sig_setmask = const SIG_SETMASK as usize,
            nr_rt_sigprocmask = const Sysno::rt_sigprocmask as usize,
        );
    }

    #[inline]
    pub unsafe fn replay_masked_wait(inv: &SyscallInvocation, _ucontext: &Ucontext) -> isize {
        // SAFETY: forwarded from caller contract.
        unsafe { super::replay(*inv) }
    }

    /// Builds a private child `ucontext` in the upper half of `altstack_top`
    /// so the `vfork`/`posix_spawn` child can install its `sigaltstack` and
    /// return via `SYS_rt_sigreturn` (preserving all GPRs, FPU/AVX/AMX state,
    /// and `eflags`).
    ///
    /// Returns the `rsp` value (`&child_uc`) to pass to `SYS_clone` and
    /// `SYS_rt_sigreturn`, or `None` if `altstack_size` is too small.
    pub unsafe fn prepare_vfork_child_sigframe(
        ucontext: *const Ucontext,
        altstack_top: usize,
        altstack_size: usize,
        orig_child_stack: usize,
        child_sigmask: u64,
    ) -> Option<usize> {
        let half = (altstack_size / 2) & !4095;
        if half < 4096 || altstack_size.saturating_sub(half) < size_of::<Ucontext>() + 64 {
            return None;
        }
        let altstack_base = altstack_top - altstack_size;
        // Leave 16 bytes above `altstack_base + half` so `rt_sigreturn`'s
        // `access_ok(rsp - 8, ...)` stays strictly inside the upper half.
        let child_uc_addr = altstack_base + half + 16;
        let child_uc_ptr = ptr::with_exposed_provenance_mut::<Ucontext>(child_uc_addr);
        // SAFETY: `ucontext` points to the live kernel `Ucontext` and
        // `child_uc_ptr` owns `size_of::<Ucontext>()` writable bytes.
        unsafe {
            ptr::copy_nonoverlapping(ucontext, child_uc_ptr, 1);
            let child_uc = &mut *child_uc_ptr;
            child_uc.uc_mcontext.gregs[REG_RAX] = 0;
            if orig_child_stack != 0 {
                child_uc.uc_mcontext.gregs[REG_RSP] = orig_child_stack as u64;
            }
            child_uc.uc_sigmask = child_sigmask;
            child_uc.uc_stack = StackT {
                ss_sp: ptr::with_exposed_provenance_mut(altstack_base),
                ss_flags: 0,
                _pad: 0,
                ss_size: half,
            };
        }
        Some(child_uc_addr)
    }

    /// Replays `clone(CLONE_VM | CLONE_VFORK, non_null_stack, ...)` starting
    /// the child on `child_sigreturn_sp` (`&child_uc`), where the child
    /// installs its pooled `sigaltstack` and returns via `SYS_rt_sigreturn`
    /// (restoring all GPRs, FPU/AVX/AMX state, `eflags`, and `sigmask`).
    /// Processes running with a CET shadow stack are not supported here.
    #[unsafe(naked)]
    pub unsafe extern "C" fn replay_vfork_with_altstack(
        inv: *const SyscallInvocation,
        child_sigreturn_sp: usize,
        altstack_size: usize,
        save_top: usize,
    ) -> isize {
        naked_asm!(
            "push rbp; push rbx; push r12",
            "mov rbp, rsp; mov r12, rdi; mov r8, rsi; xor ebx, ebx",
            "test rcx, rcx; jz 3f",
            "mov rax, [r12 + 16]; sub rax, rbp; jbe 3f",
            "mov r10, rdx; shr r10, 1; sub r10, 128; cmp rax, r10; ja 9f",
            "mov rbx, rax",
            "mov r11, rcx; sub r11, rbx; sub r11, 64; and r11, -64",
            "mov r9, rbp; and r9, 63; or r11, r9",
            "mov rsp, r11",
            "cld; mov rsi, rbp; mov rdi, r11; mov rcx, rbx; rep movsb",
            "mov rax, [r8 + {uc_fpregs_off}]",
            "cmp rax, rbp; jb 3f; cmp rax, [r12 + 16]; jae 3f",
            "sub rax, rbp; add rax, r11; mov [r8 + {uc_fpregs_off}], rax",
            "3:",
            "mov rax, [r12 + 0]; mov rdi, [r12 + 8]; mov rsi, r8",
            "mov rdx, [r12 + 24]; mov r10, [r12 + 32]; mov r8, [r12 + 40]",
            "syscall",
            "test rax, rax; jz 1f",
            "test rbx, rbx; jz 4f",
            "cld; mov rsi, rsp; mov rdi, rbp; mov rcx, rbx; rep movsb; mov rsp, rbp",
            "4:",
            "pop r12; pop rbx; pop rbp; ret",
            "9:",
            "mov rax, {neg_enomem}; jmp 4b",
            "1:",
            "mov eax, {nr_sigaltstack}; lea rdi, [rsp + {uc_stack_off}]; xor esi, esi; syscall",
            "mov eax, {nr_rt_sigreturn}; syscall; ud2",
            neg_enomem = const -(ENOMEM as i64),
            uc_stack_off = const UC_STACK_OFFSET,
            uc_fpregs_off = const UC_FPREGS_OFFSET,
            nr_sigaltstack = const Sysno::sigaltstack as usize,
            nr_rt_sigreturn = const Sysno::rt_sigreturn as usize,
        );
    }

    #[inline]
    pub unsafe fn replay_unmasked(
        inv: &SyscallInvocation,
        orig_mask: u64,
        blocked_mask: u64,
        _ucontext: &Ucontext,
    ) -> isize {
        // SAFETY: forwarded from caller contract; stack locals stay live for the call.
        unsafe { replay_unmasked_raw(inv, &raw const orig_mask, &raw const blocked_mask) }
    }

    #[expect(clippy::cast_possible_truncation, reason = "64-bit Linux native registers")]
    pub const fn decode_syscall(ucontext: &Ucontext) -> SyscallInvocation {
        let gregs = &ucontext.uc_mcontext.gregs;
        SyscallInvocation {
            nr: gregs[REG_RAX] as usize,
            args: [
                gregs[REG_RDI] as usize,
                gregs[REG_RSI] as usize,
                gregs[REG_RDX] as usize,
                gregs[REG_R10] as usize,
                gregs[REG_R8] as usize,
                gregs[REG_R9] as usize,
            ],
        }
    }

    pub const fn set_syscall_result(ucontext: &mut Ucontext, result: isize) {
        ucontext.uc_mcontext.gregs[REG_RAX] = result.cast_unsigned() as u64;
    }

    #[expect(clippy::cast_possible_truncation, reason = "64-bit x86-64 usize == u64")]
    pub const fn ucontext_sp(ucontext: &Ucontext) -> usize {
        ucontext.uc_mcontext.gregs[REG_RSP] as usize
    }

    pub const fn set_ucontext_sp(ucontext: &mut Ucontext, sp: usize) {
        ucontext.uc_mcontext.gregs[REG_RSP] = sp as u64;
    }

    pub const fn uc_sigmask_mut(ucontext: &mut Ucontext) -> &mut u64 {
        &mut ucontext.uc_sigmask
    }

    /// Invokes a target-registered `SIGSYS` handler on x86-64.
    pub unsafe fn invoke_user_sigsys_handler(
        handler_addr: usize,
        is_siginfo: bool,
        signum: i32,
        info: *mut linux_raw_sys::general::siginfo,
        ucontext: &mut Ucontext,
    ) {
        if is_siginfo {
            // SAFETY: `handler_addr` was registered by the target as an `SA_SIGINFO` handler.
            let handler: unsafe extern "C" fn(
                i32,
                *mut linux_raw_sys::general::siginfo,
                *mut c_void,
            ) = unsafe { core::mem::transmute(handler_addr) };
            // SAFETY: Invoking target's registered `SIGSYS` handler.
            unsafe { handler(signum, info, ptr::from_mut(ucontext).cast()) };
        } else {
            // SAFETY: `handler_addr` was registered by the target as a 1-arg handler.
            let handler: unsafe extern "C" fn(i32) = unsafe { core::mem::transmute(handler_addr) };
            // SAFETY: Invoking target's registered `SIGSYS` handler.
            unsafe { handler(signum) };
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    use core::{arch::naked_asm, ffi::c_void, ptr};

    use linux_raw_sys::{
        errno::ENOMEM,
        general::{SIG_SETMASK, SYS_SECCOMP},
    };
    use syscalls::Sysno;

    use super::{StackT, SyscallInvocation};
    use crate::sigsys::filter::TRAP_TAG;

    /// Linux `AArch64` `struct sigcontext` prefix (16-byte aligned due to
    /// `__reserved` carrying `__attribute__((__aligned__(16)))`).
    #[repr(C, align(16))]
    pub struct Sigcontext {
        pub fault_address: u64,
        pub regs: [u64; 31],
        pub sp: u64,
        pub pc: u64,
        pub pstate: u64,
    }

    #[repr(C)]
    pub struct Ucontext {
        pub uc_flags: u64,
        pub uc_link: *mut Self,
        pub uc_stack: StackT,
        pub uc_sigmask: u64,
        /// Kernel pads `uc_sigmask` to glibc's 1024-bit `sigset_t` (128 bytes).
        pub _sigmask_pad: [u8; 128 - 8],
        pub uc_mcontext: Sigcontext,
    }

    /// Size of `struct siginfo` preceding `ucontext` in `AArch64` `rt_sigframe`.
    const RT_SIGFRAME_UC_OFFSET: usize = 128;
    /// Offset of `uc.uc_stack` from `rt_sigframe` start (`128 + 16`).
    const RT_SIGFRAME_UC_STACK_OFFSET: usize = RT_SIGFRAME_UC_OFFSET + 16;
    /// Offset of `uc_mcontext.__reserved[4096]` from `Ucontext` start (`176 + 288`).
    const UC_RESERVED_OFFSET: usize = 176 + 288;
    /// Full kernel `struct ucontext` size including `sigcontext.__reserved[4096]`.
    const UC_FULL_BYTES: usize = UC_RESERVED_OFFSET + 4096;

    const UC_NR_OFFSET: usize = 176 + 8 + 8 * 8;

    const _: () = {
        use core::mem::offset_of;
        assert!(offset_of!(Ucontext, uc_flags) == 0);
        assert!(offset_of!(Ucontext, uc_link) == 8);
        assert!(offset_of!(Ucontext, uc_stack) == 16);
        assert!(offset_of!(Ucontext, uc_sigmask) == 40);
        assert!(offset_of!(Ucontext, uc_mcontext) == 176);
        assert!(offset_of!(Sigcontext, fault_address) == 0);
        assert!(offset_of!(Sigcontext, regs) == 8);
        assert!(offset_of!(Sigcontext, sp) == 256);
        assert!(offset_of!(Sigcontext, pc) == 264);
        assert!(offset_of!(Sigcontext, pstate) == 272);
        assert!(
            UC_NR_OFFSET
                == offset_of!(Ucontext, uc_mcontext) + offset_of!(Sigcontext, regs) + 8 * 8
        );
        assert!(
            RT_SIGFRAME_UC_STACK_OFFSET == RT_SIGFRAME_UC_OFFSET + offset_of!(Ucontext, uc_stack)
        );
    };

    #[unsafe(naked)]
    pub unsafe extern "C" fn sigsys_trampoline() {
        // Entry: x0=signum, x1=*mut siginfo, x2=*mut Ucontext, x30=rt_sigreturn.
        // Uses 0 stack bytes for foreign/masked-wait dispatch and only a 96-byte
        // frame (80B Phase2Packet + x30) around the lock-free `STACK_POOL` swap,
        // fitting comfortably inside AArch64 musl's 5 KiB `posix_spawn` stack.
        naked_asm!(
            "cbz x1, 2f",
            "ldr w9, [x1, #8]",
            "cmp w9, #{sys_seccomp}",
            "b.ne 2f",
            "ldr w9, [x1, #4]",
            "movz w10, #{trap_tag}",
            "cmp w9, w10",
            "b.ne 2f",
            "cbz x2, 3f",
            "ldr x9, [x2, #{uc_nr_off}]",
            "cmp x9, #{nr_rt_sigsuspend}; b.eq 4f",
            "cmp x9, #{nr_ppoll}; b.eq 4f",
            "cmp x9, #{nr_pselect6}; b.eq 4f",
            "cmp x9, #{nr_epoll_pwait}; b.eq 4f",
            "cmp x9, #{nr_epoll_pwait2}; b.eq 4f",
            "sub sp, sp, #96",
            "str x30, [sp, #80]",
            "mov x19, x2",
            "mov x21, sp",
            "adrp x9, {stack_pool}",
            "add x9, x9, :lo12:{stack_pool}",
            "mov x10, #{pool_slots}",
            "10:",
            "ldaxr x20, [x9]",
            "cbz x20, 11f",
            "stlxr w11, xzr, [x9]",
            "cbnz w11, 10b",
            "b 12f",
            "11:",
            "clrex",
            "add x9, x9, #8",
            "subs x10, x10, #1",
            "b.ne 10b",
            "bl {alloc_stack}",
            "mov x20, x0",
            "12:",
            "cbz x20, 6f",
            "sub x10, x20, #16",
            "mov sp, x10",
            "mov x0, x19",
            "mov x1, x21",
            "bl {handle_phase1}",
            "mov sp, x21",
            "adrp x9, {stack_pool}",
            "add x9, x9, :lo12:{stack_pool}",
            "mov x10, #{pool_slots}",
            "13:",
            "ldaxr x11, [x9]",
            "cbnz x11, 14f",
            "stlxr w12, x20, [x9]",
            "cbnz w12, 13b",
            "b 7f",
            "14:",
            "clrex",
            "add x9, x9, #8",
            "subs x10, x10, #1",
            "b.ne 13b",
            "mov x0, x20",
            "bl {free_stack}",
            "b 7f",
            "6:",
            "mov x0, x19",
            "mov x1, x21",
            "bl {handle_phase1}",
            "7:",
            "ldr x2, [x21, #72]",
            "cbz x2, 5f",
            "mov x0, x19",
            "mov x1, x21",
            "bl {finish_phase2}",
            "5:",
            "ldr x30, [sp, #80]",
            "add sp, sp, #96",
            "3:",
            "ret",
            "4:",
            "mov x0, x2",
            "b {masked_wait}",
            "2:",
            "b {foreign_sigsys}",
            sys_seccomp = const SYS_SECCOMP.cast_signed(),
            trap_tag = const TRAP_TAG as u32,
            uc_nr_off = const UC_NR_OFFSET,
            nr_rt_sigsuspend = const Sysno::rt_sigsuspend as usize,
            nr_ppoll = const Sysno::ppoll as usize,
            nr_pselect6 = const Sysno::pselect6 as usize,
            nr_epoll_pwait = const Sysno::epoll_pwait as usize,
            nr_epoll_pwait2 = const Sysno::epoll_pwait2 as usize,
            pool_slots = const crate::sigsys::stack_pool::STACK_POOL_SLOTS,
            stack_pool = sym crate::sigsys::stack_pool::STACK_POOL,
            alloc_stack = sym crate::sigsys::stack_pool::alloc_stack_raw,
            free_stack = sym crate::sigsys::stack_pool::free_stack_raw,
            handle_phase1 = sym crate::sigsys::sigsys_phase1,
            finish_phase2 = sym crate::sigsys::sigsys_phase2,
            masked_wait = sym crate::sigsys::sigsys_masked_wait,
            foreign_sigsys = sym crate::sigsys::sigsys_foreign,
        );
    }

    #[unsafe(naked)]
    pub unsafe extern "C" fn sigreturn_trampoline() -> ! {
        naked_asm!(
            "mov x8, #{nr}",
            "svc #0",
            "brk #0",
            nr = const Sysno::rt_sigreturn as usize,
        );
    }

    #[unsafe(naked)]
    pub unsafe extern "C" fn call_on_stack(
        stack_top: usize,
        arg: *mut c_void,
        entry: extern "C" fn(*mut c_void),
    ) {
        naked_asm!(
            "mov x9, sp",
            "sub x10, x0, #16",
            "stp x9, x30, [x10]",
            "mov sp, x10",
            "mov x0, x1",
            "blr x2",
            "ldp x9, x30, [sp]",
            "mov sp, x9",
            "ret",
        );
    }

    #[unsafe(naked)]
    unsafe extern "C" fn replay_with_x28_raw(
        inv: *const SyscallInvocation,
        target_x28: u64,
    ) -> isize {
        // Restore the target's `x28` (Go's `g` register on `CGO_ENABLED=0`
        // AArch64) across a blocking syscall that temporarily unmasks signals,
        // so any signal handler (e.g. Go's `SIGURG`) observes a valid `x28`.
        naked_asm!(
            "sub sp, sp, #16",
            "str x28, [sp]",
            "mov x28, x1",
            "mov x9, x0",
            "ldr x8, [x9, #0]",
            "ldp x0, x1, [x9, #8]",
            "ldp x2, x3, [x9, #24]",
            "ldp x4, x5, [x9, #40]",
            "svc #0",
            "ldr x28, [sp]",
            "add sp, sp, #16",
            "ret",
        );
    }

    #[unsafe(naked)]
    unsafe extern "C" fn replay_unmasked_raw(
        inv: *const SyscallInvocation,
        orig_mask: *const u64,
        blocked_mask: *const u64,
        target_x28: u64,
    ) -> isize {
        // x0 = inv, x1 = orig_mask, x2 = blocked_mask, x3 = target_x28.
        // Set `x28 = target_x28` BEFORE unmasking signals and re-block signals
        // BEFORE restoring caller `x28` so Rust code never runs unmasked.
        naked_asm!(
            "stp x19, x20, [sp, #-32]!",
            "stp x21, x28, [sp, #16]",
            "mov x19, x0",
            "mov x20, x2",
            "mov x28, x3",
            // Unmask `orig_mask`.
            "mov x0, #{sig_setmask}",
            "mov x2, #0",
            "mov x3, #8",
            "mov x8, #{nr_rt_sigprocmask}",
            "svc #0",
            // Replay the trapped syscall from `inv`.
            "ldr x8, [x19, #0]",
            "ldp x0, x1, [x19, #8]",
            "ldp x2, x3, [x19, #24]",
            "ldp x4, x5, [x19, #40]",
            "svc #0",
            "mov x21, x0",
            // Re-block maskable signals before restoring caller `x28`.
            "mov x0, #{sig_setmask}",
            "mov x1, x20",
            "mov x2, #0",
            "mov x3, #8",
            "mov x8, #{nr_rt_sigprocmask}",
            "svc #0",
            "mov x0, x21",
            "ldp x21, x28, [sp, #16]",
            "ldp x19, x20, [sp], #32",
            "ret",
            sig_setmask = const SIG_SETMASK as usize,
            nr_rt_sigprocmask = const Sysno::rt_sigprocmask as usize,
        );
    }

    /// Builds a private child `rt_sigframe` (including `sigcontext.__reserved`)
    /// in the upper half of `altstack_top` so the `vfork`/`posix_spawn` child
    /// can install its `sigaltstack` and return via `SYS_rt_sigreturn`
    /// (preserving `x16`, `pstate`, FP/SIMD state, and ARM64 BTI compliance
    /// via `eret`). Frames with an `EXTRA_MAGIC` record past `__reserved`
    /// (live Scalable Matrix Extension `ZA` state) are not supported here.
    ///
    /// Returns the `sp` value (`&child_rt_sigframe`) to pass to `SYS_clone`
    /// and `SYS_rt_sigreturn`, or `None` if `altstack_size` is too small.
    pub unsafe fn prepare_vfork_child_sigframe(
        ucontext: *const Ucontext,
        altstack_top: usize,
        altstack_size: usize,
        orig_child_stack: usize,
        child_sigmask: u64,
    ) -> Option<usize> {
        let half = (altstack_size / 2) & !4095;
        let upper = altstack_size.saturating_sub(half);
        let max_uc_bytes = upper.checked_sub(RT_SIGFRAME_UC_OFFSET)?;
        if half < 4096 || max_uc_bytes < UC_FULL_BYTES {
            return None;
        }
        let altstack_base = altstack_top - altstack_size;
        let child_frame_addr = altstack_base + half;
        let child_uc_addr = child_frame_addr + RT_SIGFRAME_UC_OFFSET;
        let dst_uc = ptr::with_exposed_provenance_mut::<u8>(child_uc_addr);
        // SAFETY: `ucontext` is the raw kernel `AArch64` `ucontext` pointer
        // (`UC_FULL_BYTES` = 4560 bytes including `__reserved[4096]`), and
        // `dst_uc` owns `max_uc_bytes >= UC_FULL_BYTES` writable bytes.
        unsafe {
            ptr::copy_nonoverlapping(ucontext.cast::<u8>(), dst_uc, UC_FULL_BYTES);
            let child_uc = &mut *ptr::with_exposed_provenance_mut::<Ucontext>(child_uc_addr);
            child_uc.uc_mcontext.regs[0] = 0;
            if orig_child_stack != 0 {
                child_uc.uc_mcontext.sp = orig_child_stack as u64;
            }
            child_uc.uc_sigmask = child_sigmask;
            child_uc.uc_stack = StackT {
                ss_sp: ptr::with_exposed_provenance_mut(altstack_base),
                ss_flags: 0,
                _pad: 0,
                ss_size: half,
            };
        }
        Some(child_frame_addr)
    }

    /// Replays `clone(CLONE_VM | CLONE_VFORK, non_null_stack, ...)` starting
    /// the child on `child_sigreturn_sp` (`&child_rt_sigframe`), where the
    /// child installs its pooled `sigaltstack` and returns via
    /// `SYS_rt_sigreturn` (preserving `x16`, FP/SIMD, and ARM64 BTI
    /// compliance via `eret`). Processes running with an Arm Guarded Control
    /// Stack (GCS) are not supported here.
    #[unsafe(naked)]
    pub unsafe extern "C" fn replay_vfork_with_altstack(
        inv: *const SyscallInvocation,
        child_sigreturn_sp: usize,
        altstack_size: usize,
        save_top: usize,
    ) -> isize {
        naked_asm!(
            "stp x19, x20, [sp, #-48]!; stp x21, x22, [sp, #16]; str x30, [sp, #32]",
            "mov x19, sp; ldr x20, [x0, #16]; mov x21, #0; mov x22, x3",
            "cbz x22, 3f",
            "subs x9, x20, x19; b.ls 3f",
            "lsr x15, x2, #1; sub x15, x15, #128; cmp x9, x15; b.hi 11f",
            "mov x21, x9; sub x10, x22, x21; and x10, x10, #-16; mov sp, x10",
            "mov x11, x19; mov x12, x10; mov x13, x21",
            "5: cmp x13, #8; b.lo 6f; ldr x14, [x11], #8; str x14, [x12], #8; sub x13, x13, #8; b 5b",
            "6: cbz x13, 3f; ldrb w14, [x11], #1; strb w14, [x12], #1; sub x13, x13, #1; b 6b",
            "3:",
            "mov x9, x0; mov x10, x1",
            "ldr x8, [x9, #0]; ldr x0, [x9, #8]; mov x1, x10",
            "ldr x2, [x9, #24]; ldr x3, [x9, #32]; ldr x4, [x9, #40]",
            "svc #0",
            "cbz x0, 1f",
            "cbz x21, 4f",
            "mov x11, sp; mov x12, x19; mov x13, x21",
            "8: cmp x13, #8; b.lo 9f; ldr x14, [x11], #8; str x14, [x12], #8; sub x13, x13, #8; b 8b",
            "9: cbz x13, 10f; ldrb w14, [x11], #1; strb w14, [x12], #1; sub x13, x13, #1; b 9b",
            "10: mov sp, x19",
            "4:",
            "ldr x30, [sp, #32]; ldp x21, x22, [sp, #16]; ldp x19, x20, [sp], #48; ret",
            "11:",
            "mov x0, #{neg_enomem}; b 4b",
            "1:",
            "add x0, sp, #{uc_stack_from_frame}; mov x1, #0; mov x8, #{nr_sigaltstack}; svc #0",
            "mov x8, #{nr_rt_sigreturn}; svc #0; brk #0",
            neg_enomem = const -(ENOMEM as i64),
            uc_stack_from_frame = const RT_SIGFRAME_UC_STACK_OFFSET,
            nr_sigaltstack = const Sysno::sigaltstack as usize,
            nr_rt_sigreturn = const Sysno::rt_sigreturn as usize,
        );
    }

    #[unsafe(naked)]
    unsafe extern "C" fn call_handler_with_x28_raw(
        handler_addr: usize,
        signum: i32,
        info: *mut c_void,
        ucontext: *mut c_void,
        target_x28: u64,
    ) {
        // Restore the target's `x28` (Go's `g` register on AArch64) across the
        // target's own `SIGSYS` handler invocation.
        naked_asm!(
            "stp x28, x30, [sp, #-16]!",
            "mov x9, x0",
            "mov x0, x1",
            "mov x1, x2",
            "mov x2, x3",
            "mov x28, x4",
            "blr x9",
            "ldp x28, x30, [sp], #16",
            "ret",
        );
    }

    /// Invokes a target-registered `SIGSYS` handler on `AArch64` with the
    /// target's `x28` restored from `ucontext`.
    pub unsafe fn invoke_user_sigsys_handler(
        handler_addr: usize,
        _is_siginfo: bool,
        signum: i32,
        info: *mut linux_raw_sys::general::siginfo,
        ucontext: &mut Ucontext,
    ) {
        let target_x28 = ucontext.uc_mcontext.regs[28];
        // SAFETY: `handler_addr` was registered by the target as a signal handler.
        unsafe {
            call_handler_with_x28_raw(
                handler_addr,
                signum,
                info.cast(),
                ptr::from_mut(ucontext).cast(),
                target_x28,
            );
        }
    }

    #[inline]
    pub unsafe fn replay_masked_wait(inv: &SyscallInvocation, ucontext: &Ucontext) -> isize {
        // SAFETY: forwarded from caller contract.
        unsafe { replay_with_x28_raw(inv, ucontext.uc_mcontext.regs[28]) }
    }

    #[inline]
    pub unsafe fn replay_unmasked(
        inv: &SyscallInvocation,
        orig_mask: u64,
        blocked_mask: u64,
        ucontext: &Ucontext,
    ) -> isize {
        // SAFETY: forwarded from caller contract; stack locals stay live for the call.
        unsafe {
            replay_unmasked_raw(
                inv,
                &raw const orig_mask,
                &raw const blocked_mask,
                ucontext.uc_mcontext.regs[28],
            )
        }
    }

    #[expect(clippy::cast_possible_truncation, reason = "64-bit Linux native registers")]
    pub const fn decode_syscall(ucontext: &Ucontext) -> SyscallInvocation {
        let regs = &ucontext.uc_mcontext.regs;
        SyscallInvocation {
            nr: regs[8] as usize,
            args: [
                regs[0] as usize,
                regs[1] as usize,
                regs[2] as usize,
                regs[3] as usize,
                regs[4] as usize,
                regs[5] as usize,
            ],
        }
    }

    pub const fn set_syscall_result(ucontext: &mut Ucontext, result: isize) {
        ucontext.uc_mcontext.regs[0] = result.cast_unsigned() as u64;
    }

    #[expect(clippy::cast_possible_truncation, reason = "64-bit AArch64 usize == u64")]
    pub const fn ucontext_sp(ucontext: &Ucontext) -> usize {
        ucontext.uc_mcontext.sp as usize
    }

    pub const fn set_ucontext_sp(ucontext: &mut Ucontext, sp: usize) {
        ucontext.uc_mcontext.sp = sp as u64;
    }

    pub const fn uc_sigmask_mut(ucontext: &mut Ucontext) -> &mut u64 {
        &mut ucontext.uc_sigmask
    }
}

pub use imp::{
    Ucontext, call_on_stack, decode_syscall, invoke_user_sigsys_handler,
    prepare_vfork_child_sigframe, replay_masked_wait, replay_unmasked, replay_vfork_with_altstack,
    set_syscall_result, set_ucontext_sp, sigreturn_trampoline, sigsys_trampoline, uc_sigmask_mut,
    ucontext_sp,
};

/// Executes a raw 6-argument Linux syscall from within the launcher's
/// allowlisted `PF_X` code range.
///
/// Returns the raw kernel return value (non-negative result or `-errno`).
///
/// # Safety
///
/// The caller must uphold the kernel ABI contract for syscall `nr` and its
/// six arguments.
#[inline]
pub unsafe fn syscall6(
    nr: usize,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
) -> isize {
    // SAFETY: forwarded from caller contract.
    unsafe { syscalls::raw::syscall6(nr, a0, a1, a2, a3, a4, a5) }.cast_signed()
}

/// Replays a decoded syscall invocation directly to the kernel.
///
/// # Safety
///
/// The caller must uphold the kernel ABI contract for `invocation`.
#[inline]
pub unsafe fn replay(invocation: SyscallInvocation) -> isize {
    // SAFETY: forwarded from caller contract.
    unsafe {
        syscall6(
            invocation.nr,
            invocation.args[0],
            invocation.args[1],
            invocation.args[2],
            invocation.args[3],
            invocation.args[4],
            invocation.args[5],
        )
    }
}

/// Extracts a 32-bit signed integer argument from a 64-bit syscall register.
#[inline]
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "Linux 32-bit signed syscall arguments live in the low 32 bits of a 64-bit register"
)]
pub const fn arg_i32(value: usize) -> i32 {
    value as i32
}

/// Converts a positive Linux `errno` constant into its negated raw syscall
/// return value (`-errno`).
#[inline]
#[must_use]
pub const fn neg_errno(errno: u32) -> isize {
    -(errno.cast_signed() as isize)
}

/// Calls `getpid()` directly.
#[inline]
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "Linux pid_t is i32")]
pub fn getpid() -> i32 {
    // SAFETY: `getpid` takes no arguments and cannot fail.
    unsafe { syscalls::raw::syscall0(Sysno::getpid as usize).cast_signed() as i32 }
}

/// Calls `gettid()` directly.
#[inline]
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "Linux pid_t is i32")]
pub fn gettid() -> i32 {
    // SAFETY: `gettid` takes no arguments and cannot fail.
    unsafe { syscalls::raw::syscall0(Sysno::gettid as usize).cast_signed() as i32 }
}

/// Calls `rt_sigaction` directly.
///
/// # Safety
///
/// `act` and `oldact` must be null or point to valid [`KernelSigaction`] storage.
#[inline]
pub unsafe fn rt_sigaction(
    signum: i32,
    act: *const KernelSigaction,
    oldact: *mut KernelSigaction,
) -> isize {
    // SAFETY: caller upholds pointer validity.
    unsafe {
        syscall6(
            Sysno::rt_sigaction as usize,
            signum.cast_unsigned() as usize,
            act.addr(),
            oldact.addr(),
            KERNEL_SIGSET_SIZE,
            0,
            0,
        )
    }
}

/// Calls `rt_sigprocmask` directly.
///
/// # Safety
///
/// `set` and `oldset` must be null or point to valid `u64` storage.
#[inline]
pub unsafe fn rt_sigprocmask(how: u32, set: *const u64, oldset: *mut u64) -> isize {
    // SAFETY: caller upholds pointer validity.
    unsafe {
        syscall6(
            Sysno::rt_sigprocmask as usize,
            how as usize,
            set.addr(),
            oldset.addr(),
            KERNEL_SIGSET_SIZE,
            0,
            0,
        )
    }
}
