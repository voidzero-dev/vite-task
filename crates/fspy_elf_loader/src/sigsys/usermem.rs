//! Fault-safe user memory copies for the `SIGSYS` dispatcher.
//!
//! Because `HANDLER_BLOCK_MASK` blocks `SIGSEGV` and `SIGBUS` while the
//! `SIGSYS` handler runs, directly dereferencing an invalid user pointer passed
//! to a trapped syscall would immediately kill the process instead of returning
//! `-EFAULT`.
//!
//! When permitted by the kernel/container policy, this module copies through
//! `process_vm_readv` / `process_vm_writev` on our own PID, which cleanly
//! returns `-EFAULT` on unmapped or non-accessible addresses without raising a
//! signal. If an outer sandbox blocks `process_vm_readv` / `process_vm_writev`
//! (`EPERM`/`ENOSYS`), it probes page accessibility through always-permitted
//! kernel syscalls (`rt_sigprocmask` / `getrandom`) before touching user memory
//! so unreadable or read-only pointers still return `-EFAULT` instead of
//! killing the process, without relying on `/proc` or leaving partial writes
//! behind on multi-page `EFAULT`.

use core::{
    mem::{MaybeUninit, size_of},
    ptr, slice,
    sync::atomic::{AtomicBool, Ordering},
};

use fspy_nostd::{Error, Result};
use linux_raw_sys::{
    errno::{EAGAIN, EINTR, EINVAL, ENOSYS, EPERM},
    general::{AT_FDCWD, GRND_NONBLOCK, SIG_SETMASK, iovec},
};
use syscalls::Sysno;

use super::arch::{getpid, neg_errno, syscall6};

/// Minimum valid user-space address (page 0 is never mapped on Linux).
const MIN_USER_ADDR: usize = 4096;

/// NUL-terminated `/proc/self/exe` literal used only as a strict (`rc == 1`)
/// secondary fallback for `< 8`-byte segments when both `process_vm_writev`
/// and `getrandom` are blocked by an outer sandbox.
const PROC_SELF_EXE: &[u8] = b"/proc/self/exe\0";

/// Whether `process_vm_readv` / `process_vm_writev` on our own PID succeeded
/// during single-threaded `sigsys::install`.
static PROCESS_VM_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Probes whether the page containing `addr` is readable by asking the kernel's
/// `rt_sigprocmask` (`how = !0`) to `copy_from_user` 8 aligned bytes in that
/// same page (which returns `-EFAULT` on unmapped or `PROT_NONE` pages and
/// `-EINVAL` on readable pages with zero side effects). Fails closed on any
/// other errno.
#[inline]
fn probe_page_readable(addr: usize) -> bool {
    let aligned = addr & !7;
    // SAFETY: `rt_sigprocmask(!0, aligned, NULL, 8)` never mutates the signal
    // mask; the kernel checks `copy_from_user(aligned, 8)` before validating `how`.
    let rc = unsafe {
        syscall6(Sysno::rt_sigprocmask as usize, !0_usize, aligned, 0, size_of::<u64>(), 0, 0)
    };
    rc == neg_errno(EINVAL)
}

/// Probes that every page touched by `addr..addr + len` is readable.
fn probe_range_readable(addr: usize, len: usize) -> Result<()> {
    let page = fspy_nostd::param::page_size().max(4096);
    let mut off = 0_usize;
    while off < len {
        let cur = addr.checked_add(off).ok_or(Error::FAULT)?;
        if !probe_page_readable(cur) {
            return Err(Error::FAULT);
        }
        let page_rem = page - (cur & (page - 1));
        off = off.saturating_add(page_rem);
    }
    Ok(())
}

