//! Seccomp-BPF filter construction, fingerprint probing, and installation.
//!
//! The installed filter executes in optimal order:
//! 1. Rejects syscalls whose `seccomp_data.arch` does not match the native
//!    64-bit architecture (and, on x86-64, x32 ABI syscalls with
//!    `nr >= 0x4000_0000`) with `SECCOMP_RET_ERRNO | ENOSYS` so foreign/compat
//!    ABIs cannot bypass tracking or crash on foreign-ABI `execve`.
//! 2. Answers the idempotency probe
//!    `getpid(PROBE_MAGIC, fingerprint)` with `SECCOMP_RET_ERRNO | EOWNERDEAD`
//!    **before** the `PF_X` IP allowlist so recursive `execve`s of the same
//!    launcher detect an already-installed identical filter and avoid filter
//!    stacking.
//! 3. Returns `SECCOMP_RET_ERRNO | ENOSYS` for `clone3` (so callers like
//!    glibc's `posix_spawn`, which pass `CLONE_CLEAR_SIGHAND` to reset all
//!    signal handlers including `SIGSYS`, fall back cleanly to `clone(2)`) and
//!    for `io_uring_setup` (so runtimes fall back to standard syscalls instead
//!    of bypassing `/proc/*/exe` virtualization and caller interceptors via
//!    `io_uring` rings).
//! 4. Conditionally traps:
//!    - `ppoll` / `pselect6` / `epoll_pwait` / `epoll_pwait2` **only** when the
//!      temporary sigmask pointer argument is non-null (avoiding `SIGSYS` traps
//!      on hot event-loop waits like Go's netpoller and glibc `poll`/`epoll_wait`).
//!    - `clone` when `(flags & CLONE_VM) == 0` (every new-address-space fork,
//!      with either NULL or non-NULL `child_stack`, so the child updates process
//!      ownership, resets locks, and inherits virtualized `SIGSYS` blocking), or
//!      when `(flags & (CLONE_VM | CLONE_VFORK)) == (CLONE_VM | CLONE_VFORK)`
//!      with `child_stack != 0` (so `posix_spawn` / `vfork` children with small
//!      caller stacks receive a pooled `sigaltstack` before trapping). Regular
//!      threads (`CLONE_VM` without `CLONE_VFORK`) and shared-stack `vfork`
//!      (`child_stack == 0`) never trap.
//! 5. Allows any syscall number not in the merged trapped syscall set without
//!    ever inspecting `instruction_pointer`.
//! 6. Allows trapped syscalls whose `instruction_pointer` lies inside the
//!    launcher's `PF_X` `PT_LOAD` range `[code_start, code_end)`.
//! 7. Traps all remaining matched syscalls with `SECCOMP_RET_TRAP | TRAP_TAG`.

use core::mem::offset_of;

use fspy_nostd::{Error, Result};
use linux_raw_sys::{
    errno::{ENOSYS, EOWNERDEAD},
    general::{CLONE_VFORK, CLONE_VM},
    prctl::PR_SET_NO_NEW_PRIVS,
    ptrace::{
        AUDIT_ARCH_X86_64, BPF_ABS, BPF_JEQ, BPF_JGE, BPF_JGT, BPF_JMP, BPF_JSET, BPF_K, BPF_LD,
        BPF_RET, BPF_W, SECCOMP_RET_ALLOW, SECCOMP_RET_DATA, SECCOMP_RET_ERRNO, SECCOMP_RET_TRAP,
        SECCOMP_SET_MODE_FILTER, seccomp_data, sock_filter, sock_fprog,
    },
};
use syscalls::Sysno;

use super::arch::{AUDIT_ARCH_NATIVE, neg_errno, syscall6};

#[expect(clippy::cast_possible_truncation, reason = "cBPF opcodes fit in u16")]
const LD_W_ABS: u16 = (BPF_LD | BPF_W | BPF_ABS) as u16;
#[expect(clippy::cast_possible_truncation, reason = "cBPF opcodes fit in u16")]
const JMP_JEQ_K: u16 = (BPF_JMP | BPF_JEQ | BPF_K) as u16;
#[expect(clippy::cast_possible_truncation, reason = "cBPF opcodes fit in u16")]
const JMP_JGE_K: u16 = (BPF_JMP | BPF_JGE | BPF_K) as u16;
#[expect(clippy::cast_possible_truncation, reason = "cBPF opcodes fit in u16")]
const JMP_JGT_K: u16 = (BPF_JMP | BPF_JGT | BPF_K) as u16;
#[expect(clippy::cast_possible_truncation, reason = "cBPF opcodes fit in u16")]
const JMP_JSET_K: u16 = (BPF_JMP | BPF_JSET | BPF_K) as u16;
#[expect(clippy::cast_possible_truncation, reason = "cBPF opcodes fit in u16")]
const RET_K: u16 = (BPF_RET | BPF_K) as u16;

#[expect(clippy::cast_possible_truncation, reason = "seccomp_data is 64 bytes")]
const OFF_NR: u32 = offset_of!(seccomp_data, nr) as u32;
#[expect(clippy::cast_possible_truncation, reason = "seccomp_data is 64 bytes")]
const OFF_ARCH: u32 = offset_of!(seccomp_data, arch) as u32;
// Little-endian 64-bit fields: low 32 bits first, high 32 bits second.
#[expect(clippy::cast_possible_truncation, reason = "seccomp_data is 64 bytes")]
const OFF_IP_LO: u32 = offset_of!(seccomp_data, instruction_pointer) as u32;
const OFF_IP_HI: u32 = OFF_IP_LO + 4;
#[expect(clippy::cast_possible_truncation, reason = "seccomp_data is 64 bytes")]
const OFF_ARG0_LO: u32 = offset_of!(seccomp_data, args) as u32;
const OFF_ARG0_HI: u32 = OFF_ARG0_LO + 4;
const OFF_ARG1_LO: u32 = OFF_ARG0_LO + 8;
const OFF_ARG1_HI: u32 = OFF_ARG1_LO + 4;
const OFF_ARG3_LO: u32 = OFF_ARG0_LO + 24;
const OFF_ARG3_HI: u32 = OFF_ARG3_LO + 4;
const OFF_ARG4_LO: u32 = OFF_ARG0_LO + 32;
const OFF_ARG4_HI: u32 = OFF_ARG4_LO + 4;
const OFF_ARG5_LO: u32 = OFF_ARG0_LO + 40;
const OFF_ARG5_HI: u32 = OFF_ARG5_LO + 4;

