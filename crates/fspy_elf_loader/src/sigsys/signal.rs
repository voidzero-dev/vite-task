//! `SIGSYS` signal disposition, signal-mask, pending-signal, and fork/vfork
//! virtualization.
//!
//! Seccomp `SECCOMP_RET_TRAP` requires the kernel's `SIGSYS` disposition to
//! stay pointing at our dispatcher and `SIGSYS` to stay unblocked in every
//! thread's kernel signal mask (blocking `SIGSYS` causes the kernel to kill the
//! process when a trapped syscall executes).
//!
//! This module virtualizes `SIGSYS` across:
//! - `rt_sigaction`: stores a process-wide virtualized `SIGSYS` action without
//!   modifying the kernel's `SIGSYS` disposition, strips `SIGSYS` from
//!   `sa_mask` of other signals while preserving it on read-back, ignores
//!   writes from `CLONE_VM` (`vfork`/`posix_spawn`) children so they cannot
//!   corrupt the parent's signal table, and forwards non-seccomp or foreign
//!   `SIGSYS` deliveries to the virtualized disposition.
//! - `rt_sigprocmask` & `rt_sigpending`: updates `ucontext.uc_sigmask` directly
//!   so `rt_sigreturn` commits non-`SIGSYS` bits atomically, tracks per-TID
//!   virtual `SIGSYS` blocked and pending state in a lock-free `(pid, tid)`
//!   table with automatic dead-TID reaping, and defers/delivers non-seccomp
//!   `SIGSYS` when blocked/unblocked.
//! - Masked-wait syscalls (`rt_sigsuspend`, `ppoll`, `pselect6`,
//!   `epoll_pwait`, `epoll_pwait2`): strips `SIGSYS` from temporary kernel wait
//!   masks while honoring `SIGSYS` blocking/unblocking in virtualized state.
//! - True `fork()` (`fork` / `clone` without `CLONE_VM`): updates process
//!   ownership, resets `ACTION_LOCK`, drops parent sibling TIDs, and inherits
//!   the calling thread's virtual `SIGSYS` blocked bit.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use fspy_nostd::{Error, Result};
use linux_raw_sys::{
    errno::{EFAULT, EINTR, EINVAL, ENOMEM, ESRCH},
    general::{
        CLOCK_MONOTONIC, P_PID, SA_NODEFER, SA_RESETHAND, SA_SIGINFO, SIG_BLOCK, SIG_SETMASK,
        SIG_UNBLOCK, SIGSYS, SYS_SECCOMP, WEXITED, WNOHANG, WNOWAIT, siginfo, timespec,
    },
};
use syscalls::Sysno;

use super::{
    arch::{
        KERNEL_SIGSET_SIZE, KernelSigaction, SIGSYS_BIT, SyscallInvocation, UNBLOCKABLE_MASK,
        Ucontext, arg_i32, getpid, gettid, invoke_user_sigsys_handler, masked_wait_return_pc,
        neg_errno, replay, replay_masked_wait, rt_sigaction, rt_sigprocmask, syscall6,
        uc_sigmask_mut, ucontext_pc,
    },
    usermem,
};

const SIG_DFL_RAW: usize = 0;
const SIG_IGN_RAW: usize = 1;
const SIGSYS_I32: i32 = SIGSYS.cast_signed();

/// Capacity of the lock-free `(pid, tid)` virtual `SIGSYS` state table
/// (16 KiB BSS; dead TIDs are automatically reaped on pressure).
const MAX_TID_SLOTS: usize = 2048;

/// Capacity of the lock-free pending `siginfo` side table (8 KiB BSS).
const MAX_PENDING_INFOS: usize = 64;

const KEY_BITS: u32 = 30;
const KEY_PART_MASK: u64 = (1_u64 << KEY_BITS) - 1;
const KEY_MASK: u64 = (1_u64 << (KEY_BITS * 2)) - 1;
const BIT_BLOCKED: u64 = 1_u64 << 60;
const BIT_PENDING: u64 = 1_u64 << 61;
const INFO_LOCK_BIT: u64 = 1_u64 << 63;

struct SyncCell<T>(UnsafeCell<T>);

// SAFETY: Access is serialized by `ACTION_LOCK` (for `USER_SIGSYS_ACTION`) or
// per-slot `INFO_LOCK_BIT` (for `PENDING_INFOS`).
unsafe impl<T> Sync for SyncCell<T> {}

/// Owning PID of this address space (set in `init` and refreshed in `fork`
/// children). Any thread observing `getpid() != OWNER_PID` is executing inside
/// a `CLONE_VM` (`vfork` / `posix_spawn`) child sharing the parent's memory.
static OWNER_PID: AtomicI32 = AtomicI32::new(0);

/// Owner `(pid, tid)` key holding `USER_SIGSYS_ACTION` (`0` = unlocked).
/// Storing the owner key lets waiters verify liveness via `tgkill`/`kill` and
/// reclaim the lock ONLY if the previous owner provably died (e.g. a
/// `CLONE_VM` child killed by `SIGKILL`), without ever racing a live holder.
static ACTION_LOCK: AtomicU64 = AtomicU64::new(0);
static USER_SIGSYS_ACTION: SyncCell<KernelSigaction> = SyncCell(UnsafeCell::new(KernelSigaction {
    sa_handler: SIG_DFL_RAW,
    sa_flags: 0,
    sa_restorer: 0,
    sa_mask: 0,
}));

/// Bit `sig - 1` records whether the target included `SIGSYS` in `sa_mask`
/// when installing a handler for signal `sig` (`1..=64`, `sig != SIGSYS`).
static SA_MASK_HAS_SIGSYS: AtomicU64 = AtomicU64::new(0);

/// Lock-free per-TID virtual `SIGSYS` state:
/// - `0` => empty slot
/// - bits `0..30`: Linux TID
/// - bits `30..60`: Linux PID
/// - bit `60`: `BIT_BLOCKED`
/// - bit `61`: `BIT_PENDING`
static TID_SLOTS: [AtomicU64; MAX_TID_SLOTS] = [const { AtomicU64::new(0) }; MAX_TID_SLOTS];
static OCCUPIED_SLOTS: AtomicUsize = AtomicUsize::new(0);

struct PendingInfoSlot {
    key: AtomicU64,
    info: SyncCell<MaybeUninit<siginfo>>,
}

static PENDING_INFOS: [PendingInfoSlot; MAX_PENDING_INFOS] = [const {
    PendingInfoSlot {
        key: AtomicU64::new(0),
        info: SyncCell(UnsafeCell::new(MaybeUninit::uninit())),
    }
}; MAX_PENDING_INFOS];

/// Per-slot monotonic counter incremented whenever a non-seccomp `SIGSYS` is
/// deferred on that `(pid, tid)` slot, allowing masked waits on one thread to
/// detect spurious `-EINTR` without false retries caused by deferred `SIGSYS`
/// arriving on other threads.
static TID_DEFERRED_SEQ: [AtomicU32; MAX_TID_SLOTS] = [const { AtomicU32::new(0) }; MAX_TID_SLOTS];

/// Bounded spin budget for `INFO_LOCK_BIT` so a `CLONE_VM` child killed while
/// updating a pending `siginfo` slot cannot hang another thread forever.
const MAX_INFO_LOCK_SPINS: usize = 1_000_000;