/// Probes that every page touched by `addr..addr + len` is writable when
/// `process_vm_writev` is blocked by an outer sandbox, without leaving mutated
/// bytes behind on readable pages if a later page faults.
///
/// Primary probe: `getrandom(cur, 1, GRND_NONBLOCK)` (strict `rc == 1`, zero
/// `/proc` dependency). If `getrandom` returns `ENOSYS`, `EPERM`, or `EAGAIN`:
/// - For segments `>= 8` bytes, tries `rt_sigprocmask(SIG_SETMASK, NULL, cur, 8)`
///   (strict `rc == 0`, zero `/proc` dependency).
/// - For `EAGAIN` on `< 8`-byte segments, retries blocking `getrandom(cur, 1, 0)`
///   (retrying `EINTR`).
/// - For `ENOSYS`/`EPERM` on `< 8`-byte segments, falls back to
///   `readlinkat(AT_FDCWD, "/proc/self/exe", cur, 1)` strictly requiring `rc == 1`.
///
/// Any other error fails closed with [`Error::FAULT`]. When the probed page is
/// readable, the original byte(s) are saved before probing and restored
/// immediately afterwards so a subsequent page fault leaves the destination
/// untouched.
fn probe_range_writable(addr: usize, len: usize) -> Result<()> {
    let page = fspy_nostd::param::page_size().max(4096);
    let mut off = 0_usize;
    while off < len {
        let cur = addr.checked_add(off).ok_or(Error::FAULT)?;
        let page_rem = page - (cur & (page - 1));
        let seg_len = page_rem.min(len - off);
        let readable = probe_page_readable(cur);
        let saved_byte = if readable {
            // SAFETY: `cur` is on a kernel-verified readable page.
            Some(unsafe { ptr::with_exposed_provenance::<u8>(cur).read() })
        } else {
            None
        };
        // SAFETY: `cur` is inside the caller's destination range; `getrandom`
        // uses kernel `copy_to_user` and returns `1` iff writable.
        let rc =
            unsafe { syscall6(Sysno::getrandom as usize, cur, 1, GRND_NONBLOCK as usize, 0, 0, 0) };
        if rc == 1 {
            if let Some(orig) = saved_byte {
                // SAFETY: `getrandom` just proved `cur` is writable.
                unsafe { ptr::with_exposed_provenance_mut::<u8>(cur).write(orig) };
            }
            off += seg_len;
            continue;
        }
        let getrandom_unavailable =
            rc == neg_errno(ENOSYS) || rc == neg_errno(EPERM) || rc == neg_errno(EAGAIN);
        if getrandom_unavailable && seg_len >= size_of::<u64>() {
            let saved_word = if readable {
                // SAFETY: `cur..cur+8` stays within one kernel-verified readable page.
                Some(unsafe { ptr::with_exposed_provenance::<u64>(cur).read_unaligned() })
            } else {
                None
            };
            // SAFETY: `rt_sigprocmask(SIG_SETMASK, NULL, cur, 8)` writes the
            // current sigmask via `copy_to_user` and returns `0` iff writable.
            let mask_rc = unsafe {
                syscall6(
                    Sysno::rt_sigprocmask as usize,
                    SIG_SETMASK as usize,
                    0,
                    cur,
                    size_of::<u64>(),
                    0,
                    0,
                )
            };
            if mask_rc == 0 {
                if let Some(orig) = saved_word {
                    // SAFETY: `rt_sigprocmask` just proved `cur..cur+8` is writable.
                    unsafe {
                        ptr::with_exposed_provenance_mut::<u64>(cur).write_unaligned(orig);
                    }
                }
                off += seg_len;
                continue;
            }
            return Err(Error::FAULT);
        }
        if rc == neg_errno(EAGAIN) {
            // Uninitialized CRNG on pre-5.18 kernels for `< 8`-byte segment:
            // retry blocking `getrandom`, looping past any `EINTR`.
            loop {
                // SAFETY: `cur` is inside the caller's destination range.
                let brc = unsafe { syscall6(Sysno::getrandom as usize, cur, 1, 0, 0, 0, 0) };
                if brc == neg_errno(EINTR) {
                    continue;
                }
                if brc == 1 {
                    if let Some(orig) = saved_byte {
                        // SAFETY: `getrandom` just proved `cur` is writable.
                        unsafe { ptr::with_exposed_provenance_mut::<u8>(cur).write(orig) };
                    }
                    off += seg_len;
                    break;
                }
                return Err(Error::FAULT);
            }
            continue;
        }
        if (rc == neg_errno(ENOSYS) || rc == neg_errno(EPERM)) && seg_len < size_of::<u64>() {
            // Strict `rc == 1` fallback for `< 8`-byte segments when both
            // `process_vm_writev` and `getrandom` are denied (fails closed
            // if `/proc` is absent or `readlinkat` is denied).
            // SAFETY: `PROC_SELF_EXE` is NUL-terminated; `cur` is inside destination.
            let rl_rc = unsafe {
                syscall6(
                    Sysno::readlinkat as usize,
                    AT_FDCWD.cast_unsigned() as usize,
                    PROC_SELF_EXE.as_ptr().addr(),
                    cur,
                    1,
                    0,
                    0,
                )
            };
            if rl_rc == 1 {
                if let Some(orig) = saved_byte {
                    // SAFETY: `readlinkat` just proved `cur` is writable.
                    unsafe { ptr::with_exposed_provenance_mut::<u8>(cur).write(orig) };
                }
                off += seg_len;
                continue;
            }
        }
        return Err(Error::FAULT);
    }
    Ok(())
}

