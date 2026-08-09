//! Kernel-backed [`Sys`] provider: anonymous mappings via direct syscalls.
//!
//! On Linux the provider relies on nothing from libc — rustix issues raw
//! syscalls, and even the page size is discovered with raw syscalls (see
//! [`page_size`]). macOS has no stable raw-syscall ABI, so calls go through
//! the thin libSystem stubs there.

use core::{
    ptr::{self, NonNull},
    sync::atomic::{AtomicUsize, Ordering},
};

use rustix::mm::{MapFlags, ProtFlags, mmap_anonymous, munmap};

use crate::sys::Sys;

/// Returns the kernel page size, or `None` when it cannot be determined.
///
/// On Linux the value is discovered with raw syscalls only — no libc, no
/// `/proc`, no minimum kernel version (see [`query_page_size`]). On other
/// unix platforms (macOS, where every syscall goes through libSystem by
/// platform contract anyway) it comes from `sysconf` via rustix. Either
/// way the result is a process constant, validated as a power of two and
/// cached, so `map` and `unmap` can never disagree on rounding; an
/// undeterminable page size fails the allocation rather than guessing.
fn page_size() -> Option<usize> {
    static CACHE: AtomicUsize = AtomicUsize::new(0);
    let cached = CACHE.load(Ordering::Relaxed);
    if cached != 0 {
        return Some(cached);
    }
    let page = query_page_size()?;
    if !page.is_power_of_two() {
        return None;
    }
    // First store wins; every query returns the same value, so the cache
    // only avoids repeated probing.
    let _ = CACHE.compare_exchange(0, page, Ordering::Relaxed, Ordering::Relaxed);
    Some(page)
}

/// Discovers the page size by probing, using nothing but raw syscalls.
///
/// `mprotect` fails with `EINVAL` unless its address is a multiple of the
/// page size, so re-protecting a scratch mapping at increasing
/// power-of-two offsets identifies the page size as the first offset the
/// kernel accepts (for a power-of-two page size P, the first power of two
/// P divides is P itself). A handful of syscalls, once per process:
/// async-signal-safe, fork-safe, and independent of libc, `/proc`
/// availability, and kernel version.
///
/// Why not `rustix::param::page_size()` here (it's fine on macOS)? On
/// Linux the allocator must not rely on libc, which rules out rustix's
/// `use-libc-auxv` (`getauxval`) configuration — and rustix's libc-free
/// fallback is unusable *inside* a global allocator:
///
/// - Its lazy init tries `prctl(PR_GET_AUXV)` (kernel 6.4+ only) and
///   otherwise reads `/proc/self/auxv`; with rustix's `alloc` feature
///   enabled, that read path heap-allocates (`Vec`) — through *this*
///   allocator, whose `map` is the caller waiting on the page size —
///   recursing unboundedly. And we cannot pin `alloc` off: Cargo feature
///   unification lets any other rustix user in the build graph enable it
///   for our copy.
/// - It panics on read errors, truncated auxv, or both sources being
///   unavailable, where this allocator requires failure to surface as
///   `None` (panic formatting itself allocates, re-entering the same
///   uninitialized path).
///
/// The probe has neither problem: no allocation, no panic, no minimum
/// kernel, and its worst case is `None`.
#[cfg(target_os = "linux")]
fn query_page_size() -> Option<usize> {
    use rustix::mm::{MprotectFlags, mprotect};

    /// Smallest page size of any Linux configuration; the probe starts
    /// here.
    const MIN_PROBE_PAGE: usize = 4096;
    /// Generous upper bound for the probe; no supported configuration
    /// uses larger pages.
    const MAX_PROBE_PAGE: usize = 1 << 20;

    // Large enough that every probe below stays inside the mapping even
    // after the kernel rounds the one-byte length up to a full page.
    let scratch = map_anonymous(2 * MAX_PROBE_PAGE)?;
    let mut page = None;
    let mut offset = MIN_PROBE_PAGE;
    while offset <= MAX_PROBE_PAGE {
        // SAFETY: `scratch + offset` (plus the page-rounded single byte)
        // lies within the scratch mapping, which we exclusively own; the
        // protection flags match the mapping's existing ones.
        let accepted = unsafe {
            mprotect(
                scratch.as_ptr().add(offset).cast(),
                1,
                MprotectFlags::READ | MprotectFlags::WRITE,
            )
        }
        .is_ok();
        if accepted {
            page = Some(offset);
            break;
        }
        offset *= 2;
    }
    // SAFETY: releasing the scratch mapping created above.
    let _ = unsafe { munmap(scratch.as_ptr().cast(), 2 * MAX_PROBE_PAGE) };
    page
}

