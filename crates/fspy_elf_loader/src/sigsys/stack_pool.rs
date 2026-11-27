//! Lock-free pool of private `SIGSYS` handler stacks.
//!
//! Trapped syscalls can arrive on tiny target-owned stacks — for example musl's
//! `posix_spawn` (`char stack[1024 + PATH_MAX]` = 5 KiB, almost entirely
//! consumed by the `AArch64` kernel `rt_sigframe`) or musl Rust's 12 KiB
//! `sigaltstack`. Switching `SP` to a private 64 KiB stack before dispatching
//! our `SIGSYS` handler guarantees headroom across both debug and release builds
//! without risking stack overflow or corrupting a `vfork` parent's stack frame.

use core::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use linux_raw_sys::general::{MAP_ANONYMOUS, MAP_PRIVATE, PROT_NONE, PROT_READ, PROT_WRITE};
use syscalls::Sysno;

use super::arch::{call_on_stack, syscall6};

/// Minimum size of each private `SIGSYS` handler stack (64 KiB, page-aligned on
/// 4 KiB, 16 KiB, and 64 KiB kernels). Raised dynamically via [`init_and_warm`]
/// if the kernel advertises a larger `AT_MINSIGSTKSZ` (e.g. `AArch64` SVE/SME
/// or AVX-512 / AMX hosts).
pub const MIN_SIGSYS_STACK_SIZE: usize = 64 * 1024;

/// Number of reusable stacks retained in the lock-free cache.
///
/// Sized for 64 concurrent trapping threads so high-core-count parallel file
/// walkers (e.g. `oxlint`, `biome`, `rspack` on 32–64-core CI runners) stay
/// inside the pool without `mmap`/`munmap` TLB shootdowns. Stacks are allocated
/// lazily on demand, so single-threaded workloads only map one stack.
pub const STACK_POOL_SLOTS: usize = 64;

/// Active usable byte size of each private `SIGSYS` stack (multiple of the host
/// page size, at least [`MIN_SIGSYS_STACK_SIZE`]), excluding the 1-page
/// `PROT_NONE` guard page mapped immediately below it.
static STACK_SIZE: AtomicUsize = AtomicUsize::new(MIN_SIGSYS_STACK_SIZE);

/// Byte size of the `PROT_NONE` guard page placed at the bottom of each pooled
/// stack (`0` until initialized, in which case `page_size()` is queried).
static GUARD_SIZE: AtomicUsize = AtomicUsize::new(0);

/// Cached `stack_top` pointers (`0` means empty slot).
///
/// Accessed directly from [`super::arch::sigsys_trampoline`] so the common-case
/// pop/push uses zero entry-stack bytes even in unoptimized debug builds.
pub static STACK_POOL: [AtomicUsize; STACK_POOL_SLOTS] =
    [const { AtomicUsize::new(0) }; STACK_POOL_SLOTS];

/// Configures the private `SIGSYS` stack size and guard page from
/// `AT_MINSIGSTKSZ` and `page_size`, then pre-allocates one stack during
/// single-threaded `sigsys::install` so the first trapped syscall never needs
/// to invoke `mmap` on a tiny caller stack.
pub fn init_and_warm(min_sigstksz: usize, page_size: usize) {
    let page = page_size.max(4096);
    GUARD_SIZE.store(page, Ordering::Release);
    // Reserve at least `2 * AT_MINSIGSTKSZ + 16 KiB` (and >= 64 KiB) so both
    // Phase 1 and AArch64 `vfork` `sigaltstack` (`stack_size / 2`) always
    // satisfy kernel `MINSIGSTKSZ` with ample headroom.
    let needed =
        min_sigstksz.saturating_mul(2).saturating_add(16 * 1024).max(MIN_SIGSYS_STACK_SIZE);
    let aligned = needed.checked_add(page - 1).map_or(MIN_SIGSYS_STACK_SIZE, |n| n & !(page - 1));
    STACK_SIZE.store(aligned.max(MIN_SIGSYS_STACK_SIZE), Ordering::Release);
    if let Some(top) = alloc_stack() {
        release(top);
    }
}

/// Returns the active usable byte size of each private `SIGSYS` stack.
#[inline]
#[must_use]
pub fn stack_size() -> usize {
    STACK_SIZE.load(Ordering::Acquire)
}

#[inline]
fn guard_size() -> usize {
    let guard = GUARD_SIZE.load(Ordering::Acquire);
    if guard != 0 { guard } else { fspy_nostd::param::page_size().max(4096) }
}

/// Executes `entry(ctx)` on a private 64 KiB stack, restoring the caller's
/// stack before returning.
///
/// Falls back to running on the current stack only if `mmap` fails (OOM).
///
/// # Safety
///
/// `ctx` must satisfy the safety contract expected by `entry`, and maskable
/// asynchronous signals must remain blocked for the duration of `entry` so an
/// `SA_ONSTACK` signal cannot observe `SP` outside the thread's `sigaltstack`.
pub unsafe fn run_on_private_stack(ctx: *mut c_void, entry: extern "C" fn(*mut c_void)) {
    let Some(stack_top) = acquire() else {
        entry(ctx);
        return;
    };
    // SAFETY: `stack_top` is page-aligned (therefore 16-byte aligned) and owns
    // `SIGSYS_STACK_SIZE` writable bytes below it exclusively until `release`.
    unsafe { call_on_stack(stack_top, ctx, entry) };
    release(stack_top);
}

/// Cold-path allocator called by [`super::arch::sigsys_trampoline`] when all
/// [`STACK_POOL`] slots are empty. Returns `0` on OOM.
#[inline(never)]
pub extern "C" fn alloc_stack_raw() -> usize {
    alloc_stack().unwrap_or(0)
}