/// Outcome of `handle_rt_sigprocmask`.
pub enum SigprocmaskOutcome {
    Done(isize),
    DeliverPendingSigsys(isize),
}

/// `pselect6` argument 6 points to this two-word structure.
#[derive(Clone, Copy)]
#[repr(C)]
struct PselectSigmask {
    ss: *const u64,
    ss_len: usize,
}

/// Initializes virtualized `SIGSYS` state from the process's pre-install
/// kernel `SIGSYS` action and inherited signal mask.
pub fn init(inherited_action: KernelSigaction, inherited_sigsys_blocked: bool) {
    let pid = getpid();
    OWNER_PID.store(pid, Ordering::Release);
    ACTION_LOCK.store(0, Ordering::Release);
    with_action_lock(|slot| {
        *slot = inherited_action;
    });
    SA_MASK_HAS_SIGSYS.store(0, Ordering::Release);
    clear_all_tid_slots();
    if inherited_sigsys_blocked {
        let _ = set_tid_blocked(pid, gettid(), true);
    }
}

/// Returns `true` when the calling thread belongs to a `CLONE_VM` child
/// (`vfork` / `posix_spawn`) sharing its parent's address space.
#[inline]
#[must_use]
pub fn is_shared_vm_child() -> bool {
    let owner = OWNER_PID.load(Ordering::Acquire);
    owner > 0 && getpid() != owner
}

/// Resets process-owned state in a freshly `fork()`'d child (separate COW
/// address space), dropping parent sibling TIDs, releasing any spinlock held
/// by a sibling thread at `fork()` time, and inheriting the calling thread's
/// virtual `SIGSYS` blocked state.
pub fn on_fork_child(parent_blocked: bool) {
    let pid = getpid();
    OWNER_PID.store(pid, Ordering::Release);
    ACTION_LOCK.store(0, Ordering::Release);
    clear_all_tid_slots();
    if parent_blocked {
        let _ = set_tid_blocked(pid, gettid(), true);
    }
}

/// Captures and removes the current thread's `TID_SLOTS` entry prior to
/// `SYS_execve` (so `vfork` / `posix_spawn` children do not leak a TID slot in
/// the parent's address space). Returns the previous entry flags to pass to
/// [`restore_current_tid_entry`] if `SYS_execve` fails.
pub fn take_current_tid_entry() -> Option<(u64, Option<siginfo>)> {
    if OCCUPIED_SLOTS.load(Ordering::Acquire) == 0 {
        return None;
    }
    let key = make_key(getpid(), gettid())?;
    let prev_flags = update_tid_flags(key, 0, BIT_BLOCKED | BIT_PENDING).ok()?;
    if prev_flags == 0 {
        return None;
    }
    let info = if (prev_flags & BIT_PENDING) != 0 { take_pending_info(key) } else { None };
    Some((prev_flags, info))
}

/// Restores a TID entry removed by [`take_current_tid_entry`] after a failed
/// `SYS_execve`.
pub fn restore_current_tid_entry(saved: Option<(u64, Option<siginfo>)>) {
    let Some((flags, info)) = saved else {
        return;
    };
    let Some(key) = make_key(getpid(), gettid()) else {
        return;
    };
    if let Some(info) = info {
        store_pending_info(key, info);
    }
    let _ = update_tid_flags(key, flags, 0);
}

fn with_action_lock<R>(f: impl FnOnce(&mut KernelSigaction) -> R) -> R {
    // Block ALL maskable signals (including `SIGSYS`) across the brief
    // critical section so an async non-seccomp `SIGSYS` (`tgkill`/`kill`)
    // cannot re-enter `with_action_lock` on the same thread and self-deadlock.
    // The closure performs only in-memory copies; contention wait-loop syscalls
    // (`waitid`/`kill`/`tgkill`/`sched_yield`) run from our allowlisted `PF_X`
    // range (note: a target-installed seccomp filter trapping those syscalls
    // during contention would see `SIGSYS` blocked briefly).
    let block_all = !UNBLOCKABLE_MASK;
    let mut saved: u64 = 0;
    // SAFETY: `block_all` and `saved` are valid stack-local `u64` values.
    let masked = unsafe { rt_sigprocmask(SIG_SETMASK, &raw const block_all, &raw mut saved) } == 0;
    let my_pid = getpid();
    let my_key = make_key(my_pid, gettid()).unwrap_or(1);
    let mut spins = 0_usize;
    loop {
        match ACTION_LOCK.compare_exchange_weak(0, my_key, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => break,
            Err(owner) => {
                spins += 1;
                if spins.trailing_zeros() >= 10 {
                    // Check whether the lock holder is provably dead (including
                    // an unreaped zombie `CLONE_VM` child killed by `SIGKILL`).
                    // Never steal from a live preempted thread.
                    if owner != 0
                        && owner != my_key
                        && !is_tid_alive(my_pid, slot_pid(owner), slot_tid(owner))
                        && ACTION_LOCK
                            .compare_exchange(owner, my_key, Ordering::AcqRel, Ordering::Relaxed)
                            .is_ok()
                    {
                        break;
                    }
                    // SAFETY: `sched_yield` takes no arguments and is async-signal-safe.
                    let _ = unsafe { syscall6(Sysno::sched_yield as usize, 0, 0, 0, 0, 0, 0) };
                } else {
                    core::hint::spin_loop();
                }
            }
        }
    }
    // SAFETY: Exclusively serialized by `ACTION_LOCK` (`my_key`).
    let result = unsafe { f(&mut *USER_SIGSYS_ACTION.0.get()) };
    ACTION_LOCK.store(0, Ordering::Release);
    if masked {
        // SAFETY: `saved` is a valid stack-local `u64`.
        let _ = unsafe { rt_sigprocmask(SIG_SETMASK, &raw const saved, ptr::null_mut()) };
    }
    result
}

/// Combines the thread's kernel signal mask from `ucontext.uc_sigmask` with
/// the virtualized `SIGSYS` blocked bit for the current thread.
#[must_use]
pub fn virtual_sigmask_from(kernel_mask: u64) -> u64 {
    let base = kernel_mask & !UNBLOCKABLE_MASK;
    if is_current_tid_blocked() { base | SIGSYS_BIT } else { base & !SIGSYS_BIT }
}

const fn make_key(pid: i32, tid: i32) -> Option<u64> {
    if pid <= 0 || tid <= 0 {
        return None;
    }
    let p = pid.cast_unsigned() as u64;
    let t = tid.cast_unsigned() as u64;
    if p > KEY_PART_MASK || t > KEY_PART_MASK {
        return None;
    }
    Some((p << KEY_BITS) | t)
}

const fn slot_pid(val: u64) -> i32 {
    ((val >> KEY_BITS) & KEY_PART_MASK) as i32
}

const fn slot_tid(val: u64) -> i32 {
    (val & KEY_PART_MASK) as i32
}

fn clear_all_tid_slots() {
    for (slot, seq) in TID_SLOTS.iter().zip(TID_DEFERRED_SEQ.iter()) {
        slot.store(0, Ordering::Relaxed);
        seq.store(0, Ordering::Relaxed);
    }
    for slot in &PENDING_INFOS {
        slot.key.store(0, Ordering::Relaxed);
    }
    OCCUPIED_SLOTS.store(0, Ordering::Release);
}