/// Magic `args[0]` sentinel for the `getpid(PROBE_MAGIC, fingerprint)` probe.
pub const PROBE_MAGIC: u64 = 0x6673_7079_5f73_6563;

/// Non-zero 16-bit tag delivered in `siginfo->si_errno` for our own
/// `SECCOMP_RET_TRAP` deliveries so target-installed seccomp filters (which
/// carry a different `si_errno`, typically `0`) can be forwarded untouched.
pub const TRAP_TAG: u16 = 0x5350;

/// Seccomp return action emitted for syscalls trapped by this loader.
pub const SECCOMP_RET_TAGGED_TRAP: u32 = SECCOMP_RET_TRAP | (TRAP_TAG as u32 & SECCOMP_RET_DATA);

/// Seccomp return action emitted when the `getpid` idempotency probe matches.
pub const SECCOMP_RET_PROBE: u32 = SECCOMP_RET_ERRNO | (EOWNERDEAD & SECCOMP_RET_DATA);

/// Seccomp return action emitted for foreign/compat ABIs (including x32 on
/// x86-64), `clone3`, and `io_uring_setup` so foreign ABIs cannot bypass
/// tracking and runtimes fall back to `clone(2)` and standard filesystem
/// syscalls.
pub const SECCOMP_RET_ENOSYS: u32 = SECCOMP_RET_ERRNO | (ENOSYS & SECCOMP_RET_DATA);

/// Linux `__X32_SYSCALL_BIT`: x32 ABI syscalls share `AUDIT_ARCH_X86_64` and
/// set bit 30 (`nr >= 0x4000_0000`).
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// Maximum number of unique unconditionally trapped syscalls (loader base +
/// interceptor).
pub const MAX_TRAPPED_SYSCALLS: usize = 64;

/// Maximum instruction count emitted by [`build_filter`].
const MAX_BPF_INSNS: usize = 192;

/// Loader-owned syscalls trapped unconditionally on x86-64 for `SIGSYS`,
/// `fork`, and `/proc/*/exe` virtualization.
///
/// Masked waits (`ppoll`, `pselect6`, `epoll_pwait`, `epoll_pwait2`) and
/// `clone` are handled by conditional BPF gates in [`build_filter`] so hot
/// `NULL`-sigmask waits, regular threads, and shared-stack `vfork` never trap.
#[cfg(target_arch = "x86_64")]
pub const BASE_TRAPPED_SYSCALLS: &[u32] = &[
    // Signal & fork virtualization
    Sysno::rt_sigaction as u32,
    Sysno::rt_sigprocmask as u32,
    Sysno::rt_sigpending as u32,
    Sysno::rt_sigsuspend as u32,
    Sysno::fork as u32,
    // /proc/self/exe virtualization
    Sysno::readlink as u32,
    Sysno::readlinkat as u32,
    Sysno::open as u32,
    Sysno::openat as u32,
    Sysno::openat2 as u32,
    // `lstat` never follows `/proc/self/exe`, so it needs no rewrite.
    Sysno::stat as u32,
    Sysno::newfstatat as u32,
    Sysno::statx as u32,
    Sysno::access as u32,
    Sysno::faccessat as u32,
    Sysno::faccessat2 as u32,
    Sysno::execve as u32,
    Sysno::execveat as u32,
];

/// Loader-owned syscalls trapped unconditionally on `AArch64` for `SIGSYS` and
/// `/proc/*/exe` virtualization.
#[cfg(target_arch = "aarch64")]
pub const BASE_TRAPPED_SYSCALLS: &[u32] = &[
    // Signal virtualization
    Sysno::rt_sigaction as u32,
    Sysno::rt_sigprocmask as u32,
    Sysno::rt_sigpending as u32,
    Sysno::rt_sigsuspend as u32,
    // /proc/self/exe virtualization
    Sysno::readlinkat as u32,
    Sysno::openat as u32,
    Sysno::openat2 as u32,
    Sysno::fstatat as u32,
    Sysno::statx as u32,
    Sysno::faccessat as u32,
    Sysno::faccessat2 as u32,
    Sysno::execve as u32,
    Sysno::execveat as u32,
];

/// Fixed-capacity cBPF program buffer requiring no heap allocation.
pub struct FilterProgram {
    insns: [sock_filter; MAX_BPF_INSNS],
    len: usize,
}

impl FilterProgram {
    const fn new() -> Self {
        Self { insns: [sock_filter { code: 0, jt: 0, jf: 0, k: 0 }; MAX_BPF_INSNS], len: 0 }
    }

    const fn push(&mut self, code: u16, jt: u8, jf: u8, k: u32) -> usize {
        assert!(self.len < MAX_BPF_INSNS, "seccomp BPF filter exceeded static capacity");
        let idx = self.len;
        self.insns[idx] = sock_filter { code, jt, jf, k };
        self.len += 1;
        idx
    }

