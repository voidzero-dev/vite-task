//! Process parameters.
//!
//! On Linux, `page_size` asks the kernel itself — this crate stands in for
//! libc, so it cannot lean on libc's startup knowledge. The kernel hands a
//! process its auxiliary vector exactly once, above the stack pointer at
//! entry, and only the code owning the entry point (a libc, or ld.so) sees
//! it; the retroactive interfaces (`prctl(PR_GET_AUXV)`, `/proc/self/auxv`)
//! need a 6.4 kernel or a mounted `/proc`. So instead of the auxiliary
//! vector, the value is learned from syscall behavior that any kernel
//! version guarantees: see [`linux::page_size`].
//!
//! On macOS, where every syscall goes through libSystem by platform contract
//! anyway, it calls `sysconf` directly.

#[cfg(any(target_os = "linux", target_os = "none"))]
pub use linux::page_size;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
pub use mac::page_size;

#[cfg(any(target_os = "linux", target_os = "none"))]
mod linux {
    use core::{
        ptr,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use crate::mm::{self, MapFlags, MprotectFlags, ProtFlags};

    /// Smallest page size of any Linux configuration; the probe starts
    /// here.
    const MIN_PROBE_PAGE: usize = 4096;
    /// Generous upper bound for the probe; no supported configuration uses
    /// larger pages.
    const MAX_PROBE_PAGE: usize = 1 << 20;

    /// Returns the size of a memory page, or 0 if it cannot be determined.
    ///
    /// Discovered from the kernel with a handful of raw syscalls, once per
    /// process, and cached in an atomic: no libc, no auxiliary vector, no
    /// `/proc`, no minimum kernel version, no allocation, and no init to
    /// call first — safe from the first instruction on, in signal handlers,
    /// and in the child of `fork()`.
    ///
    /// Zero — the kernel refusing a scratch mapping, i.e. out of memory —
    /// is not cached, so a later call retries; callers treat it as "fail
    /// this operation" (an allocator refuses the allocation).
    #[must_use]
    pub fn page_size() -> usize {
        static CACHE: AtomicUsize = AtomicUsize::new(0);
        let cached = CACHE.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }
        let page = probe_page_size();
        if page != 0 {
            // Every probe returns the same value; the cache only avoids
            // repeated probing.
            CACHE.store(page, Ordering::Relaxed);
        }
        page
    }

    /// Discovers the page size by probing, using nothing but raw syscalls.
    ///
    /// `mprotect` fails with `EINVAL` unless its address is a multiple of
    /// the page size, and `mmap` returns page-aligned bases — both
    /// kernel-guaranteed on every version. So re-protecting a scratch
    /// mapping at increasing power-of-two offsets identifies the page size
    /// as the first offset the kernel accepts: for a power-of-two page
    /// size P, the first power of two P divides is P itself.
    ///
    /// Why not an auxiliary-vector helper? Its Linux sources are exactly the
    /// ones this crate must not assume: libc startup state,
    /// `prctl(PR_GET_AUXV)` (kernel 6.4+), `/proc/self/auxv`, or an initial
    /// stack pointer retained from process entry. The probe has none of those
    /// requirements: no allocation, no panic, no minimum kernel, and its worst
    /// case is 0.
    #[cold]
    fn probe_page_size() -> usize {
        // Large enough that every probe below stays inside the mapping
        // even after the kernel rounds the one-byte length up to a full
        // page.
        const SCRATCH_LEN: usize = 2 * MAX_PROBE_PAGE;

        // SAFETY: a fresh anonymous private mapping at no particular
        // address has no memory-safety preconditions.
        let Ok(base) = (unsafe {
            mm::mmap_anonymous(
                ptr::null_mut(),
                SCRATCH_LEN,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            )
        }) else {
            return 0;
        };
        let mut page = 0;
        let mut offset = MIN_PROBE_PAGE;
        while offset <= MAX_PROBE_PAGE {
            // SAFETY: `base + offset` (plus the page-rounded single byte)
            // lies within the scratch mapping, which we exclusively own;
            // the protection flags match the mapping's existing ones.
            let accepted = unsafe {
                mm::mprotect(
                    base.cast::<u8>().add(offset).cast(),
                    1,
                    MprotectFlags::READ | MprotectFlags::WRITE,
                )
            }
            .is_ok();
            if accepted {
                page = offset;
                break;
            }
            offset *= 2;
        }
        // SAFETY: releasing the scratch mapping created above; nothing
        // uses it afterwards.
        let _ = unsafe { mm::munmap(base, SCRATCH_LEN) };
        page
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn probe_is_stable_and_cached() {
            assert_eq!(probe_page_size(), page_size());
            assert_eq!(page_size(), page_size());
            #[cfg(target_arch = "x86_64")]
            assert_eq!(page_size(), 4096);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
mod mac {
    /// Returns the process page size, or zero if libSystem cannot report it.
    #[must_use]
    pub fn page_size() -> usize {
        // SAFETY: `sysconf` accepts this constant by value and accesses no
        // caller-owned memory.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        usize::try_from(page).unwrap_or(0)
    }
}