fn find_tid_slot_index(pid: i32, tid: i32) -> Option<usize> {
    if OCCUPIED_SLOTS.load(Ordering::Acquire) == 0 {
        return None;
    }
    let key = make_key(pid, tid)?;
    for (idx, slot) in TID_SLOTS.iter().enumerate() {
        let val = slot.load(Ordering::Acquire);
        if val != 0 && (val & KEY_MASK) == key {
            return Some(idx);
        }
    }
    None
}

fn read_tid_deferred_seq(pid: i32, tid: i32) -> u32 {
    find_tid_slot_index(pid, tid).map_or(0, |idx| TID_DEFERRED_SEQ[idx].load(Ordering::Acquire))
}

fn bump_tid_deferred_seq(pid: i32, tid: i32) {
    if let Some(idx) = find_tid_slot_index(pid, tid) {
        TID_DEFERRED_SEQ[idx].fetch_add(1, Ordering::Release);
    }
}

fn lookup_tid_flags(pid: i32, tid: i32) -> u64 {
    if OCCUPIED_SLOTS.load(Ordering::Acquire) == 0 {
        return 0;
    }
    let Some(key) = make_key(pid, tid) else {
        return 0;
    };
    for slot in &TID_SLOTS {
        let val = slot.load(Ordering::Acquire);
        if val != 0 && (val & KEY_MASK) == key {
            return val & (BIT_BLOCKED | BIT_PENDING);
        }
    }
    0
}

#[must_use]
pub fn is_current_tid_blocked() -> bool {
    if OCCUPIED_SLOTS.load(Ordering::Acquire) == 0 {
        return false;
    }
    (lookup_tid_flags(getpid(), gettid()) & BIT_BLOCKED) != 0
}

fn is_current_tid_pending() -> bool {
    if OCCUPIED_SLOTS.load(Ordering::Acquire) == 0 {
        return false;
    }
    (lookup_tid_flags(getpid(), gettid()) & BIT_PENDING) != 0
}

fn is_tid_alive(my_pid: i32, slot_p: i32, slot_t: i32) -> bool {
    if slot_p == my_pid {
        // `tgkill(pid, tid, 0)` probes thread liveness in our thread group.
        // SAFETY: signal 0 performs only existence validation.
        let rc = unsafe {
            syscall6(
                Sysno::tgkill as usize,
                my_pid.cast_unsigned() as usize,
                slot_t.cast_unsigned() as usize,
                0,
                0,
                0,
                0,
            )
        };
        rc != neg_errno(ESRCH)
    } else {
        // First detect an unreaped zombie child without reaping it (`WNOWAIT`),
        // since `kill(pid, 0)` still returns `0` for zombies.
        // SAFETY: zero-initialized `siginfo` is valid writable storage for `waitid`.
        let mut info: siginfo = unsafe { core::mem::zeroed() };
        // SAFETY: `waitid(P_PID, slot_p, &info, WEXITED | WNOHANG | WNOWAIT)`
        // never blocks and never consumes the child's wait status.
        let wrc = unsafe {
            syscall6(
                Sysno::waitid as usize,
                P_PID as usize,
                slot_p.cast_unsigned() as usize,
                (&raw mut info).addr(),
                (WEXITED | WNOHANG | WNOWAIT) as usize,
                0,
                0,
            )
        };
        if wrc == 0 {
            // SAFETY: `waitid(WEXITED)` populates `_sigchld._pid` when a child has exited.
            let exited_pid =
                unsafe { info.__bindgen_anon_1.__bindgen_anon_1._sifields._sigchld._pid };
            if exited_pid == slot_p {
                return false;
            }
        }
        // `kill(pid, 0)` probes whether a `vfork`/other process still exists.
        // SAFETY: signal 0 performs only existence validation.
        let rc = unsafe {
            syscall6(Sysno::kill as usize, slot_p.cast_unsigned() as usize, 0, 0, 0, 0, 0)
        };
        rc != neg_errno(ESRCH)
    }
}

fn reap_dead_tids(my_pid: i32) {
    for slot in &TID_SLOTS {
        let val = slot.load(Ordering::Acquire);
        if val == 0 {
            continue;
        }
        let p = slot_pid(val);
        let t = slot_tid(val);
        if !is_tid_alive(my_pid, p, t)
            && slot.compare_exchange(val, 0, Ordering::AcqRel, Ordering::Relaxed).is_ok()
        {
            OCCUPIED_SLOTS.fetch_sub(1, Ordering::Release);
            let _ = take_pending_info(val & KEY_MASK);
        }
    }
}