    #[expect(clippy::cast_possible_truncation, reason = "verified <= u8::MAX before cast")]
    const fn patch_jt(&mut self, jump_idx: usize, target_idx: usize) {
        assert!(target_idx > jump_idx, "BPF forward jump must target a later instruction");
        let delta = target_idx - jump_idx - 1;
        assert!(delta <= u8::MAX as usize, "BPF jt offset overflow");
        self.insns[jump_idx].jt = delta as u8;
    }

    #[expect(clippy::cast_possible_truncation, reason = "verified <= u8::MAX before cast")]
    const fn patch_jf(&mut self, jump_idx: usize, target_idx: usize) {
        assert!(target_idx > jump_idx, "BPF forward jump must target a later instruction");
        let delta = target_idx - jump_idx - 1;
        assert!(delta <= u8::MAX as usize, "BPF jf offset overflow");
        self.insns[jump_idx].jf = delta as u8;
    }

    #[cfg(test)]
    #[must_use]
    pub fn as_slice(&self) -> &[sock_filter] {
        &self.insns[..self.len]
    }
}

/// Merges `BASE_TRAPPED_SYSCALLS` with `extra`, returning a sorted,
/// deduplicated array and its populated length.
///
/// # Errors
///
/// Returns [`Error::INVAL`] if the merged set exceeds [`MAX_TRAPPED_SYSCALLS`].
pub fn collect_trapped_syscalls(extra: &[Sysno]) -> Result<([u32; MAX_TRAPPED_SYSCALLS], usize)> {
    let mut out = [0_u32; MAX_TRAPPED_SYSCALLS];
    let mut len = 0_usize;

    let mut insert = |nr: u32| -> Result<()> {
        let mut pos = 0_usize;
        while pos < len {
            if out[pos] == nr {
                return Ok(());
            }
            if out[pos] > nr {
                break;
            }
            pos += 1;
        }
        if len >= MAX_TRAPPED_SYSCALLS {
            return Err(Error::INVAL);
        }
        let mut i = len;
        while i > pos {
            out[i] = out[i - 1];
            i -= 1;
        }
        out[pos] = nr;
        len += 1;
        Ok(())
    };

    for &nr in BASE_TRAPPED_SYSCALLS {
        insert(nr)?;
    }
    for &sysno in extra {
        insert(sysno as u32)?;
    }

    Ok((out, len))
}

/// Computes a 64-bit FNV-1a fingerprint over `(code_start, code_end,
/// sorted_trapped_syscalls)`.
#[must_use]
pub const fn compute_fingerprint(
    code_start: u64,
    code_end: u64,
    sorted_trapped_syscalls: &[u32],
) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    const fn mix_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
            i += 1;
        }
        hash
    }

    let mut hash = FNV_OFFSET_BASIS;
    hash = mix_bytes(hash, &code_start.to_le_bytes());
    hash = mix_bytes(hash, &code_end.to_le_bytes());
    let mut i = 0;
    while i < sorted_trapped_syscalls.len() {
        hash = mix_bytes(hash, &sorted_trapped_syscalls[i].to_le_bytes());
        i += 1;
    }
    hash
}