/// Cold-path deallocator called by [`super::arch::sigsys_trampoline`] when all
/// [`STACK_POOL`] slots are full.
#[inline(never)]
pub extern "C" fn free_stack_raw(stack_top: usize) {
    if stack_top != 0 {
        free_stack(stack_top);
    }
}

/// Acquires a private `SIGSYS` stack top from [`STACK_POOL`] (or `mmap` on miss).
pub fn acquire_stack() -> Option<usize> {
    acquire()
}

/// Returns a private `SIGSYS` stack top to [`STACK_POOL`] (or `munmap` if full).
pub fn release_stack(stack_top: usize) {
    release(stack_top);
}

fn acquire() -> Option<usize> {
    for slot in &STACK_POOL {
        if slot.load(Ordering::Relaxed) != 0 {
            let top = slot.swap(0, Ordering::AcqRel);
            if top != 0 {
                return Some(top);
            }
        }
    }
    alloc_stack()
}

fn release(stack_top: usize) {
    for slot in &STACK_POOL {
        if slot.load(Ordering::Relaxed) == 0
            && slot.compare_exchange(0, stack_top, Ordering::Release, Ordering::Relaxed).is_ok()
        {
            return;
        }
    }
    free_stack(stack_top);
}

#[inline(never)]
fn alloc_stack() -> Option<usize> {
    let usable = stack_size();
    let guard = guard_size();
    let total = usable.checked_add(guard)?;
    // SAFETY: anonymous private `mmap` of `guard + usable` bytes starting as
    // `PROT_NONE`; the usable upper pages are made read/write below while the
    // lowest page stays `PROT_NONE` as a stack-overflow guard.
    let rc = unsafe {
        syscall6(
            Sysno::mmap as usize,
            0,
            total,
            PROT_NONE as usize,
            (MAP_PRIVATE | MAP_ANONYMOUS) as usize,
            (-1_isize).cast_unsigned(),
            0,
        )
    };
    if rc == 0 {
        // SAFETY: `mmap` succeeded at address 0 (`vm.mmap_min_addr == 0`);
        // release the mapping before failing so it is not leaked.
        let _ = unsafe { syscall6(Sysno::munmap as usize, 0, total, 0, 0, 0, 0) };
        return None;
    }
    if rc < 0 {
        return None;
    }
    let base = rc.cast_unsigned();
    let Some(usable_base) = base.checked_add(guard) else {
        // SAFETY: release mapping if address arithmetic ever overflowed.
        let _ = unsafe { syscall6(Sysno::munmap as usize, base, total, 0, 0, 0, 0) };
        return None;
    };
    let Some(top) = usable_base.checked_add(usable) else {
        // SAFETY: release mapping if address arithmetic ever overflowed.
        let _ = unsafe { syscall6(Sysno::munmap as usize, base, total, 0, 0, 0, 0) };
        return None;
    };
    // SAFETY: `usable_base..top` lies exclusively inside our fresh `mmap`.
    let mrc = unsafe {
        syscall6(
            Sysno::mprotect as usize,
            usable_base,
            usable,
            (PROT_READ | PROT_WRITE) as usize,
            0,
            0,
            0,
        )
    };
    if mrc < 0 {
        // SAFETY: release fresh mapping on `mprotect` failure.
        let _ = unsafe { syscall6(Sysno::munmap as usize, base, total, 0, 0, 0, 0) };
        return None;
    }
    Some(top)
}

#[inline(never)]
fn free_stack(stack_top: usize) {
    let total = stack_size() + guard_size();
    let base = stack_top - total;
    // SAFETY: `base..stack_top` (guard + usable) was mapped by `alloc_stack`.
    let _ = unsafe { syscall6(Sysno::munmap as usize, base, total, 0, 0, 0, 0) };
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::AtomicUsize;

    use super::*;

    static OBSERVED_SP: AtomicUsize = AtomicUsize::new(0);
    static OBSERVED_ARG: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn record_stack_and_arg(arg: *mut c_void) {
        let local = 0usize;
        OBSERVED_SP.store(core::ptr::from_ref(&local).addr(), Ordering::Release);
        OBSERVED_ARG.store(arg.addr(), Ordering::Release);
    }

    #[test]
    fn acquires_reuses_and_switches_to_private_stack() {
        let size = stack_size();
        let top = acquire().expect("mmap private stack");
        assert_eq!(top % 16, 0);
        let sentinel = core::ptr::without_provenance_mut::<c_void>(0x1234_5678);
        // SAFETY: `top` is a freshly acquired private stack and `record_stack_and_arg`
        // never dereferences `sentinel`.
        unsafe { call_on_stack(top, sentinel, record_stack_and_arg) };
        let sp = OBSERVED_SP.load(Ordering::Acquire);
        assert!(
            (top - size..top).contains(&sp),
            "expected sp {sp:#x} inside [{:#x}, {top:#x})",
            top - size
        );
        assert_eq!(OBSERVED_ARG.load(Ordering::Acquire), 0x1234_5678);

        // Verify the bottom guard page is `PROT_NONE` (`---p`) and the usable
        // stack region is read-write (`rw-p`).
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let prot_of = |addr: usize| {
            maps.lines()
                .find_map(|line| {
                    let mut fields = line.split_whitespace();
                    let (start, end) = fields.next()?.split_once('-')?;
                    let range = usize::from_str_radix(start, 16).ok()?
                        ..usize::from_str_radix(end, 16).ok()?;
                    range.contains(&addr).then(|| fields.next().unwrap())
                })
                .unwrap()
        };
        assert_eq!(prot_of(top - size - guard_size()), "---p");
        assert_eq!(prot_of(top - size), "rw-p");

        release(top);
        let reused = acquire().expect("re-acquire cached stack");
        assert_eq!(reused, top);
        free_stack(reused);
    }
}