/// Atomically sets `set_bits` and clears `clear_bits` on the `(pid, tid)`
/// slot, returning the previous flags (`BIT_BLOCKED | BIT_PENDING`).
fn update_tid_flags(key: u64, set_bits: u64, clear_bits: u64) -> Result<u64> {
    // 1. Update an existing entry for `key` if present.
    if OCCUPIED_SLOTS.load(Ordering::Acquire) != 0 {
        for slot in &TID_SLOTS {
            loop {
                let val = slot.load(Ordering::Acquire);
                if val == 0 || (val & KEY_MASK) != key {
                    break;
                }
                let old_flags = val & (BIT_BLOCKED | BIT_PENDING);
                let new_flags = (old_flags | set_bits) & !clear_bits;
                if new_flags == old_flags {
                    return Ok(old_flags);
                }
                let new_val = if new_flags == 0 { 0 } else { key | new_flags };
                if slot
                    .compare_exchange_weak(val, new_val, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    if new_val == 0 {
                        OCCUPIED_SLOTS.fetch_sub(1, Ordering::Release);
                    }
                    return Ok(old_flags);
                }
            }
        }
    }

    let new_flags = set_bits & !clear_bits;
    if new_flags == 0 {
        return Ok(0);
    }

    // 2. Insert a new slot; if the table is under pressure or full, reap dead
    //    TIDs (e.g. exited pthreads that blocked signals on teardown).
    if OCCUPIED_SLOTS.load(Ordering::Acquire) >= MAX_TID_SLOTS / 2 {
        reap_dead_tids(slot_pid(key));
    }

    for attempt in 0..2 {
        for slot in &TID_SLOTS {
            if slot.load(Ordering::Relaxed) == 0
                && slot
                    .compare_exchange(0, key | new_flags, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            {
                OCCUPIED_SLOTS.fetch_add(1, Ordering::Release);
                return Ok(0);
            }
        }
        if attempt == 0 {
            reap_dead_tids(slot_pid(key));
        }
    }

    Err(Error::NOMEM)
}

fn set_tid_blocked(pid: i32, tid: i32, blocked: bool) -> Result<()> {
    let key = make_key(pid, tid).ok_or(Error::INVAL)?;
    if blocked {
        update_tid_flags(key, BIT_BLOCKED, 0)?;
    } else {
        update_tid_flags(key, 0, BIT_BLOCKED)?;
    }
    Ok(())
}

const fn synthetic_sigsys_info() -> siginfo {
    // SAFETY: zero-initialized `siginfo` is a valid C struct; populate `si_signo`.
    let mut info: siginfo = unsafe { core::mem::zeroed() };
    info.__bindgen_anon_1.__bindgen_anon_1.si_signo = SIGSYS_I32;
    info.__bindgen_anon_1.__bindgen_anon_1.si_code = -6; // SI_TKILL
    info
}

fn store_pending_info(key: u64, info: siginfo) {
    // Pass 0: update an existing slot for `key`.
    // Pass 1: claim an empty slot.
    // Pass 2: reclaim any slot (including one abandoned with `INFO_LOCK_BIT`)
    // whose owner thread is provably dead via `tgkill`/`kill` (`-ESRCH`).
    let my_pid = slot_pid(key);
    for pass in 0..3 {
        for slot in &PENDING_INFOS {
            let cur = slot.key.load(Ordering::Acquire);
            let can_claim = match pass {
                0 => cur == key,
                1 => cur == 0,
                _ => {
                    let owner = cur & KEY_MASK;
                    owner != 0
                        && owner != key
                        && !is_tid_alive(my_pid, slot_pid(owner), slot_tid(owner))
                }
            };
            if can_claim
                && slot
                    .key
                    .compare_exchange(cur, key | INFO_LOCK_BIT, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            {
                // SAFETY: `INFO_LOCK_BIT` grants exclusive write access to `slot.info`.
                unsafe { (*slot.info.0.get()).write(info) };
                slot.key.store(key, Ordering::Release);
                return;
            }
        }
    }
}

fn take_pending_info(key: u64) -> Option<siginfo> {
    // Lock with the calling (live) thread's own key so `store_pending_info`
    // pass 2 never mistakes a live `reap_dead_tids` take for an abandoned dead slot.
    let lock_val = make_key(getpid(), gettid()).unwrap_or(key) | INFO_LOCK_BIT;
    for slot in &PENDING_INFOS {
        let mut spins = 0_usize;
        loop {
            let cur = slot.key.load(Ordering::Acquire);
            if cur == (key | INFO_LOCK_BIT) {
                spins += 1;
                if spins >= MAX_INFO_LOCK_SPINS {
                    // Never clear a live thread's lock bit (which would race
                    // an in-progress write); fall back to `synthetic_sigsys_info`.
                    break;
                }
                core::hint::spin_loop();
                continue;
            }
            if cur != key {
                break;
            }
            if slot
                .key
                .compare_exchange_weak(key, lock_val, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // SAFETY: `slot.info` was initialized before publishing `key`.
                let info = unsafe { (*slot.info.0.get()).assume_init_read() };
                slot.key.store(0, Ordering::Release);
                return Some(info);
            }
        }
    }
    None
}

fn record_pending_sigsys(pid: i32, tid: i32, info: *const siginfo) {
    let Some(key) = make_key(pid, tid) else {
        return;
    };
    let saved = if info.is_null() {
        synthetic_sigsys_info()
    } else {
        // SAFETY: Kernel guarantees `info` is readable in `SA_SIGINFO` handler.
        unsafe { *info }
    };
    store_pending_info(key, saved);
    let _ = update_tid_flags(key, BIT_PENDING, 0);
}

fn take_pending_sigsys(pid: i32, tid: i32) -> Option<siginfo> {
    if OCCUPIED_SLOTS.load(Ordering::Acquire) == 0 {
        return None;
    }
    let key = make_key(pid, tid)?;
    let prev = update_tid_flags(key, 0, BIT_PENDING).ok()?;
    if (prev & BIT_PENDING) == 0 {
        return None;
    }
    Some(take_pending_info(key).unwrap_or_else(synthetic_sigsys_info))
}

fn discard_pending_sigsys(pid: i32, tid: i32) {
    let _ = take_pending_sigsys(pid, tid);
}

/// Handles intercepted `rt_sigaction(signum, act, oldact, sigsetsize)`.
///
/// # Safety
///
/// `inv` must contain the arguments from a trapped `rt_sigaction` syscall.
pub unsafe fn handle_rt_sigaction(inv: SyscallInvocation) -> isize {
    let signum = arg_i32(inv.args[0]);
    let act_ptr = inv.args[1];
    let oldact_ptr = inv.args[2];
    let sigsetsize = inv.args[3];

    if sigsetsize != KERNEL_SIGSET_SIZE {
        return neg_errno(EINVAL);
    }

    let shared_vm = is_shared_vm_child();

    if signum == SIGSYS_I32 {
        let new_act = if act_ptr != 0 {
            // SAFETY: `KernelSigaction` is a plain `#[repr(C)]` struct of integers.
            let Ok(mut act) = (unsafe { usermem::read_user::<KernelSigaction>(act_ptr) }) else {
                return neg_errno(EFAULT);
            };
            act.sa_mask &= !UNBLOCKABLE_MASK;
            Some(act)
        } else {
            None
        };

        // Read `prev` first and validate/write `oldact` before mutating the
        // virtualized action so an unwritable `oldact` returns `-EFAULT`
        // without changing signal state.
        let prev = with_action_lock(|slot| *slot);
        // SAFETY: user pointer from the trapped syscall (see `usermem`).
        if oldact_ptr != 0 && unsafe { usermem::write_user(oldact_ptr, &prev) }.is_err() {
            return neg_errno(EFAULT);
        }

        if let Some(act) = new_act
            && !shared_vm
        {
            with_action_lock(|slot| {
                *slot = act;
            });
            if act.sa_handler == SIG_IGN_RAW {
                discard_pending_sigsys(getpid(), gettid());
            }
        }
        return 0;
    }

    // Other signals: strip SIGSYS from `act.sa_mask` before calling the kernel,
    // and restore the virtualized SIGSYS bit in `oldact.sa_mask` on read-back.
    let sig_bit =
        if (1..=64).contains(&signum) { Some(1_u64 << (signum - 1).cast_unsigned()) } else { None };

    let (filtered_act, new_had_sigsys) = if act_ptr != 0 {
        // SAFETY: `KernelSigaction` is a plain `#[repr(C)]` struct of integers.
        let Ok(mut act) = (unsafe { usermem::read_user::<KernelSigaction>(act_ptr) }) else {
            return neg_errno(EFAULT);
        };
        let had = (act.sa_mask & SIGSYS_BIT) != 0;
        act.sa_mask &= !SIGSYS_BIT;
        (Some(act), Some(had))
    } else {
        (None, None)
    };

    let prev_had_sigsys =
        sig_bit.is_some_and(|bit| (SA_MASK_HAS_SIGSYS.load(Ordering::Acquire) & bit) != 0);

    let kernel_act_ptr = filtered_act.as_ref().map_or(0, |a| ptr::from_ref(a).addr());
    // SAFETY: `kernel_act_ptr` is either null or points to `filtered_act`.
    let rc = unsafe {
        syscall6(
            Sysno::rt_sigaction as usize,
            inv.args[0],
            kernel_act_ptr,
            oldact_ptr,
            sigsetsize,
            0,
            0,
        )
    };

    if rc == 0 {
        if !shared_vm && let (Some(bit), Some(had)) = (sig_bit, new_had_sigsys) {
            if had {
                SA_MASK_HAS_SIGSYS.fetch_or(bit, Ordering::Release);
            } else {
                SA_MASK_HAS_SIGSYS.fetch_and(!bit, Ordering::Release);
            }
        }
        if oldact_ptr != 0 && prev_had_sigsys {
            // SAFETY: `KernelSigaction` is a plain `#[repr(C)]` struct of integers.
            if let Ok(mut oldact) = unsafe { usermem::read_user::<KernelSigaction>(oldact_ptr) } {
                oldact.sa_mask |= SIGSYS_BIT;
                // SAFETY: the kernel has just written `oldact_ptr`.
                let _ = unsafe { usermem::write_user(oldact_ptr, &oldact) };
            }
        }
    }

    rc
}

/// Handles intercepted `rt_sigprocmask(how, set, oldset, sigsetsize)`.
///
/// Reads and mutates `ucontext.uc_sigmask` directly because `rt_sigreturn`
/// unconditionally installs `ucontext.uc_sigmask` as the thread's signal mask
/// when our `SIGSYS` handler returns.
///
/// # Safety
///
/// `inv` must contain the arguments from a trapped `rt_sigprocmask` syscall,
/// and `ucontext` must be the live writable `Ucontext` for that signal frame.
#[expect(clippy::cast_possible_truncation, reason = "Linux sigprocmask `how` is u32")]
pub unsafe fn handle_rt_sigprocmask(
    inv: SyscallInvocation,
    ucontext: &mut Ucontext,
) -> SigprocmaskOutcome {
    let how = inv.args[0] as u32;
    let set_ptr = inv.args[1];
    let oldset_ptr = inv.args[2];
    let sigsetsize = inv.args[3];

    if sigsetsize != KERNEL_SIGSET_SIZE {
        return SigprocmaskOutcome::Done(neg_errno(EINVAL));
    }

    // Invalid `how` with non-null `set`: replay directly to the kernel so
    // `copy_from_user(set)` (`-EFAULT`) runs BEFORE the `how` check (`-EINVAL`),
    // matching Linux kernel semantics relied upon by Abseil `AddressIsReadable`
    // (`syscall(SYS_rt_sigprocmask, ~0, addr, NULL, 8)`).
    if set_ptr != 0 && how != SIG_BLOCK && how != SIG_UNBLOCK && how != SIG_SETMASK {
        // SAFETY: Kernel never modifies the signal mask when `how` is invalid.
        return SigprocmaskOutcome::Done(unsafe { replay(inv) });
    }

    let pid = getpid();
    let tid = gettid();
    let cur_kernel_mask = *uc_sigmask_mut(ucontext) & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
    let was_blocked = (lookup_tid_flags(pid, tid) & BIT_BLOCKED) != 0;

    // Read `set` before writing `oldset` so `set == oldset` aliasing works and
    // an unreadable `set` returns `-EFAULT` before touching `oldset`.
    let raw_mask = if set_ptr != 0 {
        // SAFETY: `u64` accepts any bit pattern.
        let Ok(mask) = (unsafe { usermem::read_user::<u64>(set_ptr) }) else {
            return SigprocmaskOutcome::Done(neg_errno(EFAULT));
        };
        Some(mask)
    } else {
        None
    };

    if oldset_ptr != 0 {
        let old_virtual =
            if was_blocked { cur_kernel_mask | SIGSYS_BIT } else { cur_kernel_mask & !SIGSYS_BIT };
        // SAFETY: user pointer from the trapped syscall (see `usermem`).
        if unsafe { usermem::write_user(oldset_ptr, &old_virtual) }.is_err() {
            return SigprocmaskOutcome::Done(neg_errno(EFAULT));
        }
    }

    if let Some(raw_mask) = raw_mask {
        let filtered_mask = raw_mask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
        let has_sigsys = (raw_mask & SIGSYS_BIT) != 0;

        let new_kernel_mask = match how {
            SIG_BLOCK => cur_kernel_mask | filtered_mask,
            SIG_UNBLOCK => cur_kernel_mask & !filtered_mask,
            SIG_SETMASK => filtered_mask,
            _ => unreachable!(),
        };
        let now_blocked = match how {
            SIG_BLOCK => was_blocked || has_sigsys,
            SIG_UNBLOCK => was_blocked && !has_sigsys,
            SIG_SETMASK => has_sigsys,
            _ => unreachable!(),
        };

        if now_blocked != was_blocked && set_tid_blocked(pid, tid, now_blocked).is_err() {
            return SigprocmaskOutcome::Done(neg_errno(ENOMEM));
        }
        *uc_sigmask_mut(ucontext) = new_kernel_mask;

        if was_blocked && !now_blocked && (lookup_tid_flags(pid, tid) & BIT_PENDING) != 0 {
            return SigprocmaskOutcome::DeliverPendingSigsys(0);
        }
    }

    SigprocmaskOutcome::Done(0)
}

/// Takes and delivers any deferred pending `SIGSYS` for the calling thread on
/// the entry stack.
///
/// # Safety
///
/// Must be called on the entry stack with a valid writable `ucontext`.
#[inline(never)]
pub unsafe fn deliver_pending_sigsys(ucontext: &mut Ucontext) {
    let pid = getpid();
    let tid = gettid();
    if let Some(mut info) = take_pending_sigsys(pid, tid) {
        // SAFETY: `info` is valid stack storage and `ucontext` is writable.
        unsafe { deliver_sigsys_now(SIGSYS_I32, &raw mut info, ucontext) };
    }
}

/// Handles intercepted `rt_sigpending(set, sigsetsize)`, merging any virtually
/// pending `SIGSYS` into the returned set.
///
/// # Safety
///
/// `inv` must contain the arguments from a trapped `rt_sigpending` syscall.
pub unsafe fn handle_rt_sigpending(inv: SyscallInvocation) -> isize {
    // SAFETY: Replays `rt_sigpending` directly to the kernel.
    let rc = unsafe { replay(inv) };
    if rc == 0 && inv.args[0] != 0 && inv.args[1] == KERNEL_SIGSET_SIZE && is_current_tid_pending()
    {
        // SAFETY: `u64` accepts any bit pattern.
        if let Ok(mask) = unsafe { usermem::read_user::<u64>(inv.args[0]) } {
            let updated = mask | SIGSYS_BIT;
            // SAFETY: the kernel has just written `inv.args[0]`.
            if unsafe { usermem::write_user(inv.args[0], &updated) }.is_err() {
                return neg_errno(EFAULT);
            }
        }
    }
    rc
}

/// Handles masked-wait syscalls where `args[mask_idx]` is a `*const u64`
/// sigset pointer and `args[size_idx]` is `sigsetsize`.
///
/// Covers `rt_sigsuspend` (0, 1, `allow_null_mask = false`), `ppoll` (3, 4),
/// `epoll_pwait` (4, 5), and `epoll_pwait2` (4, 5).
///
/// Must execute on the entry stack (not on `stack_pool`) so any `SA_ONSTACK`
/// signal delivered while waiting does not re-enter and clobber the active
/// `SIGSYS` frame on `sigaltstack`.
///
/// # Safety
///
/// `inv` must contain the arguments from the corresponding trapped syscall, and
/// `ucontext` must be the live writable signal frame for that trap.
pub unsafe fn handle_masked_wait(
    mut inv: SyscallInvocation,
    ucontext: &mut Ucontext,
    mask_idx: usize,
    size_idx: usize,
    allow_null_mask: bool,
) -> isize {
    let mask_ptr = inv.args[mask_idx];
    let sigsetsize = inv.args[size_idx];

    if mask_ptr == 0 {
        if !allow_null_mask {
            // `rt_sigsuspend(NULL, ...)` fails immediately with `-EFAULT`.
            // SAFETY: Replays the invalid invocation directly.
            return unsafe { replay(inv) };
        }
        let orig_mask = ucontext.uc_sigmask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
        inv.args[mask_idx] = (&raw const orig_mask).addr();
        inv.args[size_idx] = KERNEL_SIGSET_SIZE;
        // SAFETY: `orig_mask` remains valid on the entry stack for the syscall.
        return unsafe { replay_masked_wait(&inv, ucontext) };
    }

    if sigsetsize != KERNEL_SIGSET_SIZE {
        // SAFETY: Replay invalid sigsetsize to preserve exact `-EINVAL`.
        return unsafe { replay(inv) };
    }

    // SAFETY: `u64` accepts any bit pattern.
    let Ok(raw_mask) = (unsafe { usermem::read_user::<u64>(mask_ptr) }) else {
        return neg_errno(EFAULT);
    };

    let filtered = raw_mask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
    inv.args[mask_idx] = (&raw const filtered).addr();
    // SAFETY: `filtered` lives on the entry stack for the wait duration.
    unsafe { run_wait_with_virtual_sigsys(inv, ucontext, (raw_mask & SIGSYS_BIT) != 0) }
}

/// Handles `pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask_struct)`.
///
/// Must execute on the entry stack (not on `stack_pool`).
///
/// # Safety
///
/// `inv` must contain the arguments from a trapped `pselect6` syscall, and
/// `ucontext` must be the live writable signal frame for that trap.
pub unsafe fn handle_pselect6(mut inv: SyscallInvocation, ucontext: &mut Ucontext) -> isize {
    let orig_mask = ucontext.uc_sigmask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
    let sig_ptr = inv.args[5];
    if sig_ptr == 0 {
        let local_wrapper = PselectSigmask { ss: &raw const orig_mask, ss_len: KERNEL_SIGSET_SIZE };
        inv.args[5] = (&raw const local_wrapper).addr();
        // SAFETY: `local_wrapper` and `orig_mask` live on the entry stack.
        return unsafe { replay_masked_wait(&inv, ucontext) };
    }

    // SAFETY: `PselectSigmask` is a plain `#[repr(C)]` struct of pointer + usize.
    let Ok(wrapper) = (unsafe { usermem::read_user::<PselectSigmask>(sig_ptr) }) else {
        return neg_errno(EFAULT);
    };
    if wrapper.ss.is_null() {
        let local_wrapper = PselectSigmask { ss: &raw const orig_mask, ss_len: KERNEL_SIGSET_SIZE };
        inv.args[5] = (&raw const local_wrapper).addr();
        // SAFETY: `local_wrapper` and `orig_mask` live on the entry stack.
        return unsafe { replay_masked_wait(&inv, ucontext) };
    }
    if wrapper.ss_len != KERNEL_SIGSET_SIZE {
        // SAFETY: Invalid sigsetsize; replay directly to return `-EINVAL`.
        return unsafe { replay(inv) };
    }

    // SAFETY: `u64` accepts any bit pattern.
    let Ok(raw_mask) = (unsafe { usermem::read_user::<u64>(wrapper.ss.addr()) }) else {
        return neg_errno(EFAULT);
    };
    let filtered = raw_mask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
    let local_wrapper = PselectSigmask { ss: &raw const filtered, ss_len: KERNEL_SIGSET_SIZE };
    inv.args[5] = (&raw const local_wrapper).addr();
    // SAFETY: `local_wrapper` and `filtered` live on the entry stack for the wait.
    unsafe { run_wait_with_virtual_sigsys(inv, ucontext, (raw_mask & SIGSYS_BIT) != 0) }
}

#[inline]
fn monotonic_ns() -> Option<i128> {
    let mut ts = timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is valid stack storage for `clock_gettime(CLOCK_MONOTONIC)`.
    let rc = unsafe {
        syscall6(
            Sysno::clock_gettime as usize,
            CLOCK_MONOTONIC as usize,
            (&raw mut ts).addr(),
            0,
            0,
            0,
            0,
        )
    };
    if rc == 0 {
        Some(i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec))
    } else {
        None
    }
}

/// Clears `pselect6`'s non-null `readfds`/`writefds`/`exceptfds` bitsets when
/// a zero-timeout retry loop is exhausted under a continuous `SIGSYS` flood.
fn clear_pselect_fdsets(inv: &SyscallInvocation) {
    // Cap at `FD_SETSIZE` (1024 bits = 128 bytes) so callers passing a large
    // `nfds` (e.g. `getdtablesize()`) with a standard `fd_set` never get memory
    // beyond their `fd_set` zeroed if zero-timeout retries are exhausted.
    let nfds = arg_i32(inv.args[0]).min(1024);
    if nfds <= 0 {
        return;
    }
    let words = nfds.cast_unsigned().div_ceil(64) as usize;
    let total_bytes = words * core::mem::size_of::<u64>();
    let zeros = [0_u8; 128];
    for fdset_ptr in [inv.args[1], inv.args[2], inv.args[3]] {
        if fdset_ptr == 0 {
            continue;
        }
        // SAFETY: user pointer from the trapped syscall (see `usermem`).
        let _ = unsafe { usermem::write_user_bytes(fdset_ptr, &zeros[..total_bytes]) };
    }
}

/// Replays a masked-wait syscall while `SIGSYS` is virtually blocked, retrying
/// if the wait returned `-EINTR` solely because a virtually-blocked non-seccomp
/// `SIGSYS` woke the kernel wait.
#[expect(clippy::cast_possible_truncation, reason = "Linux timeout fields fit in i32/i64")]
unsafe fn replay_masked_wait_deferring_sigsys(
    pid: i32,
    tid: i32,
    mut inv: SyscallInvocation,
    ucontext: &mut Ucontext,
) -> isize {
    const MAX_ZERO_POLL_RETRIES: u8 = 8;
    let sysno = Sysno::new(inv.nr);
    // Snapshot initial relative timeout (if any) before first wait so retries
    // after a deferred `SIGSYS` preserve the caller's deadline for `ppoll`,
    // `pselect6`, `epoll_pwait`, and `epoll_pwait2`.
    let mut local_ts = timespec { tv_sec: 0, tv_nsec: 0 };
    let mut ts_arg_idx: Option<usize> = None;
    let mut orig_ts_ptr: usize = 0;
    let initial_timeout_ns: Option<i128> = match sysno {
        Some(Sysno::ppoll | Sysno::pselect6 | Sysno::epoll_pwait2) => {
            let idx = match sysno {
                Some(Sysno::ppoll) => 2,
                Some(Sysno::pselect6) => 4,
                _ => 3,
            };
            let ts_ptr = inv.args[idx];
            if ts_ptr == 0 {
                None
            } else {
                // SAFETY: `timespec` is a `#[repr(C)]` struct of `i64` fields.
                let Ok(ts) = (unsafe { usermem::read_user::<timespec>(ts_ptr) }) else {
                    // SAFETY: Replays invalid timeout pointer directly (`-EFAULT`).
                    return unsafe { replay_masked_wait(&inv, ucontext) };
                };
                if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
                    // SAFETY: Replays invalid timeout directly (`-EINVAL`).
                    return unsafe { replay_masked_wait(&inv, ucontext) };
                }
                local_ts = ts;
                ts_arg_idx = Some(idx);
                orig_ts_ptr = ts_ptr;
                Some(i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec))
            }
        }
        Some(Sysno::epoll_pwait) => {
            let ms = arg_i32(inv.args[3]);
            if ms < 0 { None } else { Some(i128::from(ms) * 1_000_000) }
        }
        // `rt_sigsuspend` has no timeout.
        _ => None,
    };

    let start_ns = initial_timeout_ns.and_then(|_| monotonic_ns());
    let mut retried = false;
    let mut zero_poll_retries = 0_u8;

    loop {
        if retried && let Some(idx) = ts_arg_idx {
            inv.args[idx] = (&raw const local_ts).addr();
        }
        let seq_before = read_tid_deferred_seq(pid, tid);
        // SAFETY: `inv` points to valid entry-stack wait args (including `local_ts`).
        let rc = unsafe { replay_masked_wait(&inv, ucontext) };
        if rc != neg_errno(EINTR) || read_tid_deferred_seq(pid, tid) == seq_before {
            if retried && matches!(sysno, Some(Sysno::pselect6 | Sysno::ppoll)) && orig_ts_ptr != 0
            {
                // SAFETY: user pointer from the trapped syscall (see `usermem`).
                let _ = unsafe { usermem::write_user(orig_ts_ptr, &local_ts) };
            }
            return rc;
        }
        retried = true;
        // A virtually-blocked `SIGSYS` interrupted this thread's kernel wait;
        // retry with the remaining timeout (or a bounded number of zero-timeout
        // kernel polls if expired, falling back to clearing `pselect6` fd sets
        // directly so a tight `SIGSYS` flood can neither livelock nor leave
        // stale fd-set bits behind).
        if let (Some(total_ns), Some(t0)) = (initial_timeout_ns, start_ns) {
            let elapsed = monotonic_ns().map_or(total_ns, |now| now.saturating_sub(t0));
            let rem = total_ns - elapsed;
            if rem <= 0 {
                if zero_poll_retries >= MAX_ZERO_POLL_RETRIES {
                    if sysno == Some(Sysno::pselect6) {
                        clear_pselect_fdsets(&inv);
                    }
                    if matches!(sysno, Some(Sysno::pselect6 | Sysno::ppoll)) && orig_ts_ptr != 0 {
                        local_ts = timespec { tv_sec: 0, tv_nsec: 0 };
                        // SAFETY: user pointer from the trapped syscall (see `usermem`).
                        let _ = unsafe { usermem::write_user(orig_ts_ptr, &local_ts) };
                    }
                    return 0;
                }
                zero_poll_retries += 1;
                match sysno {
                    Some(Sysno::ppoll | Sysno::pselect6 | Sysno::epoll_pwait2) => {
                        local_ts = timespec { tv_sec: 0, tv_nsec: 0 };
                    }
                    Some(Sysno::epoll_pwait) => {
                        inv.args[3] = 0;
                    }
                    _ => {}
                }
                continue;
            }
            match sysno {
                Some(Sysno::ppoll | Sysno::pselect6 | Sysno::epoll_pwait2) => {
                    local_ts = timespec {
                        tv_sec: (rem / 1_000_000_000) as i64,
                        tv_nsec: (rem % 1_000_000_000) as i64,
                    };
                }
                Some(Sysno::epoll_pwait) => {
                    let rem_ms =
                        ((rem + 999_999) / 1_000_000).clamp(1, i128::from(i32::MAX)) as i32;
                    inv.args[3] = rem_ms.cast_unsigned() as usize;
                }
                _ => {}
            }
        }
    }
}