/// Builds the cBPF seccomp filter for `arch`, half-open launcher `PF_X` code
/// range `[code_start, code_end)`, idempotency `fingerprint`, and
/// `trapped_syscalls`.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    reason = "BPF constants/offsets fit in u8/u32 and keeping program assembly in one const fn preserves symbolic jump patching"
)]
pub const fn build_filter(
    arch: u32,
    code_start: u64,
    code_end: u64,
    fingerprint: u64,
    trapped_syscalls: &[u32],
) -> FilterProgram {
    assert!(code_start < code_end, "invalid launcher PF_X code range");
    assert!(
        trapped_syscalls.len() <= MAX_TRAPPED_SYSCALLS,
        "too many trapped syscalls for cBPF filter"
    );

    let mut prog = FilterProgram::new();

    // 1. Native architecture gate: reject foreign/compat architectures (e.g.
    //    i386 `int 0x80` on x86-64) with `ENOSYS` so they cannot bypass
    //    tracking or crash on foreign-ABI `execve`.
    prog.push(LD_W_ABS, 0, 0, OFF_ARCH);
    let j_arch = prog.push(JMP_JEQ_K, 0, 0, arch);
    prog.push(RET_K, 0, 0, SECCOMP_RET_ENOSYS);
    let after_arch = prog.len;
    prog.patch_jt(j_arch, after_arch);

    // Load `seccomp_data.nr` once; reused by the x32 gate and idempotency probe.
    prog.push(LD_W_ABS, 0, 0, OFF_NR);

    // 1b. On x86-64, x32 syscalls share `AUDIT_ARCH_X86_64` with bit 30 set
    //     (`nr >= 0x4000_0000`). Reject them with `ENOSYS` before any allowlist
    //     or trap dispatch so they cannot bypass tracking.
    if arch == AUDIT_ARCH_X86_64 {
        let j_x32 = prog.push(JMP_JGE_K, 0, 0, X32_SYSCALL_BIT);
        prog.push(RET_K, 0, 0, SECCOMP_RET_ENOSYS);
        let after_x32 = prog.len;
        prog.patch_jf(j_x32, after_x32);
    }

    // 2. Idempotency probe (runs BEFORE the PF_X IP allowlist):
    //    `getpid(PROBE_MAGIC, fingerprint)` -> `SECCOMP_RET_ERRNO | EOWNERDEAD`.
    //    `A` already holds `nr`.
    let magic_lo = PROBE_MAGIC as u32;
    let magic_hi = (PROBE_MAGIC >> 32) as u32;
    let fp_lo = fingerprint as u32;
    let fp_hi = (fingerprint >> 32) as u32;

    let j_probe_nr = prog.push(JMP_JEQ_K, 0, 0, Sysno::getpid as u32);
    prog.push(LD_W_ABS, 0, 0, OFF_ARG0_LO);
    let j_probe_a0_lo = prog.push(JMP_JEQ_K, 0, 0, magic_lo);
    prog.push(LD_W_ABS, 0, 0, OFF_ARG0_HI);
    let j_probe_a0_hi = prog.push(JMP_JEQ_K, 0, 0, magic_hi);
    prog.push(LD_W_ABS, 0, 0, OFF_ARG1_LO);
    let j_probe_a1_lo = prog.push(JMP_JEQ_K, 0, 0, fp_lo);
    prog.push(LD_W_ABS, 0, 0, OFF_ARG1_HI);
    let j_probe_a1_hi = prog.push(JMP_JEQ_K, 0, 0, fp_hi);
    prog.push(RET_K, 0, 0, SECCOMP_RET_PROBE);
    let after_probe = prog.len;
    prog.patch_jf(j_probe_nr, after_probe);
    prog.patch_jf(j_probe_a0_lo, after_probe);
    prog.patch_jf(j_probe_a0_hi, after_probe);
    prog.patch_jf(j_probe_a1_lo, after_probe);
    prog.patch_jf(j_probe_a1_hi, after_probe);

    // 3. Reject `clone3` and `io_uring_setup` with `ENOSYS`:
    //    - `clone3` callers using `CLONE_CLEAR_SIGHAND` would reset our
    //      `SIGSYS` handler to `SIG_DFL` in the child while keeping the
    //      seccomp filter active; `ENOSYS` makes glibc fall back to `clone(2)`.
    //    - `io_uring` operations bypass seccomp syscall traps entirely, so
    //      rejecting `io_uring_setup` ensures runtimes fall back to standard
    //      syscalls where `/proc/*/exe` virtualization and caller interceptors
    //      observe them.
    prog.push(LD_W_ABS, 0, 0, OFF_NR);
    let j_clone3 = prog.push(JMP_JEQ_K, 0, 0, Sysno::clone3 as u32);
    let j_io_uring = prog.push(JMP_JEQ_K, 0, 0, Sysno::io_uring_setup as u32);
    let ret_enosys = prog.push(RET_K, 0, 0, SECCOMP_RET_ENOSYS);
    let after_enosys = prog.len;
    prog.patch_jt(j_clone3, ret_enosys);
    prog.patch_jf(j_io_uring, after_enosys);

    // Collect indices of jumps that must target `ip_check_idx`.
    let mut on_true_to_ip = [0_usize; MAX_TRAPPED_SYSCALLS + 16];
    let mut true_to_ip_count = 0_usize;
    let mut on_false_to_ip = [0_usize; 16];
    let mut false_to_ip_count = 0_usize;

    // 4a. Conditional `clone` gate:
    //     - Always trap new-address-space `clone` where `(args[0] & CLONE_VM) == 0`
    //       (regardless of whether `child_stack` is NULL or non-NULL) so
    //       `on_fork_child` refreshes `OWNER_PID`, drops parent TID slots, and
    //       resets the action seqlock in every forked process.
    //     - Also trap `CLONE_VM | CLONE_VFORK` ONLY when `child_stack != 0`
    //       (separate child stack such as musl's 5 KiB `posix_spawn` stack,
    //       where AArch64 or AVX-512/AMX `rt_sigframe`s would overflow, or
    //       glibc AArch64 `vfork()` passing `child_stack == parent_sp`).
    //       Shared-stack `vfork` (`child_stack == 0`) MUST NOT be trapped so
    //       our `SIGSYS` frame is never placed below the parent's active SP.
    //     - Regular threads (`CLONE_VM` without `CLONE_VFORK`) allow directly.
    //     A holds `nr`.
    let j_not_clone = prog.push(JMP_JEQ_K, 0, 0, Sysno::clone as u32);
    prog.push(LD_W_ABS, 0, 0, OFF_ARG0_LO);
    let j_not_clone_vm = prog.push(JMP_JSET_K, 0, 0, CLONE_VM);
    on_false_to_ip[false_to_ip_count] = j_not_clone_vm;
    false_to_ip_count += 1;
    // `CLONE_VM` is set; only trap if `CLONE_VFORK` is also set AND `child_stack != 0`.
    let j_vfork = prog.push(JMP_JSET_K, 0, 0, CLONE_VFORK);
    prog.push(RET_K, 0, 0, SECCOMP_RET_ALLOW);
    let check_vfork_stack = prog.len;
    prog.patch_jt(j_vfork, check_vfork_stack);
    prog.push(LD_W_ABS, 0, 0, OFF_ARG1_LO);
    let j_stack_lo_nz = prog.push(JMP_JEQ_K, 0, 0, 0);
    on_false_to_ip[false_to_ip_count] = j_stack_lo_nz;
    false_to_ip_count += 1;
    prog.push(LD_W_ABS, 0, 0, OFF_ARG1_HI);
    let j_stack_hi_nz = prog.push(JMP_JEQ_K, 0, 0, 0);
    on_false_to_ip[false_to_ip_count] = j_stack_hi_nz;
    false_to_ip_count += 1;
    prog.push(RET_K, 0, 0, SECCOMP_RET_ALLOW);
    let after_clone = prog.len;
    prog.patch_jf(j_not_clone, after_clone);

    // 4b. Conditional masked-wait gates: trap `ppoll`, `pselect6`,
    //     `epoll_pwait`, and `epoll_pwait2` ONLY when the sigmask pointer
    //     argument is non-null.
    //     Note: `A` still holds `nr` on entry because we jumped here on `nr != clone`.
    let masked_waits: [(u32, u32, u32); 4] = [
        (Sysno::ppoll as u32, OFF_ARG3_LO, OFF_ARG3_HI),
        (Sysno::pselect6 as u32, OFF_ARG5_LO, OFF_ARG5_HI),
        (Sysno::epoll_pwait as u32, OFF_ARG4_LO, OFF_ARG4_HI),
        (Sysno::epoll_pwait2 as u32, OFF_ARG4_LO, OFF_ARG4_HI),
    ];
    let mut mw_i = 0;
    while mw_i < masked_waits.len() {
        let (nr, arg_lo, arg_hi) = masked_waits[mw_i];
        let j_not_mw = prog.push(JMP_JEQ_K, 0, 0, nr);
        prog.push(LD_W_ABS, 0, 0, arg_lo);
        let j_lo_nz = prog.push(JMP_JEQ_K, 0, 0, 0);
        on_false_to_ip[false_to_ip_count] = j_lo_nz;
        false_to_ip_count += 1;
        prog.push(LD_W_ABS, 0, 0, arg_hi);
        let j_hi_nz = prog.push(JMP_JEQ_K, 0, 0, 0);
        on_false_to_ip[false_to_ip_count] = j_hi_nz;
        false_to_ip_count += 1;
        prog.push(RET_K, 0, 0, SECCOMP_RET_ALLOW);
        let next_mw = prog.len;
        prog.patch_jf(j_not_mw, next_mw);
        mw_i += 1;
    }

    // 4c. Unconditional trapped-syscall dispatch: `A` still holds `nr`.
    let count = trapped_syscalls.len();
    let mut i = 0;
    while i < count {
        let j = prog.push(JMP_JEQ_K, 0, 0, trapped_syscalls[i]);
        on_true_to_ip[true_to_ip_count] = j;
        true_to_ip_count += 1;
        i += 1;
    }
    prog.push(RET_K, 0, 0, SECCOMP_RET_ALLOW);

    // 5. Launcher `PF_X` IP range check: allow if `code_start <= ip < code_end`,
    //    i.e. `code_start <= ip <= code_end - 1`, else `SECCOMP_RET_TAGGED_TRAP`.
    let ip_check_idx = prog.push(LD_W_ABS, 0, 0, OFF_IP_HI);

    // Patch all deferred jumps to `ip_check_idx`.
    let mut k = 0;
    while k < true_to_ip_count {
        prog.patch_jt(on_true_to_ip[k], ip_check_idx);
        k += 1;
    }
    let mut k = 0;
    while k < false_to_ip_count {
        prog.patch_jf(on_false_to_ip[k], ip_check_idx);
        k += 1;
    }

    let end_inclusive = code_end - 1;
    let start_lo = code_start as u32;
    let start_hi = (code_start >> 32) as u32;
    let end_lo = end_inclusive as u32;
    let end_hi = (end_inclusive >> 32) as u32;

    assert!(start_hi == end_hi, "launcher PF_X code range must not cross a 4 GiB boundary");

    let j_ip_hi = prog.push(JMP_JEQ_K, 0, 0, start_hi);
    prog.push(LD_W_ABS, 0, 0, OFF_IP_LO);
    let j_ip_ge_start = prog.push(JMP_JGE_K, 0, 0, start_lo);
    let j_ip_gt_end = prog.push(JMP_JGT_K, 0, 0, end_lo);
    prog.push(RET_K, 0, 0, SECCOMP_RET_ALLOW);
    let ret_trap = prog.push(RET_K, 0, 0, SECCOMP_RET_TAGGED_TRAP);
    prog.patch_jf(j_ip_hi, ret_trap);
    prog.patch_jf(j_ip_ge_start, ret_trap);
    prog.patch_jt(j_ip_gt_end, ret_trap);

    prog
}

