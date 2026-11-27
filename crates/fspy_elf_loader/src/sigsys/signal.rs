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
//!   ownership, resets the action seqlock, drops parent sibling TIDs, and
//!   inherits the calling thread's virtual `SIGSYS` blocked bit.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering, fence},
};

use fspy_nostd::{Error, Result};
use linux_raw_sys::{
    errno::{EFAULT, EINTR, EINVAL, ENOMEM, ESRCH},
    general::{
        CLOCK_MONOTONIC, SA_NODEFER, SA_RESETHAND, SA_SIGINFO, SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK,
        SIGSYS, SYS_SECCOMP, siginfo, timespec,
    },
};
use syscalls::Sysno;

use super::{
    arch::{
        HANDLER_BLOCK_MASK, KERNEL_SIGSET_SIZE, KernelSigaction, SIGSYS_BIT, SyscallInvocation,
        UNBLOCKABLE_MASK, Ucontext, arg_i32, getpid, gettid, invoke_user_sigsys_handler, neg_errno,
        replay, replay_masked_wait, rt_sigaction, rt_sigprocmask, set_syscall_result, syscall6,
        uc_sigmask_mut,
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

// SAFETY: Access is serialized by per-slot `INFO_LOCK_BIT` (for `PENDING_INFOS`).
unsafe impl<T> Sync for SyncCell<T> {}

/// Lock-free seqlock storing the process-wide virtualized `SIGSYS` action.
///
/// Readers (`CLONE_VM` children, `rt_sigaction` queries, `forward_non_seccomp_sigsys`,
/// and non-`SA_RESETHAND` deliveries) read completely lock-free without ever
/// taking a lock. Writers (`!is_shared_vm_child()`) serialize via `writer_lock`
/// while all maskable signals including `SIGSYS` are already blocked by our
/// `SIGSYS` handler `sa_mask`.
struct SeqAction {
    /// Even when quiescent, odd while a writer updates the primary fields.
    seq: AtomicUsize,
    writer_lock: AtomicBool,
    sa_handler: AtomicUsize,
    sa_flags: AtomicUsize,
    sa_restorer: AtomicUsize,
    sa_mask: AtomicU64,
    /// Last fully committed even snapshot, updated only while `seq` is even so
    /// `reset_after_fork` can roll back a torn odd `seq` in a forked child.
    backup_handler: AtomicUsize,
    backup_flags: AtomicUsize,
    backup_restorer: AtomicUsize,
    backup_mask: AtomicU64,
}

impl SeqAction {
    const fn new_dfl() -> Self {
        Self {
            seq: AtomicUsize::new(0),
            writer_lock: AtomicBool::new(false),
            sa_handler: AtomicUsize::new(SIG_DFL_RAW),
            sa_flags: AtomicUsize::new(0),
            sa_restorer: AtomicUsize::new(0),
            sa_mask: AtomicU64::new(0),
            backup_handler: AtomicUsize::new(SIG_DFL_RAW),
            backup_flags: AtomicUsize::new(0),
            backup_restorer: AtomicUsize::new(0),
            backup_mask: AtomicU64::new(0),
        }
    }

    fn init(&self, act: KernelSigaction) {
        self.writer_lock.store(false, Ordering::Relaxed);
        self.sa_handler.store(act.sa_handler, Ordering::Relaxed);
        self.sa_flags.store(act.sa_flags, Ordering::Relaxed);
        self.sa_restorer.store(act.sa_restorer, Ordering::Relaxed);
        self.sa_mask.store(act.sa_mask, Ordering::Relaxed);
        self.backup_handler.store(act.sa_handler, Ordering::Relaxed);
        self.backup_flags.store(act.sa_flags, Ordering::Relaxed);
        self.backup_restorer.store(act.sa_restorer, Ordering::Relaxed);
        self.backup_mask.store(act.sa_mask, Ordering::Relaxed);
        self.seq.store(0, Ordering::Release);
    }

    fn reset_after_fork(&self) {
        // Runs single-threaded in a fresh `fork()` child. If `fork()` captured
        // another thread mid-`write_with` (odd `seq`), `backup_*` still holds
        // the previous fully committed action; restore it instead of sealing
        // torn fields. If `seq` is even, primary is consistent and `backup_*`
        // may have been mid-sync, so refresh `backup_*` from primary.
        let seq = self.seq.load(Ordering::Relaxed);
        if (seq & 1) != 0 {
            self.sa_handler.store(self.backup_handler.load(Ordering::Relaxed), Ordering::Relaxed);
            self.sa_flags.store(self.backup_flags.load(Ordering::Relaxed), Ordering::Relaxed);
            self.sa_restorer.store(self.backup_restorer.load(Ordering::Relaxed), Ordering::Relaxed);
            self.sa_mask.store(self.backup_mask.load(Ordering::Relaxed), Ordering::Relaxed);
            self.seq.store(seq.wrapping_add(1), Ordering::Release);
        } else {
            self.backup_handler.store(self.sa_handler.load(Ordering::Relaxed), Ordering::Relaxed);
            self.backup_flags.store(self.sa_flags.load(Ordering::Relaxed), Ordering::Relaxed);
            self.backup_restorer.store(self.sa_restorer.load(Ordering::Relaxed), Ordering::Relaxed);
            self.backup_mask.store(self.sa_mask.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        self.writer_lock.store(false, Ordering::Release);
    }

    fn read(&self) -> KernelSigaction {
        loop {
            let seq1 = self.seq.load(Ordering::Acquire);
            if (seq1 & 1) != 0 {
                core::hint::spin_loop();
                continue;
            }
            let act = KernelSigaction {
                sa_handler: self.sa_handler.load(Ordering::Relaxed),
                sa_flags: self.sa_flags.load(Ordering::Relaxed),
                sa_restorer: self.sa_restorer.load(Ordering::Relaxed),
                sa_mask: self.sa_mask.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == seq1 {
                return act;
            }
            core::hint::spin_loop();
        }
    }

    fn write_with<R>(&self, f: impl FnOnce(KernelSigaction) -> (KernelSigaction, R)) -> R {
        let mut spins = 0_usize;
        while self
            .writer_lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spins += 1;
            if spins.trailing_zeros() >= 10 {
                // SAFETY: `sched_yield` takes no arguments and is async-signal-safe.
                let _ = unsafe { syscall6(Sysno::sched_yield as usize, 0, 0, 0, 0, 0, 0) };
            } else {
                core::hint::spin_loop();
            }
        }
        let cur = KernelSigaction {
            sa_handler: self.sa_handler.load(Ordering::Relaxed),
            sa_flags: self.sa_flags.load(Ordering::Relaxed),
            sa_restorer: self.sa_restorer.load(Ordering::Relaxed),
            sa_mask: self.sa_mask.load(Ordering::Relaxed),
        };
        let (next, result) = f(cur);
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.sa_handler.store(next.sa_handler, Ordering::Relaxed);
        self.sa_flags.store(next.sa_flags, Ordering::Relaxed);
        self.sa_restorer.store(next.sa_restorer, Ordering::Relaxed);
        self.sa_mask.store(next.sa_mask, Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
        self.backup_handler.store(next.sa_handler, Ordering::Relaxed);
        self.backup_flags.store(next.sa_flags, Ordering::Relaxed);
        self.backup_restorer.store(next.sa_restorer, Ordering::Relaxed);
        self.backup_mask.store(next.sa_mask, Ordering::Relaxed);
        self.writer_lock.store(false, Ordering::Release);
        result
    }
}

/// Owning PID of this address space (set in `init` and refreshed in `fork`
/// children). Any thread observing `getpid() != OWNER_PID` is executing inside
/// a `CLONE_VM` (`vfork` / `posix_spawn`) child sharing the parent's memory.
static OWNER_PID: AtomicI32 = AtomicI32::new(0);

static USER_SIGSYS_ACTION: SeqAction = SeqAction::new_dfl();

/// Returns `true` when the target's virtualized `SIGSYS` disposition is `SIG_IGN`.
pub fn is_sigsys_ignored() -> bool {
    USER_SIGSYS_ACTION.read().sa_handler == SIG_IGN_RAW
}

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
    USER_SIGSYS_ACTION.init(inherited_action);
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
/// address space), dropping parent sibling TIDs, resetting any action writer
/// state held by a sibling thread at `fork()` time, and inheriting the calling
/// thread's virtual `SIGSYS` blocked state.
pub fn on_fork_child(parent_blocked: bool) {
    let pid = getpid();
    OWNER_PID.store(pid, Ordering::Release);
    USER_SIGSYS_ACTION.reset_after_fork();
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
    // Pass 0: update an existing unlocked slot for `key`.
    // Pass 1: claim an empty slot.
    // Pass 2: reclaim an unlocked slot whose owner thread is dead (`-ESRCH`).
    let my_pid = slot_pid(key);
    for pass in 0..3 {
        for slot in &PENDING_INFOS {
            let cur = slot.key.load(Ordering::Acquire);
            let can_claim = match pass {
                0 => cur == key,
                1 => cur == 0,
                _ => {
                    // Reclaim any slot (including one abandoned with
                    // `INFO_LOCK_BIT` set by a killed `CLONE_VM` child) whose
                    // recorded `(pid, tid)` is confirmed dead (`-ESRCH`).
                    // Live lockers (`store_pending_info` / `take_pending_info`)
                    // always store their own live `(pid, tid)`, so this never
                    // races an active writer.
                    let owner_key = cur & !INFO_LOCK_BIT;
                    owner_key != 0
                        && owner_key != key
                        && !is_tid_alive(my_pid, slot_pid(owner_key), slot_tid(owner_key))
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
    let my_pid = getpid();
    let lock_val = make_key(my_pid, gettid()).unwrap_or(key) | INFO_LOCK_BIT;
    for slot in &PENDING_INFOS {
        let mut spins = 0_usize;
        loop {
            let cur = slot.key.load(Ordering::Acquire);
            if cur == (key | INFO_LOCK_BIT) {
                spins += 1;
                if spins >= MAX_INFO_LOCK_SPINS {
                    // If the locked slot's owner thread is confirmed dead
                    // (`-ESRCH`), release the leaked slot; never clear a live
                    // thread's lock bit.
                    if !is_tid_alive(my_pid, slot_pid(key), slot_tid(key)) {
                        let _ =
                            slot.key.compare_exchange(cur, 0, Ordering::AcqRel, Ordering::Relaxed);
                    }
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

/// Discards any pending virtual `SIGSYS` across every thread in process `pid`
/// (matching POSIX/Linux `rt_sigaction(SIGSYS, SIG_IGN)` semantics).
fn discard_all_pending_sigsys(pid: i32) {
    if OCCUPIED_SLOTS.load(Ordering::Acquire) == 0 {
        return;
    }
    for slot in &TID_SLOTS {
        loop {
            let val = slot.load(Ordering::Acquire);
            if val == 0 || slot_pid(val) != pid || (val & BIT_PENDING) == 0 {
                break;
            }
            let key = val & KEY_MASK;
            let new_flags = (val & (BIT_BLOCKED | BIT_PENDING)) & !BIT_PENDING;
            let new_val = if new_flags == 0 { 0 } else { key | new_flags };
            if slot.compare_exchange_weak(val, new_val, Ordering::AcqRel, Ordering::Relaxed).is_ok()
            {
                if new_val == 0 {
                    OCCUPIED_SLOTS.fetch_sub(1, Ordering::Release);
                }
                let _ = take_pending_info(key);
                break;
            }
        }
    }
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
        let prev = USER_SIGSYS_ACTION.read();
        // SAFETY: user pointer from the trapped syscall (see `usermem`).
        if oldact_ptr != 0 && unsafe { usermem::write_user(oldact_ptr, &prev) }.is_err() {
            return neg_errno(EFAULT);
        }

        if let Some(act) = new_act
            && !shared_vm
        {
            let actual_prev = USER_SIGSYS_ACTION.write_with(|cur| (act, cur));
            if oldact_ptr != 0 && actual_prev != prev {
                // SAFETY: `oldact_ptr` was validated writable above.
                let _ = unsafe { usermem::write_user(oldact_ptr, &actual_prev) };
            }
            if act.sa_handler == SIG_IGN_RAW {
                discard_all_pending_sigsys(getpid());
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

/// Completes a masked-wait syscall in `ucontext`, delivering any pending
/// `SIGSYS` AFTER writing the syscall return value so `SA_SIGINFO` handlers
/// observe the real return value and any edits they make to `ucontext` survive.
unsafe fn finish_masked_wait(ucontext: &mut Ucontext, rc: isize, pending: Option<siginfo>) {
    set_syscall_result(ucontext, rc);
    *uc_sigmask_mut(ucontext) &= !SIGSYS_BIT;
    if let Some(mut info) = pending {
        // SAFETY: `info` is valid stack storage and `ucontext` is writable.
        unsafe { deliver_sigsys_now(SIGSYS_I32, &raw mut info, ucontext) };
    }
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
) {
    let mask_ptr = inv.args[mask_idx];
    let sigsetsize = inv.args[size_idx];

    if mask_ptr == 0 {
        if !allow_null_mask {
            // `rt_sigsuspend(NULL, ...)` fails immediately with `-EFAULT`.
            // SAFETY: Replays the invalid invocation directly.
            let rc = unsafe { replay(inv) };
            // SAFETY: Completes `ucontext` with no pending delivery.
            unsafe { finish_masked_wait(ucontext, rc, None) };
            return;
        }
        let orig_mask = ucontext.uc_sigmask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
        inv.args[mask_idx] = (&raw const orig_mask).addr();
        inv.args[size_idx] = KERNEL_SIGSET_SIZE;
        // SAFETY: `orig_mask` remains valid on the entry stack for the syscall.
        let rc = unsafe { replay_masked_wait(&inv, ucontext) };
        // SAFETY: Completes `ucontext` with no pending delivery.
        unsafe { finish_masked_wait(ucontext, rc, None) };
        return;
    }

    if sigsetsize != KERNEL_SIGSET_SIZE {
        // SAFETY: Replay invalid sigsetsize to preserve exact `-EINVAL`.
        let rc = unsafe { replay(inv) };
        // SAFETY: Completes `ucontext` with no pending delivery.
        unsafe { finish_masked_wait(ucontext, rc, None) };
        return;
    }

    // SAFETY: `u64` accepts any bit pattern.
    let Ok(raw_mask) = (unsafe { usermem::read_user::<u64>(mask_ptr) }) else {
        // SAFETY: Completes `ucontext` with `-EFAULT`.
        unsafe { finish_masked_wait(ucontext, neg_errno(EFAULT), None) };
        return;
    };

    let filtered = raw_mask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
    inv.args[mask_idx] = (&raw const filtered).addr();
    // SAFETY: `filtered` lives on the entry stack for the wait duration.
    unsafe { run_wait_with_virtual_sigsys(inv, ucontext, (raw_mask & SIGSYS_BIT) != 0) };
}

/// Handles `pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask_struct)`.
///
/// Must execute on the entry stack (not on `stack_pool`).
///
/// # Safety
///
/// `inv` must contain the arguments from a trapped `pselect6` syscall, and
/// `ucontext` must be the live writable signal frame for that trap.
pub unsafe fn handle_pselect6(mut inv: SyscallInvocation, ucontext: &mut Ucontext) {
    let orig_mask = ucontext.uc_sigmask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
    let sig_ptr = inv.args[5];
    if sig_ptr == 0 {
        let local_wrapper = PselectSigmask { ss: &raw const orig_mask, ss_len: KERNEL_SIGSET_SIZE };
        inv.args[5] = (&raw const local_wrapper).addr();
        // SAFETY: `local_wrapper` and `orig_mask` live on the entry stack.
        let rc = unsafe { replay_masked_wait(&inv, ucontext) };
        // SAFETY: Completes `ucontext` with no pending delivery.
        unsafe { finish_masked_wait(ucontext, rc, None) };
        return;
    }

    // SAFETY: `PselectSigmask` is a plain `#[repr(C)]` struct of pointer + usize.
    let Ok(wrapper) = (unsafe { usermem::read_user::<PselectSigmask>(sig_ptr) }) else {
        // SAFETY: Completes `ucontext` with `-EFAULT`.
        unsafe { finish_masked_wait(ucontext, neg_errno(EFAULT), None) };
        return;
    };
    if wrapper.ss.is_null() {
        let local_wrapper = PselectSigmask { ss: &raw const orig_mask, ss_len: KERNEL_SIGSET_SIZE };
        inv.args[5] = (&raw const local_wrapper).addr();
        // SAFETY: `local_wrapper` and `orig_mask` live on the entry stack.
        let rc = unsafe { replay_masked_wait(&inv, ucontext) };
        // SAFETY: Completes `ucontext` with no pending delivery.
        unsafe { finish_masked_wait(ucontext, rc, None) };
        return;
    }
    if wrapper.ss_len != KERNEL_SIGSET_SIZE {
        // SAFETY: Invalid sigsetsize; replay directly to return `-EINVAL`.
        let rc = unsafe { replay(inv) };
        // SAFETY: Completes `ucontext` with no pending delivery.
        unsafe { finish_masked_wait(ucontext, rc, None) };
        return;
    }

    // SAFETY: `u64` accepts any bit pattern.
    let Ok(raw_mask) = (unsafe { usermem::read_user::<u64>(wrapper.ss.addr()) }) else {
        // SAFETY: Completes `ucontext` with `-EFAULT`.
        unsafe { finish_masked_wait(ucontext, neg_errno(EFAULT), None) };
        return;
    };
    let filtered = raw_mask & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
    let local_wrapper = PselectSigmask { ss: &raw const filtered, ss_len: KERNEL_SIGSET_SIZE };
    inv.args[5] = (&raw const local_wrapper).addr();
    // SAFETY: `local_wrapper` and `filtered` live on the entry stack for the wait.
    unsafe { run_wait_with_virtual_sigsys(inv, ucontext, (raw_mask & SIGSYS_BIT) != 0) };
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

/// Replays a masked-wait syscall while `SIGSYS` is virtually blocked, retrying
/// with the remaining timeout if a virtually-blocked non-seccomp `SIGSYS` woke
/// the kernel wait with `-EINTR`.
#[expect(clippy::cast_possible_truncation, reason = "Linux timeout fields fit in i32/i64")]
unsafe fn replay_masked_wait_deferring_sigsys(
    pid: i32,
    tid: i32,
    mut inv: SyscallInvocation,
    ucontext: &mut Ucontext,
) -> isize {
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
        if let (Some(total_ns), Some(t0)) = (initial_timeout_ns, start_ns) {
            let elapsed = monotonic_ns().map_or(total_ns, |now| now.saturating_sub(t0));
            let rem = total_ns - elapsed;
            if rem <= 0 {
                // Deadline expired: perform one non-blocking zero-timeout poll
                // so Linux reports any ready fds or clears `pselect6` fd sets
                // and updates the caller's timespec, then return directly.
                match sysno {
                    Some(Sysno::ppoll | Sysno::pselect6 | Sysno::epoll_pwait2) => {
                        local_ts = timespec { tv_sec: 0, tv_nsec: 0 };
                        if let Some(idx) = ts_arg_idx {
                            inv.args[idx] = (&raw const local_ts).addr();
                        }
                    }
                    Some(Sysno::epoll_pwait) => {
                        inv.args[3] = 0;
                    }
                    _ => {}
                }
                // SAFETY: `inv` points to valid entry-stack wait args with zero timeout.
                let rc = unsafe { replay_masked_wait(&inv, ucontext) };
                if matches!(sysno, Some(Sysno::pselect6 | Sysno::ppoll)) && orig_ts_ptr != 0 {
                    // SAFETY: user pointer from the trapped syscall (see `usermem`).
                    let _ = unsafe { usermem::write_user(orig_ts_ptr, &local_ts) };
                }
                return rc;
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
/// blocked/unblocked bit in the wait mask, and completes `ucontext`.
unsafe fn run_wait_with_virtual_sigsys(
    inv: SyscallInvocation,
    ucontext: &mut Ucontext,
    wait_blocks_sigsys: bool,
) {
    let pid = getpid();
    let tid = gettid();
    let was_blocked = (lookup_tid_flags(pid, tid) & BIT_BLOCKED) != 0;

    if !wait_blocks_sigsys && was_blocked {
        // Wait mask temporarily unblocks `SIGSYS`. If one is already pending,
        // write `-EINTR` to `ucontext` first and then deliver it immediately
        // (matching Linux `sigsuspend`).
        if let Some(info) = take_pending_sigsys(pid, tid) {
            // SAFETY: Delivering pending virtual `SIGSYS` on entry stack.
            unsafe { finish_masked_wait(ucontext, neg_errno(EINTR), Some(info)) };
            return;
        }
        let _ = set_tid_blocked(pid, tid, false);
        // SAFETY: `inv` points to valid entry-stack wait args.
        let rc = unsafe { replay_masked_wait(&inv, ucontext) };
        let _ = set_tid_blocked(pid, tid, true);
        // SAFETY: Completes `ucontext` with no pending delivery.
        unsafe { finish_masked_wait(ucontext, rc, None) };
        return;
    }

    if wait_blocks_sigsys && !was_blocked {
        // Wait mask temporarily blocks `SIGSYS`; defer any non-seccomp `SIGSYS`
        // that arrives during the wait until the wait completes, then write
        // `rc` to `ucontext` BEFORE delivering any deferred `SIGSYS`.
        let _ = set_tid_blocked(pid, tid, true);
        // SAFETY: `inv` points to valid entry-stack wait args.
        let rc = unsafe { replay_masked_wait_deferring_sigsys(pid, tid, inv, ucontext) };
        let _ = set_tid_blocked(pid, tid, false);
        let pending = take_pending_sigsys(pid, tid);
        // SAFETY: Writes `rc` before delivering any deferred `SIGSYS`.
        unsafe { finish_masked_wait(ucontext, rc, pending) };
        return;
    }

    let rc = if wait_blocks_sigsys {
        // SAFETY: `SIGSYS` is already blocked; retry if a deferred `SIGSYS` wakes the wait.
        unsafe { replay_masked_wait_deferring_sigsys(pid, tid, inv, ucontext) }
    } else {
        // SAFETY: `inv` points to valid entry-stack wait args.
        unsafe { replay_masked_wait(&inv, ucontext) }
    };
    // SAFETY: Completes `ucontext` with no pending delivery.
    unsafe { finish_masked_wait(ucontext, rc, None) };
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
        if USER_SIGSYS_ACTION.read().sa_handler != SIG_IGN_RAW {
            record_pending_sigsys(pid, tid, info);
        }
        bump_tid_deferred_seq(pid, tid);
        return;
    }

    debug_assert!(!ucontext.is_null(), "kernel SA_SIGINFO delivery must supply ucontext");
    if ucontext.is_null() {
        return;
    }
    // SAFETY: Caller passed valid kernel signal frame pointers.
    unsafe { deliver_sigsys_now(signum, info, &mut *ucontext) };
}

/// Delivers a `SIGSYS` immediately to the target's virtualized `SIGSYS`
/// disposition (used by both [`forward_non_seccomp_sigsys`] and deferred
/// delivery upon unblocking `SIGSYS`).
///
/// # Safety
///
/// Must be called on the entry stack with valid `info` and writable `ucontext`.
pub unsafe fn deliver_sigsys_now(mut signum: i32, mut info: *mut siginfo, ucontext: &mut Ucontext) {
    let mut deferred_info: siginfo;
    loop {
        let user_act = {
            let act = USER_SIGSYS_ACTION.read();
            if (act.sa_flags & SA_RESETHAND as usize) != 0 && !is_shared_vm_child() {
                USER_SIGSYS_ACTION.write_with(|mut cur| {
                    let prev = cur;
                    if (cur.sa_flags & SA_RESETHAND as usize) != 0 {
                        cur.sa_handler = SIG_DFL_RAW;
                    }
                    (cur, prev)
                })
            } else {
                act
            }
        };

        match user_act.sa_handler {
            SIG_IGN_RAW => return,
            SIG_DFL_RAW => terminate_with_default_sigsys(),
            handler_addr => {
                // Our `SIGSYS` `sa_mask` blocked all maskable signals (including
                // `SIGSYS`) on entry; install the exact mask the target handler
                // expects (`orig_mask | user_act.sa_mask`, with `SIGSYS` unblocked
                // in the kernel so trapped syscalls inside the handler keep working).
                let orig_mask = ucontext.uc_sigmask;
                let handler_mask =
                    (orig_mask | user_act.sa_mask) & !(SIGSYS_BIT | UNBLOCKABLE_MASK);
                // SAFETY: `handler_mask` is a valid stack-local `u64`.
                let _ = unsafe {
                    rt_sigprocmask(SIG_SETMASK, &raw const handler_mask, ptr::null_mut())
                };

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

                // Ensure returning through `rt_sigreturn` cannot re-block `SIGSYS`
                // even if the user handler mutated `ucontext->uc_sigmask`.
                *uc_sigmask_mut(ucontext) &= !SIGSYS_BIT;

                if defer_sigsys && !prev_virtual_blocked {
                    let _ = set_tid_blocked(pid, tid, false);
                    if let Some(next) = take_pending_sigsys(pid, tid) {
                        // Re-block maskable signals before looping so any
                        // `SA_RESETHAND` write in the next iteration cannot be
                        // preempted by an async signal.
                        let block_all = HANDLER_BLOCK_MASK;
                        // SAFETY: `block_all` is a valid stack-local `u64`.
                        let _ = unsafe {
                            rt_sigprocmask(SIG_SETMASK, &raw const block_all, ptr::null_mut())
                        };
                        deferred_info = next;
                        info = &raw mut deferred_info;
                        signum = SIGSYS_I32;
                        continue;
                    }
                }
                return;
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
    use std::sync::Mutex;

    use super::*;

    /// Serializes unit tests that mutate the process-wide `TID_SLOTS` /
    /// `OCCUPIED_SLOTS` tables.
    static TID_SLOT_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn blocked_tid_table_tracks_insert_lookup_pending_and_remove() {
        let _guard = TID_SLOT_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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

    #[test]
    fn discard_all_pending_clears_every_thread_in_pid_only() {
        let _guard = TID_SLOT_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_all_tid_slots();
        let pid = getpid();
        set_tid_blocked(pid, 10, true).unwrap();
        record_pending_sigsys(pid, 10, ptr::null());
        record_pending_sigsys(pid, 11, ptr::null());
        record_pending_sigsys(pid + 1, 10, ptr::null());

        discard_all_pending_sigsys(pid);

        assert_eq!(lookup_tid_flags(pid, 10), BIT_BLOCKED);
        assert_eq!(lookup_tid_flags(pid, 11), 0);
        assert_eq!(lookup_tid_flags(pid + 1, 10), BIT_PENDING);
        clear_all_tid_slots();
    }

    #[test]
    fn seq_action_reads_writes_and_resets_after_fork() {
        let cell = SeqAction::new_dfl();
        assert_eq!(cell.read().sa_handler, SIG_DFL_RAW);
        let custom = KernelSigaction {
            sa_handler: 0x1234,
            sa_flags: SA_SIGINFO as usize,
            sa_restorer: 0x5678,
            sa_mask: 0x9abc,
        };
        let prev = cell.write_with(|cur| (custom, cur));
        assert_eq!(prev.sa_handler, SIG_DFL_RAW);
        assert_eq!(cell.read(), custom);

        // Simulate a sibling thread interrupted mid-write across `fork()`.
        cell.writer_lock.store(true, Ordering::Relaxed);
        cell.seq.fetch_add(1, Ordering::Relaxed);
        cell.reset_after_fork();
        assert_eq!(cell.read(), custom);
    }
}