/// Runs a masked-wait syscall while honoring the caller's temporary `SIGSYS`
/// blocked/unblocked bit in the wait mask.
unsafe fn run_wait_with_virtual_sigsys(
    inv: SyscallInvocation,
    ucontext: &mut Ucontext,
    wait_blocks_sigsys: bool,
) -> isize {
    let pid = getpid();
    let tid = gettid();
    let was_blocked = (lookup_tid_flags(pid, tid) & BIT_BLOCKED) != 0;

    if !wait_blocks_sigsys && was_blocked {
        // Wait mask temporarily unblocks `SIGSYS`. If one is already pending,
        // deliver it immediately and return `-EINTR` (matching Linux `sigsuspend`).
        if let Some(mut info) = take_pending_sigsys(pid, tid) {
            // SAFETY: Delivering pending virtual `SIGSYS` on entry stack.
            unsafe { deliver_sigsys_now(SIGSYS_I32, &raw mut info, ucontext) };
            return neg_errno(EINTR);
        }
        let _ = set_tid_blocked(pid, tid, false);
        // SAFETY: `inv` points to valid entry-stack wait args.
        let rc = unsafe { replay_masked_wait(&inv, ucontext) };
        let _ = set_tid_blocked(pid, tid, true);
        return rc;
    }

    if wait_blocks_sigsys && !was_blocked {
        // Wait mask temporarily blocks `SIGSYS`; defer any non-seccomp `SIGSYS`
        // that arrives during the wait until the wait completes.
        let _ = set_tid_blocked(pid, tid, true);
        // SAFETY: `inv` points to valid entry-stack wait args.
        let rc = unsafe { replay_masked_wait_deferring_sigsys(pid, tid, inv, ucontext) };
        let _ = set_tid_blocked(pid, tid, false);
        if let Some(mut info) = take_pending_sigsys(pid, tid) {
            // SAFETY: Delivering `SIGSYS` that arrived while masked by wait.
            unsafe { deliver_sigsys_now(SIGSYS_I32, &raw mut info, ucontext) };
        }
        return rc;
    }

    if wait_blocks_sigsys {
        // SAFETY: `SIGSYS` is already blocked; retry if a deferred `SIGSYS` wakes the wait.
        return unsafe { replay_masked_wait_deferring_sigsys(pid, tid, inv, ucontext) };
    }

    // SAFETY: `inv` points to valid entry-stack wait args.
    unsafe { replay_masked_wait(&inv, ucontext) }
}