/// Probes whether an identical seccomp filter with `fingerprint` is already
/// installed on the current process.
#[expect(clippy::cast_possible_truncation, reason = "64-bit Linux native pointer/word size")]
fn probe_installed_filter(fingerprint: u64) -> bool {
    // SAFETY: `getpid` ignores extra arguments when no matching filter is
    // installed; a matching filter returns `-EOWNERDEAD` directly.
    let rc = unsafe {
        syscalls::raw::syscall2(Sysno::getpid as usize, PROBE_MAGIC as usize, fingerprint as usize)
    }
    .cast_signed();
    rc == neg_errno(EOWNERDEAD)
}

/// Installs the seccomp-BPF filter unless an identical filter (matching
/// `code_start`, `code_end`, and the merged trapped syscall set) is already
/// active on the process.
///
/// # Errors
///
/// Returns [`Error::INVAL`] if the syscall set exceeds capacity, or the kernel
/// error if `prctl` or `seccomp` fails.
pub fn install_seccomp_filter(code_start: u64, code_end: u64, extra: &[Sysno]) -> Result<()> {
    let (syscalls_buf, count) = collect_trapped_syscalls(extra)?;
    let trapped = &syscalls_buf[..count];
    let fingerprint = compute_fingerprint(code_start, code_end, trapped);

    if probe_installed_filter(fingerprint) {
        return Ok(());
    }

    // SAFETY: `PR_SET_NO_NEW_PRIVS` takes integer arguments only and enables
    // unprivileged seccomp filter installation.
    let rc =
        unsafe { syscall6(Sysno::prctl as usize, PR_SET_NO_NEW_PRIVS as usize, 1, 0, 0, 0, 0) };
    check_syscall_result(rc)?;

    let mut filter = build_filter(AUDIT_ARCH_NATIVE, code_start, code_end, fingerprint, trapped);
    let len = u16::try_from(filter.len).map_err(|_| Error::INVAL)?;
    let prog = sock_fprog { len, filter: filter.insns.as_mut_ptr() };

    // SAFETY: `prog` points to `len` initialized `sock_filter` instructions for
    // the duration of the `seccomp` syscall.
    let rc = unsafe {
        syscall6(
            Sysno::seccomp as usize,
            SECCOMP_SET_MODE_FILTER as usize,
            0,
            core::ptr::from_ref(&prog).addr(),
            0,
            0,
            0,
        )
    };
    check_syscall_result(rc)
}