/// Probes once during single-threaded initialization whether
/// `process_vm_readv` works on the current process.
pub fn init() {
    let sample: u64 = 0x1122_3344_5566_7788;
    let mut out: u64 = 0;
    let local = iovec { iov_base: (&raw mut out).cast(), iov_len: size_of::<u64>() as u64 };
    let remote =
        iovec { iov_base: (&raw const sample).cast_mut().cast(), iov_len: size_of::<u64>() as u64 };
    // SAFETY: `local` and `remote` point to live stack-local `u64`s.
    let rc = unsafe {
        syscall6(
            Sysno::process_vm_readv as usize,
            getpid().cast_unsigned() as usize,
            (&raw const local).addr(),
            1,
            (&raw const remote).addr(),
            1,
            0,
        )
    };
    PROCESS_VM_AVAILABLE
        .store(rc == size_of::<u64>().cast_signed() && out == sample, Ordering::Release);
}

#[inline]
fn check_range(addr: usize, len: usize) -> Result<()> {
    if len == 0 {
        return Ok(());
    }
    if addr < MIN_USER_ADDR {
        return Err(Error::FAULT);
    }
    addr.checked_add(len).ok_or(Error::FAULT)?;
    Ok(())
}

/// Copies a `Copy` value of type `T` from user address `addr`, returning
/// [`Error::FAULT`] if `addr` is unreadable.
///
/// # Safety
///
/// `T` must be valid for any bit pattern (e.g. integers or `#[repr(C)]` kernel
/// UAPI structs of integers/raw pointers).
pub unsafe fn read_user<T: Copy>(addr: usize) -> Result<T> {
    let len = size_of::<T>();
    check_range(addr, len)?;
    let mut dst = MaybeUninit::<T>::uninit();
    if PROCESS_VM_AVAILABLE.load(Ordering::Acquire) {
        let local = iovec { iov_base: dst.as_mut_ptr().cast(), iov_len: len as u64 };
        let remote =
            iovec { iov_base: ptr::with_exposed_provenance_mut(addr), iov_len: len as u64 };
        // SAFETY: `local` points to `dst` (`len` bytes); kernel validates `remote`.
        let rc = unsafe {
            syscall6(
                Sysno::process_vm_readv as usize,
                getpid().cast_unsigned() as usize,
                (&raw const local).addr(),
                1,
                (&raw const remote).addr(),
                1,
                0,
            )
        };
        if rc == len.cast_signed() {
            // SAFETY: kernel wrote all `len` bytes into `dst`, and `T` accepts any bit pattern.
            return Ok(unsafe { dst.assume_init() });
        }
        if rc < 0 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Linux negative syscall errno fits in u32"
            )]
            let errno = (-rc).cast_unsigned() as u32;
            if errno != EPERM && errno != ENOSYS {
                return Err(Error::FAULT);
            }
        } else {
            return Err(Error::FAULT);
        }
    }
    // Fallback when `process_vm_readv` is blocked by an outer sandbox: verify
    // page readability via `rt_sigprocmask(!0, ...)` before touching memory.
    probe_range_readable(addr, len)?;
    // SAFETY: `check_range` and `probe_range_readable` validated `addr..addr+len`.
    Ok(unsafe { ptr::with_exposed_provenance::<T>(addr).read_unaligned() })
}

/// Copies a `Copy` value of type `T` to user address `addr`, returning
/// [`Error::FAULT`] if `addr` is unwritable.
pub fn write_user<T: Copy>(addr: usize, value: &T) -> Result<()> {
    // SAFETY: `value` is a valid initialized `T` of `size_of::<T>()` bytes.
    let bytes = unsafe { slice::from_raw_parts(ptr::from_ref(value).cast::<u8>(), size_of::<T>()) };
    write_user_bytes(addr, bytes)
}

/// Copies `src` bytes to user address `addr`, returning [`Error::FAULT`] if the
/// destination range is unwritable.
pub fn write_user_bytes(addr: usize, src: &[u8]) -> Result<()> {
    let len = src.len();
    check_range(addr, len)?;
    if len == 0 {
        return Ok(());
    }
    if PROCESS_VM_AVAILABLE.load(Ordering::Acquire) {
        let local = iovec { iov_base: src.as_ptr().cast_mut().cast(), iov_len: len as u64 };
        let remote =
            iovec { iov_base: ptr::with_exposed_provenance_mut(addr), iov_len: len as u64 };
        // SAFETY: `local` points to `src` (`len` bytes); kernel validates `remote`.
        let rc = unsafe {
            syscall6(
                Sysno::process_vm_writev as usize,
                getpid().cast_unsigned() as usize,
                (&raw const local).addr(),
                1,
                (&raw const remote).addr(),
                1,
                0,
            )
        };
        if rc == len.cast_signed() {
            return Ok(());
        }
        if rc < 0 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Linux negative syscall errno fits in u32"
            )]
            let errno = (-rc).cast_unsigned() as u32;
            if errno != EPERM && errno != ENOSYS {
                return Err(Error::FAULT);
            }
        } else {
            return Err(Error::FAULT);
        }
    }
    // Fallback when `process_vm_writev` is blocked by an outer sandbox: verify
    // page writability via `rt_sigprocmask` / `readlinkat` before copying.
    probe_range_writable(addr, len)?;
    // SAFETY: `check_range` and `probe_range_writable` validated `addr..addr+len`.
    unsafe {
        ptr::copy_nonoverlapping(src.as_ptr(), ptr::with_exposed_provenance_mut(addr), len);
    }
    Ok(())
}