#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "must match the signature of the fallible Linux probe variant"
)]
fn query_page_size() -> Option<usize> {
    Some(rustix::param::page_size())
}

const fn round_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn map_anonymous(len: usize) -> Option<NonNull<u8>> {
    // SAFETY: a fresh anonymous private mapping at no particular address
    // has no memory-safety preconditions.
    let ptr = unsafe {
        mmap_anonymous(ptr::null_mut(), len, ProtFlags::READ | ProtFlags::WRITE, MapFlags::PRIVATE)
    }
    .ok()?;
    NonNull::new(ptr.cast::<u8>())
}

/// Kernel-backed provider: anonymous mappings obtained through direct
/// syscalls, alignment achieved by over-mapping and trimming.
pub struct Mmap;

impl Sys for Mmap {
    fn map(size: usize, align: usize) -> Option<NonNull<u8>> {
        debug_assert!(align.is_power_of_two());
        let page = page_size()?;
        let size = round_up(size.max(1), page);

        if align <= page {
            // Mapping results are aligned to the (verified real) page
            // size.
            return map_anonymous(size);
        }

        // Over-map by `align`, then unmap the misaligned head and the
        // leftover tail. All cut points are page-aligned: `raw` and
        // `aligned` are page-aligned, and `size`/`align` are multiples of
        // the page size.
        let raw = map_anonymous(size.checked_add(align)?)?;
        let raw_addr = raw.as_ptr().addr();
        let aligned_addr = round_up(raw_addr, align);
        let head = aligned_addr - raw_addr;
        let tail = align - head;
        if head > 0 {
            // SAFETY: `[raw_addr, raw_addr + head)` lies within the fresh
            // mapping and `raw_addr` is page-aligned. Failure is
            // impossible for a region we own; if it happened anyway the
            // pages would merely stay mapped.
            let _ = unsafe { munmap(raw.as_ptr().cast(), head) };
        }
        if tail > 0 {
            let tail_start = raw.as_ptr().with_addr(aligned_addr + size);
            // SAFETY: `[aligned_addr + size, raw_addr + size + align)`
            // lies within the fresh mapping and its start is page-aligned
            // (see above). Failure is impossible for a region we own.
            let _ = unsafe { munmap(tail_start.cast(), tail) };
        }
        // `aligned_addr` lies inside a successful mapping and so can
        // never be zero, but checked construction costs nothing here.
        core::num::NonZero::new(aligned_addr).map(|addr| raw.with_addr(addr))
    }

    unsafe fn unmap(ptr: NonNull<u8>, size: usize, _align: usize) {
        // A successful `map` proved the page size, so this cannot fail
        // for a live mapping; if it somehow did, leaking the region is
        // the only safe response.
        let Some(page) = page_size() else { return };
        // Whether or not `map` trimmed for alignment, the retained region
        // is exactly `[ptr, ptr + round_up(size, page))`.
        let size = round_up(size.max(1), page);
        // SAFETY: caller contract — `ptr`/`size` describe a live mapping
        // returned by `map`. Failure is impossible for a region we own.
        let _ = unsafe { munmap(ptr.as_ptr().cast(), size) };
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::Mmap;
    use crate::sys::Sys;

    #[test]
    fn real_mappings_are_aligned_zeroed_and_writable() {
        for (size, align) in [(1, 1), (4096, 4096), (100, 1 << 20), (5 << 20, 4096)] {
            let ptr = Mmap::map(size, align).unwrap();
            assert_eq!(ptr.as_ptr().addr() % align, 0, "align {align}");
            for i in 0..size {
                // SAFETY: fresh exclusive mapping of at least `size` bytes.
                assert_eq!(unsafe { ptr.as_ptr().add(i).read() }, 0);
            }
            // SAFETY: fresh exclusive mapping of at least `size` bytes.
            unsafe { ptr.as_ptr().write_bytes(0x5A, size) };
            // SAFETY: mapped above with the same size and alignment.
            unsafe { Mmap::unmap(ptr, size, align) };
        }
    }
}