#[expect(clippy::cast_possible_truncation, reason = "Linux errno values fit in i32")]
const fn check_syscall_result(rc: isize) -> Result<()> {
    if rc < 0 { Err(Error::from_raw_os_error((-rc) as i32)) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_bpf(prog: &[sock_filter], data: &seccomp_data) -> u32 {
        // SAFETY: `seccomp_data` is a plain `#[repr(C)]` struct of integer fields.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                core::ptr::from_ref(data).cast::<u8>(),
                core::mem::size_of::<seccomp_data>(),
            )
        };
        let mut pc = 0_usize;
        let mut acc = 0_u32;
        loop {
            let insn = prog[pc];
            pc += 1;
            match insn.code {
                LD_W_ABS => {
                    let off = insn.k as usize;
                    acc = u32::from_ne_bytes(bytes[off..off + 4].try_into().unwrap());
                }
                JMP_JEQ_K => pc += usize::from(if acc == insn.k { insn.jt } else { insn.jf }),
                JMP_JGE_K => pc += usize::from(if acc >= insn.k { insn.jt } else { insn.jf }),
                JMP_JGT_K => pc += usize::from(if acc > insn.k { insn.jt } else { insn.jf }),
                JMP_JSET_K => {
                    pc += usize::from(if (acc & insn.k) != 0 { insn.jt } else { insn.jf });
                }
                RET_K => return insn.k,
                other => panic!("unexpected BPF opcode: {other:#x}"),
            }
        }
    }

    fn make_data(arch: u32, ip: u64, nr: u32, args: [u64; 6]) -> seccomp_data {
        seccomp_data { nr: nr.cast_signed(), arch, instruction_pointer: ip, args }
    }

    #[test]
    fn filter_allows_pf_x_range_and_traps_listed_syscalls_same_hi() {
        let code_start = 0x5500_0000_1000_u64;
        let code_end = 0x5500_0000_9000_u64;
        let (buf, count) = collect_trapped_syscalls(&[]).unwrap();
        let trapped = &buf[..count];
        let fp = compute_fingerprint(code_start, code_end, trapped);
        let prog = build_filter(AUDIT_ARCH_NATIVE, code_start, code_end, fp, trapped);
        let insns = prog.as_slice();

        // Foreign arch always returns `ENOSYS` (even for matching probe or trapped syscall).
        assert_eq!(
            eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE ^ 1, 0x1000, trapped[0], [0; 6])),
            SECCOMP_RET_ENOSYS
        );
        assert_eq!(
            eval_bpf(
                insns,
                &make_data(
                    AUDIT_ARCH_NATIVE ^ 1,
                    code_start + 8,
                    Sysno::getpid as u32,
                    [PROBE_MAGIC, fp, 0, 0, 0, 0],
                ),
            ),
            SECCOMP_RET_ENOSYS
        );

        // Inside half-open `[code_start, code_end)` is allowed even for trapped syscalls.
        for ip in [code_start, code_start + 1, code_end - 1] {
            for &nr in trapped {
                assert_eq!(
                    eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE, ip, nr, [0; 6])),
                    SECCOMP_RET_ALLOW,
                    "expected ALLOW inside PF_X for ip={ip:#x} nr={nr}"
                );
            }
        }

        // `clone3` and `io_uring_setup` always return `SECCOMP_RET_ENOSYS` on the native arch.
        for nr in [Sysno::clone3 as u32, Sysno::io_uring_setup as u32] {
            for ip in [0, code_start, code_end, 0x0000_7fff_0000_0000] {
                assert_eq!(
                    eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE, ip, nr, [0; 6])),
                    SECCOMP_RET_ENOSYS,
                    "expected ENOSYS for nr={nr} at ip={ip:#x}"
                );
            }
        }

        // Outside `[code_start, code_end)` (including `code_end` itself!) is trapped with `TRAP_TAG`.
        for ip in [0, code_start - 1, code_end, code_end + 1, 0x0000_7fff_0000_0000, u64::MAX] {
            for &nr in trapped {
                assert_eq!(
                    eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE, ip, nr, [0; 6])),
                    SECCOMP_RET_TAGGED_TRAP,
                    "expected tagged TRAP outside PF_X for ip={ip:#x} nr={nr}"
                );
            }
            // Untrapped hot syscalls are always allowed.
            for nr in [
                Sysno::write as u32,
                Sysno::read as u32,
                Sysno::exit_group as u32,
                Sysno::rt_sigreturn as u32,
                Sysno::mmap as u32,
            ] {
                assert_eq!(
                    eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE, ip, nr, [0; 6])),
                    SECCOMP_RET_ALLOW,
                    "expected ALLOW for untrapped ip={ip:#x} nr={nr}"
                );
            }
        }
    }

    #[test]
    fn conditional_gates_filter_clone_vm() {
        let code_start = 0x5500_0000_1000_u64;
        let code_end = 0x5500_0000_9000_u64;
        let (buf, count) = collect_trapped_syscalls(&[]).unwrap();
        let trapped = &buf[..count];
        let fp = compute_fingerprint(code_start, code_end, trapped);
        let prog = build_filter(AUDIT_ARCH_NATIVE, code_start, code_end, fp, trapped);
        let insns = prog.as_slice();
        let user_ip = 0x0000_7fff_0000_1000;
        let launcher_ip = code_start + 0x10;

        let eval_clone = |ip: u64, flags: u64, stack: u64| {
            eval_bpf(
                insns,
                &make_data(AUDIT_ARCH_NATIVE, ip, Sysno::clone as u32, [flags, stack, 0, 0, 0, 0]),
            )
        };

        // `clone`: allow regular threads (`CLONE_VM` without `CLONE_VFORK`);
        // trap ALL new-address-space clones (`(flags & CLONE_VM) == 0`, whether
        // `child_stack` is NULL or non-NULL) outside PF_X.
        assert_eq!(eval_clone(user_ip, u64::from(CLONE_VM), 0), SECCOMP_RET_ALLOW);
        assert_eq!(eval_clone(user_ip, u64::from(CLONE_VM), 0x7fff_0000_0000), SECCOMP_RET_ALLOW);
        assert_eq!(eval_clone(user_ip, 0x11, 0), SECCOMP_RET_TAGGED_TRAP);
        assert_eq!(eval_clone(user_ip, 0x11, 0x7fff_0000_0000), SECCOMP_RET_TAGGED_TRAP);
        assert_eq!(eval_clone(user_ip, 0x11, 0x1_0000_0000), SECCOMP_RET_TAGGED_TRAP);
        assert_eq!(eval_clone(launcher_ip, 0x11, 0), SECCOMP_RET_ALLOW);
        let vfork_flags = u64::from(CLONE_VM | CLONE_VFORK | 0x11);
        // Shared-stack `vfork` (`child_stack == 0`) MUST allow directly so our
        // SIGSYS frame never sits below the shared parent SP.
        assert_eq!(eval_clone(user_ip, vfork_flags, 0), SECCOMP_RET_ALLOW);
        // Separate-stack `vfork`/`posix_spawn` (`child_stack != 0`, low or high
        // 32 bits) MUST trap outside PF_X so the child gets a safe sigaltstack.
        for stack_ptr in [0x0000_0000_0000_8000_u64, 0x0000_7fff_0000_0000_u64] {
            assert_eq!(eval_clone(user_ip, vfork_flags, stack_ptr), SECCOMP_RET_TAGGED_TRAP);
            assert_eq!(eval_clone(launcher_ip, vfork_flags, stack_ptr), SECCOMP_RET_ALLOW);
        }
    }

    #[test]
    fn conditional_gates_filter_null_masked_waits() {
        let code_start = 0x5500_0000_1000_u64;
        let code_end = 0x5500_0000_9000_u64;
        let (buf, count) = collect_trapped_syscalls(&[]).unwrap();
        let trapped = &buf[..count];
        let fp = compute_fingerprint(code_start, code_end, trapped);
        let prog = build_filter(AUDIT_ARCH_NATIVE, code_start, code_end, fp, trapped);
        let insns = prog.as_slice();
        let user_ip = 0x0000_7fff_0000_1000;
        let launcher_ip = code_start + 0x10;

        // Masked waits: NULL sigmask allows directly; non-null (low or high 32-bit) traps outside PF_X.
        for (nr, mask_idx) in [
            (Sysno::ppoll as u32, 3_usize),
            (Sysno::pselect6 as u32, 5_usize),
            (Sysno::epoll_pwait as u32, 4_usize),
            (Sysno::epoll_pwait2 as u32, 4_usize),
        ] {
            assert_eq!(
                eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE, user_ip, nr, [0; 6])),
                SECCOMP_RET_ALLOW,
                "NULL mask must allow directly for nr={nr}"
            );
            for mask_ptr in
                [0x0000_0000_0000_1234_u64, 0x0000_7fff_0000_0000_u64, 0x0000_7fff_1234_5678_u64]
            {
                let mut args = [0_u64; 6];
                args[mask_idx] = mask_ptr;
                assert_eq!(
                    eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE, user_ip, nr, args)),
                    SECCOMP_RET_TAGGED_TRAP,
                    "non-null mask {mask_ptr:#x} must trap outside PF_X for nr={nr}"
                );
                assert_eq!(
                    eval_bpf(insns, &make_data(AUDIT_ARCH_NATIVE, launcher_ip, nr, args)),
                    SECCOMP_RET_ALLOW,
                    "non-null mask inside PF_X must allow for nr={nr}"
                );
            }
        }
    }

    #[test]
    fn getpid_probe_matches_before_pf_x_check_and_ignores_mismatches() {
        let code_start = 0x0002_0000_1000_u64;
        let code_end = 0x0002_0000_8000_u64;
        let (buf, count) = collect_trapped_syscalls(&[]).unwrap();
        let trapped = &buf[..count];
        let fp = compute_fingerprint(code_start, code_end, trapped);
        let prog = build_filter(AUDIT_ARCH_NATIVE, code_start, code_end, fp, trapped);
        let insns = prog.as_slice();

        for ip in [code_start + 0x20, 0x7fff_0000_0000] {
            assert_eq!(
                eval_bpf(
                    insns,
                    &make_data(
                        AUDIT_ARCH_NATIVE,
                        ip,
                        Sysno::getpid as u32,
                        [PROBE_MAGIC, fp, 0, 0, 0, 0],
                    ),
                ),
                SECCOMP_RET_PROBE
            );
        }

        for args in [
            [0, 0, 0, 0, 0, 0],
            [PROBE_MAGIC ^ 1, fp, 0, 0, 0, 0],
            [PROBE_MAGIC ^ (1 << 32), fp, 0, 0, 0, 0],
            [PROBE_MAGIC, fp ^ 1, 0, 0, 0, 0],
            [PROBE_MAGIC, fp ^ (1 << 32), 0, 0, 0, 0],
        ] {
            assert_eq!(
                eval_bpf(
                    insns,
                    &make_data(AUDIT_ARCH_NATIVE, code_start + 0x20, Sysno::getpid as u32, args),
                ),
                SECCOMP_RET_ALLOW
            );
        }

        let (buf2, count2) = collect_trapped_syscalls(&[Sysno::getpid]).unwrap();
        let trapped2 = &buf2[..count2];
        let fp2 = compute_fingerprint(code_start, code_end, trapped2);
        let prog2 = build_filter(AUDIT_ARCH_NATIVE, code_start, code_end, fp2, trapped2);
        let insns2 = prog2.as_slice();
        assert_eq!(
            eval_bpf(
                insns2,
                &make_data(
                    AUDIT_ARCH_NATIVE,
                    code_start + 0x20,
                    Sysno::getpid as u32,
                    [PROBE_MAGIC, fp2, 0, 0, 0, 0],
                ),
            ),
            SECCOMP_RET_PROBE
        );
        assert_eq!(
            eval_bpf(insns2, &make_data(AUDIT_ARCH_NATIVE, 0x1000, Sysno::getpid as u32, [0; 6])),
            SECCOMP_RET_TAGGED_TRAP
        );
        assert_eq!(
            eval_bpf(
                insns2,
                &make_data(AUDIT_ARCH_NATIVE, code_start + 0x20, Sysno::getpid as u32, [0; 6]),
            ),
            SECCOMP_RET_ALLOW
        );
    }

    #[test]
    #[should_panic(expected = "4 GiB")]
    fn filter_rejects_pf_x_crossing_4gib_boundary() {
        let code_start = 0x0000_0001_ffff_fffe_u64;
        let code_end = 0x0000_0003_0000_0003_u64;
        let trapped = &[Sysno::openat as u32];
        let fp = compute_fingerprint(code_start, code_end, trapped);
        let _ = build_filter(AUDIT_ARCH_NATIVE, code_start, code_end, fp, trapped);
    }

    #[test]
    fn collect_deduplicates_sorts_and_fingerprints_deterministically() {
        let (a, len_a) =
            collect_trapped_syscalls(&[Sysno::getegid, Sysno::openat, Sysno::unlinkat]).unwrap();
        let (b, len_b) =
            collect_trapped_syscalls(&[Sysno::unlinkat, Sysno::getegid, Sysno::getegid]).unwrap();
        assert_eq!(len_a, len_b);
        assert_eq!(&a[..len_a], &b[..len_b]);
        assert!(a[..len_a].windows(2).all(|w| w[0] < w[1]));

        let fp1 = compute_fingerprint(0x2_0000_1000, 0x2_0000_5000, &a[..len_a]);
        let fp2 = compute_fingerprint(0x2_0000_1000, 0x2_0000_5000, &b[..len_b]);
        let fp_diff_range = compute_fingerprint(0x2_0000_1000, 0x2_0000_6000, &a[..len_a]);
        let (c, len_c) = collect_trapped_syscalls(&[]).unwrap();
        let fp_diff_syscalls = compute_fingerprint(0x2_0000_1000, 0x2_0000_5000, &c[..len_c]);
        assert_eq!(fp1, fp2);
        assert_ne!(fp1, fp_diff_range);
        assert_ne!(fp1, fp_diff_syscalls);
    }

    #[test]
    fn filter_rejects_foreign_arch_and_x32_syscalls_with_enosys() {
        let code_start = 0x5500_0000_1000_u64;
        let code_end = 0x5500_0000_9000_u64;
        let (buf, count) = collect_trapped_syscalls(&[]).unwrap();
        let trapped = &buf[..count];
        let fp = compute_fingerprint(code_start, code_end, trapped);

        let prog_x86 = build_filter(AUDIT_ARCH_X86_64, code_start, code_end, fp, trapped);
        let insns_x86 = prog_x86.as_slice();

        // Compat i386 (`AUDIT_ARCH_I386`) and arbitrary foreign arch return `ENOSYS`.
        for foreign_arch in [linux_raw_sys::ptrace::AUDIT_ARCH_I386, AUDIT_ARCH_X86_64 ^ 1] {
            for ip in [0x1000, code_start + 8] {
                assert_eq!(
                    eval_bpf(insns_x86, &make_data(foreign_arch, ip, 1, [0; 6])),
                    SECCOMP_RET_ENOSYS
                );
            }
        }

        // x32 syscalls (`nr >= 0x4000_0000`) return `ENOSYS` even from `PF_X`
        // or with matching `getpid` probe arguments.
        for nr in [
            X32_SYSCALL_BIT,
            X32_SYSCALL_BIT | (Sysno::getpid as u32),
            X32_SYSCALL_BIT | (Sysno::openat as u32),
            X32_SYSCALL_BIT | (Sysno::execve as u32),
            u32::MAX,
        ] {
            for ip in [0x1000, code_start + 8] {
                assert_eq!(
                    eval_bpf(
                        insns_x86,
                        &make_data(AUDIT_ARCH_X86_64, ip, nr, [PROBE_MAGIC, fp, 0, 0, 0, 0]),
                    ),
                    SECCOMP_RET_ENOSYS,
                    "expected ENOSYS for x32 nr={nr:#x} at ip={ip:#x}"
                );
            }
        }

        // Native x86-64 syscalls below `X32_SYSCALL_BIT` keep working normally.
        assert_eq!(
            eval_bpf(
                insns_x86,
                &make_data(
                    AUDIT_ARCH_X86_64,
                    0x1000,
                    Sysno::getpid as u32,
                    [PROBE_MAGIC, fp, 0, 0, 0, 0],
                ),
            ),
            SECCOMP_RET_PROBE
        );
        assert_eq!(
            eval_bpf(insns_x86, &make_data(AUDIT_ARCH_X86_64, 0x1000, trapped[0], [0; 6])),
            SECCOMP_RET_TAGGED_TRAP
        );
        assert_eq!(
            eval_bpf(insns_x86, &make_data(AUDIT_ARCH_X86_64, 0x1000, Sysno::write as u32, [0; 6])),
            SECCOMP_RET_ALLOW
        );
    }
}