/// Handles a non-seccomp or foreign-seccomp `SIGSYS` delivery, deferring
/// non-seccomp `SIGSYS` when virtually blocked on this thread and otherwise
/// forwarding it to the target's virtualized `SIGSYS` disposition.
///
/// # Safety
///
/// Must be called on the entry stack from the `SIGSYS` signal handler with the
/// kernel-supplied `info` and `ucontext` pointers.
pub unsafe fn forward_non_seccomp_sigsys(signum: i32, info: *mut siginfo, ucontext: *mut Ucontext) {
    let is_foreign_seccomp = !info.is_null() && {
        // SAFETY: Kernel guarantees `info` is valid in `SA_SIGINFO` handler.
        let common = unsafe { &(*info).__bindgen_anon_1.__bindgen_anon_1 };
        common.si_code == SYS_SECCOMP.cast_signed()
    };

    if !is_foreign_seccomp && is_current_tid_blocked() {
        let pid = getpid();
        let tid = gettid();
        let handler = with_action_lock(|slot| slot.sa_handler);
        if handler != SIG_IGN_RAW {
            record_pending_sigsys(pid, tid, info);
        }
        // Only mark this thread's masked wait as woken by a deferred `SIGSYS`
        // when `SIGSYS` directly interrupted `replay_masked_wait`'s syscall.
        // If another signal (e.g. `SIGINT`/`SIGUSR1`) was dequeued first on
        // the same wake or was already executing its handler, `ucontext_pc`
        // points to that user handler rather than `masked_wait_return_pc()`,
        // so the wait preserves `-EINTR` instead of swallowing it.
        if !ucontext.is_null()
            // SAFETY: Kernel guarantees `ucontext` is readable in `SA_SIGINFO` handler.
            && ucontext_pc(unsafe { &*ucontext }) == masked_wait_return_pc()
        {
            bump_tid_deferred_seq(pid, tid);
        }
        return;
    }

    // SAFETY: Caller passed valid kernel signal frame pointers.
    unsafe { deliver_sigsys_now(signum, info, ucontext) };
}