/// Safely reads a NUL-terminated C string from user address `addr` into `buf`,
/// returning the byte slice excluding the NUL terminator.
///
/// Returns `Ok(None)` if no NUL byte is found within `buf.len()` bytes (all
/// `buf.len()` bytes have been populated in `buf`), or `Err(Error::FAULT)` if
/// `addr` is unreadable before a NUL terminator is reached.
pub fn read_user_cstr(addr: usize, buf: &mut [MaybeUninit<u8>]) -> Result<Option<&[u8]>> {
    if addr < MIN_USER_ADDR {
        return Err(Error::FAULT);
    }
    if buf.is_empty() {
        return Ok(None);
    }
    let page = fspy_nostd::param::page_size().max(4096);
    let use_vm = PROCESS_VM_AVAILABLE.load(Ordering::Acquire);
    let pid = if use_vm { getpid().cast_unsigned() as usize } else { 0 };

    let mut copied = 0_usize;
    while copied < buf.len() {
        let cur_addr = addr.checked_add(copied).ok_or(Error::FAULT)?;
        // Clamp each read to the current page so we never fault on an unmapped
        // adjacent page past the terminating NUL. Start with a 128-byte chunk
        // so short paths do not copy an entire 4 KiB page.
        let page_rem = page - (cur_addr & (page - 1));
        let max_chunk = if copied == 0 { 128 } else { page };
        let chunk_len = page_rem.min(max_chunk).min(buf.len() - copied);
        if use_vm {
            let local =
                iovec { iov_base: buf[copied..].as_mut_ptr().cast(), iov_len: chunk_len as u64 };
            let remote = iovec {
                iov_base: ptr::with_exposed_provenance_mut(cur_addr),
                iov_len: chunk_len as u64,
            };
            // SAFETY: `local` points into `buf[copied..copied+chunk_len]`.
            let rc = unsafe {
                syscall6(
                    Sysno::process_vm_readv as usize,
                    pid,
                    (&raw const local).addr(),
                    1,
                    (&raw const remote).addr(),
                    1,
                    0,
                )
            };
            if rc > 0 {
                let n = rc.cast_unsigned().min(chunk_len);
                // SAFETY: `process_vm_readv` initialized `buf[copied..copied + n]`.
                let written =
                    unsafe { slice::from_raw_parts(buf[copied..].as_ptr().cast::<u8>(), n) };
                if let Some(nul_pos) = written.iter().position(|&b| b == 0) {
                    // SAFETY: `buf[..copied + nul_pos]` has been initialized.
                    let out = unsafe {
                        slice::from_raw_parts(buf.as_ptr().cast::<u8>(), copied + nul_pos)
                    };
                    return Ok(Some(out));
                }
                copied += n;
                if n < chunk_len {
                    return Err(Error::FAULT);
                }
                continue;
            }
            if rc < 0 {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "Linux negative syscall errno fits in u32"
                )]
                let errno = (-rc).cast_unsigned() as u32;
                if errno != EPERM && errno != ENOSYS {
                    return Err(Error::FAULT);
                }
            } else {
                return Err(Error::FAULT);
            }
        }

        // Fallback when `process_vm_readv` is blocked by an outer sandbox: verify
        // that the current page is readable via `rt_sigprocmask(!0, ...)` before
        // reading bytes from it (note `cur_addr..cur_addr+chunk_len` stays
        // strictly within one page).
        if !probe_page_readable(cur_addr) {
            return Err(Error::FAULT);
        }
        let base = ptr::with_exposed_provenance::<u8>(cur_addr);
        for i in 0..chunk_len {
            // SAFETY: `cur_addr..cur_addr+chunk_len` lies within a single
            // kernel-probed readable page.
            let b = unsafe { base.add(i).read() };
            if b == 0 {
                // SAFETY: `buf[..copied + i]` has been initialized.
                let out = unsafe { slice::from_raw_parts(buf.as_ptr().cast::<u8>(), copied + i) };
                return Ok(Some(out));
            }
            buf[copied + i].write(b);
        }
        copied += chunk_len;
    }

    Ok(None)
}