/// Delivers a `SIGSYS` immediately to the target's virtualized `SIGSYS`
/// disposition (used by both [`forward_non_seccomp_sigsys`] and deferred
/// delivery upon unblocking `SIGSYS`).
///
/// # Safety
///
/// Must be called on the entry stack with valid `info` and `ucontext` pointers.
pub unsafe fn deliver_sigsys_now(signum: i32, info: *mut siginfo, ucontext: *mut Ucontext) {
    let user_act = with_action_lock(|slot| {
        let act = *slot;
        if (act.sa_flags & SA_RESETHAND as usize) != 0 {
            slot.sa_handler = SIG_DFL_RAW;
        }
        act
    });

    match user_act.sa_handler {
        SIG_IGN_RAW => {}
        SIG_DFL_RAW => terminate_with_default_sigsys(),
        handler_addr => {
            // Our `SIGSYS` `sa_mask` blocked all maskable signals on entry;
            // install the exact mask the target handler expects (`orig_mask |
            // user_act.sa_mask`, with `SIGSYS` kept unblocked in the kernel).
            let orig_mask = if ucontext.is_null() {
                0
            } else {
                // SAFETY: Kernel-provided `ucontext` is readable.
                unsafe { (*ucontext).uc_sigmask }
            };
            let handler_mask = (orig_mask | user_act.sa_mask) & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
            // SAFETY: `handler_mask` is a valid stack-local `u64`.
            let _ =
                unsafe { rt_sigprocmask(SIG_SETMASK, &raw const handler_mask, ptr::null_mut()) };

            let defer_sigsys = (user_act.sa_flags & SA_NODEFER as usize) == 0;
            let pid = if defer_sigsys { getpid() } else { 0 };
            let tid = if defer_sigsys { gettid() } else { 0 };
            let prev_virtual_blocked =
                defer_sigsys && (lookup_tid_flags(pid, tid) & BIT_BLOCKED) != 0;
            if defer_sigsys && !prev_virtual_blocked {
                let _ = set_tid_blocked(pid, tid, true);
            }

            // SAFETY: `handler_addr` was registered by the target as its `SIGSYS`
            // handler; `invoke_user_sigsys_handler` restores `x28` on AArch64.
            unsafe {
                invoke_user_sigsys_handler(
                    handler_addr,
                    (user_act.sa_flags & SA_SIGINFO as usize) != 0,
                    signum,
                    info,
                    ucontext,
                );
            }

            if defer_sigsys && !prev_virtual_blocked {
                let _ = set_tid_blocked(pid, tid, false);
            }
            if ucontext.is_null() {
                let restore = orig_mask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
                // SAFETY: `restore` is a valid stack-local `u64`.
                let _ = unsafe { rt_sigprocmask(SIG_SETMASK, &raw const restore, ptr::null_mut()) };
            } else {
                // Ensure returning through `rt_sigreturn` cannot re-block SIGSYS
                // even if the user handler mutated `ucontext->uc_sigmask`.
                // `rt_sigreturn` also restores `ucontext->uc_sigmask` atomically.
                // SAFETY: Kernel-provided `ucontext` is writable.
                *uc_sigmask_mut(unsafe { &mut *ucontext }) &= !SIGSYS_BIT;
            }
        }
    }
}

fn terminate_with_default_sigsys() -> ! {
    let dfl = KernelSigaction { sa_handler: SIG_DFL_RAW, sa_flags: 0, sa_restorer: 0, sa_mask: 0 };
    // SAFETY: Restoring kernel default `SIGSYS` disposition before re-raising.
    let _ = unsafe { rt_sigaction(SIGSYS_I32, &raw const dfl, ptr::null_mut()) };
    let sigsys = SIGSYS_BIT;
    // SAFETY: Unblocking `SIGSYS` so `tgkill` terminates immediately.
    let _ = unsafe { rt_sigprocmask(SIG_UNBLOCK, &raw const sigsys, ptr::null_mut()) };
    let pid = getpid();
    let tid = gettid();
    // SAFETY: Sends `SIGSYS` with default disposition to the current thread.
    let _ = unsafe {
        syscall6(
            Sysno::tgkill as usize,
            pid.cast_unsigned() as usize,
            tid.cast_unsigned() as usize,
            SIGSYS as usize,
            0,
            0,
            0,
        )
    };
    loop {
        // SAFETY: Fallback termination if `SIGSYS` somehow did not terminate.
        let _ =
            unsafe { syscall6(Sysno::exit_group as usize, 128 + SIGSYS as usize, 0, 0, 0, 0, 0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_tid_table_tracks_insert_lookup_pending_and_remove() {
        clear_all_tid_slots();
        let pid = getpid();
        assert_eq!(lookup_tid_flags(pid, 42), 0);
        set_tid_blocked(pid, 42, true).unwrap();
        assert_eq!(lookup_tid_flags(pid, 42), BIT_BLOCKED);
        assert_eq!(lookup_tid_flags(pid, 99), 0);
        assert_eq!(lookup_tid_flags(pid + 1, 42), 0);

        // Duplicate insert is a no-op.
        set_tid_blocked(pid, 42, true).unwrap();
        assert_eq!(OCCUPIED_SLOTS.load(Ordering::Acquire), 1);

        // Pending bit coexists with blocked bit.
        record_pending_sigsys(pid, 42, ptr::null());
        assert_eq!(lookup_tid_flags(pid, 42), BIT_BLOCKED | BIT_PENDING);
        let taken = take_pending_sigsys(pid, 42);
        assert!(taken.is_some());
        assert_eq!(lookup_tid_flags(pid, 42), BIT_BLOCKED);

        set_tid_blocked(pid, 99, true).unwrap();
        assert_eq!(OCCUPIED_SLOTS.load(Ordering::Acquire), 2);

        set_tid_blocked(pid, 42, false).unwrap();
        assert_eq!(lookup_tid_flags(pid, 42), 0);
        assert_eq!(lookup_tid_flags(pid, 99), BIT_BLOCKED);
        assert_eq!(OCCUPIED_SLOTS.load(Ordering::Acquire), 1);

        set_tid_blocked(pid, 99, false).unwrap();
        assert_eq!(lookup_tid_flags(pid, 99), 0);
        assert_eq!(OCCUPIED_SLOTS.load(Ordering::Acquire), 0);
    }
}
