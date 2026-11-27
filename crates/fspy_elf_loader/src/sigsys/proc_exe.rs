//! `/proc/self/exe` virtualization for the `SIGSYS` dispatcher.
//!
//! Because the kernel execs the freestanding launcher instead of the target
//! ELF, `/proc/self/exe`, `/proc/thread-self/exe`, `/proc/<pid>/exe`, and
//! `/proc/<pid>/task/<tid>/exe` all resolve to the launcher binary.
//!
//! This module handles:
//! - `readlink` / `readlinkat`: returns the target executable's path without a
//!   trailing NUL, matching Linux truncation and `bufsiz <= 0` (`-EINVAL`)
//!   semantics.
//! - Path-following syscalls (`open`, `openat`, `openat2`, `stat`,
//!   `newfstatat`/`fstatat`, `statx`, `access`, `faccessat`, `faccessat2`,
//!   `execve`, `execveat`): rewrites matching paths (including relative lookups
//!   via a `/proc/...` `dirfd` or `cwd`) in place in [`SyscallInvocation`] to
//!   the permanent target executable path, honoring `O_NOFOLLOW`,
//!   `AT_SYMLINK_NOFOLLOW`, and `openat2` resolve flags.

use core::{
    mem::{MaybeUninit, size_of},
    ptr,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use allocator_api2::{alloc::Allocator, vec::Vec};
use fspy_nostd::{
    BorrowedFd, CStr, CWD, Error, Result, Thin,
    fs::{AccessFlags, PATH_MAX, faccessat},
};
use linux_raw_sys::{
    errno::{EFAULT, EINVAL},
    general::{
        AT_FDCWD, AT_SYMLINK_NOFOLLOW, O_NOFOLLOW, RESOLVE_BENEATH, RESOLVE_IN_ROOT,
        RESOLVE_NO_MAGICLINKS, RESOLVE_NO_SYMLINKS, RESOLVE_NO_XDEV, open_how,
    },
};
use syscalls::Sysno;

use super::{
    arch::{SyscallInvocation, arg_i32, getpid, gettid, neg_errno},
    usermem,
};

/// Permanently retained pointer to the NUL-terminated target executable path.
static EXE_PTR: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
/// Byte length of the target executable path, excluding its terminating NUL.
static EXE_LEN: AtomicUsize = AtomicUsize::new(0);

/// Stores the target executable path in permanent page-backed storage.
///
/// Must be called once during `sigsys::install` before entering the target.
///
/// # Errors
///
/// Returns [`Error::INVAL`] if `exe_path` is empty or contains an interior NUL,
/// or [`Error::NOMEM`] if storage allocation fails.
pub fn init(exe_path: &[u8]) -> Result<()> {
    if exe_path.is_empty() || exe_path.contains(&0) {
        return Err(Error::INVAL);
    }
    let bump = fspy_nostd_alloc::page_bump();
    let mut owned = Vec::new_in(&bump);
    owned.try_reserve_exact(exe_path.len() + 1).map_err(|_| Error::NOMEM)?;
    owned.extend_from_slice(exe_path);
    owned.push(0);
    let ptr = owned.as_mut_ptr();
    // Retain the permanent mapping for the lifetime of the process.
    core::mem::forget(owned);
    core::mem::forget(bump);
    EXE_PTR.store(ptr, Ordering::Release);
    EXE_LEN.store(exe_path.len(), Ordering::Release);
    Ok(())
}

/// Resolves the canonical path of an already-opened executable `fd` via
/// [`fspy_nostd_alloc::fs::fd_path`], falling back to `fallback` (made absolute
/// with `getcwd` if relative) when `/proc` is unavailable.
///
/// # Errors
///
/// Returns [`Error::NOMEM`] if temporary allocation fails.
pub fn resolve_exe_path_for_fd<A: Allocator + Copy>(
    alloc: A,
    fd: BorrowedFd<'_>,
    fallback: CStr<'_, Thin>,
) -> Result<Vec<u8, A>> {
    if let Ok(Some(resolved)) = fspy_nostd_alloc::fs::fd_path(alloc, fd)
        && resolved.as_units().starts_with(b"/")
    {
        return Ok(resolved.into_units());
    }

    let fallback_bytes = fallback.count().as_units();
    if fallback_bytes.starts_with(b"/") {
        let mut out = Vec::new_in(alloc);
        out.try_reserve_exact(fallback_bytes.len()).map_err(|_| Error::NOMEM)?;
        out.extend_from_slice(fallback_bytes);
        return Ok(out);
    }

    if let Ok(cwd) = fspy_nostd_alloc::fs::getcwd(alloc) {
        let cwd_bytes = cwd.as_units();
        let mut joined = Vec::new_in(alloc);
        let needed = cwd_bytes.len() + 1 + fallback_bytes.len();
        joined.try_reserve_exact(needed).map_err(|_| Error::NOMEM)?;
        joined.extend_from_slice(cwd_bytes);
        if !cwd_bytes.ends_with(b"/") {
            joined.push(b'/');
        }
        joined.extend_from_slice(fallback_bytes);
        let mut normalized = Vec::new_in(alloc);
        if normalize_abs_path(&joined, &mut normalized) {
            return Ok(normalized);
        }
    }

    let mut out = Vec::new_in(alloc);
    out.try_reserve_exact(fallback_bytes.len()).map_err(|_| Error::NOMEM)?;
    out.extend_from_slice(fallback_bytes);
    Ok(out)
}

fn target_exe() -> Option<(*const u8, usize)> {
    let len = EXE_LEN.load(Ordering::Acquire);
    if len == 0 {
        return None;
    }
    let ptr = EXE_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        return None;
    }
    Some((ptr.cast_const(), len))
}

/// Handles `readlink(path, buf, bufsiz)` (x86-64) and
/// `readlinkat(dirfd, path, buf, bufsiz)` (both arches) when `(dirfd, path_ptr)`
/// targets `/proc/*/exe`.
///
/// Returns `None` when the path does not match `/proc/*/exe` so the dispatcher
/// can invoke any registered [`crate::SyscallInterceptor`] and replay the call.
///
/// # Safety
///
/// Arguments must come from a trapped `readlink` or `readlinkat` syscall.
pub unsafe fn try_handle_readlink(
    dirfd: i32,
    path_ptr: usize,
    buf_ptr: usize,
    bufsiz_raw: usize,
) -> Option<isize> {
    let (exe_ptr, exe_len) = target_exe()?;
    // SAFETY: Inspects user path pointer up to `PATH_MAX` bytes.
    if !unsafe { is_proc_self_exe(dirfd, path_ptr) } {
        return None;
    }

    // Linux `do_readlinkat` takes `int bufsiz` and rejects `<= 0` with `-EINVAL`.
    let bufsiz = arg_i32(bufsiz_raw);
    if bufsiz <= 0 {
        return Some(neg_errno(EINVAL));
    }
    if buf_ptr == 0 {
        return Some(neg_errno(EFAULT));
    }

    let copy_len = exe_len.min(bufsiz.cast_unsigned() as usize);
    // SAFETY: `exe_ptr` is permanently retained and readable for `exe_len` bytes.
    let src = unsafe { core::slice::from_raw_parts(exe_ptr, copy_len) };
    if usermem::write_user_bytes(buf_ptr, src).is_err() {
        return Some(neg_errno(EFAULT));
    }
    Some(copy_len.cast_signed())
}

/// Path and `open_how` values already fault-safely copied while inspecting a
/// trapped syscall for `/proc/*/exe` rewriting, reused by the caller's
/// [`crate::SyscallInterceptor`] and unmasked-replay check to avoid duplicate
/// `process_vm_readv` round trips on the hot path.
pub struct Prefetched<'a> {
    pub path: Option<(usize, &'a [u8])>,
    pub open_how: Option<open_how>,
}

/// Rewrites `/proc/*/exe` path arguments in `inv` in place to the permanent
/// target executable path for path-following syscalls, returning any
/// already-copied path bytes (post-rewrite if rewritten) and `open_how` value.
///
/// # Safety
///
/// `inv` must contain the arguments from a trapped syscall matching `sysno`.
pub unsafe fn rewrite_path_arg<'a>(
    inv: &mut SyscallInvocation,
    sysno: Sysno,
    path_buf: &'a mut [MaybeUninit<u8>; PATH_MAX],
) -> Prefetched<'a> {
    let Some((exe_ptr, exe_len)) = target_exe() else {
        return Prefetched { path: None, open_how: None };
    };

    // SAFETY: All helpers inspect user pointers according to the kernel ABI for
    // `sysno` and only replace arguments with the permanent `exe_ptr` / `AT_FDCWD`.
    unsafe {
        match sysno {
            Sysno::openat => Prefetched {
                path: rewrite_open(inv, exe_ptr, exe_len, Some(0), 1, 2, path_buf),
                open_how: None,
            },
            Sysno::openat2 => rewrite_openat2(inv, exe_ptr, exe_len, path_buf),
            #[cfg(target_arch = "x86_64")]
            Sysno::open => Prefetched {
                path: rewrite_open(inv, exe_ptr, exe_len, None, 0, 1, path_buf),
                open_how: None,
            },
            #[cfg(target_arch = "x86_64")]
            Sysno::newfstatat => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, Some(0), 1, Some(3), path_buf),
                open_how: None,
            },
            #[cfg(target_arch = "aarch64")]
            Sysno::fstatat => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, Some(0), 1, Some(3), path_buf),
                open_how: None,
            },
            Sysno::statx => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, Some(0), 1, Some(2), path_buf),
                open_how: None,
            },
            Sysno::faccessat => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, Some(0), 1, None, path_buf),
                open_how: None,
            },
            Sysno::faccessat2 => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, Some(0), 1, Some(3), path_buf),
                open_how: None,
            },
            #[cfg(target_arch = "x86_64")]
            Sysno::stat | Sysno::access => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, None, 0, None, path_buf),
                open_how: None,
            },
            Sysno::execve => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, None, 0, None, path_buf),
                open_how: None,
            },
            Sysno::execveat => Prefetched {
                path: rewrite_at_follow(inv, exe_ptr, exe_len, Some(0), 1, Some(4), path_buf),
                open_how: None,
            },
            _ => Prefetched { path: None, open_how: None },
        }
    }
}

#[expect(clippy::cast_possible_truncation, reason = "Linux open flags are u32")]
unsafe fn rewrite_open<'a>(
    inv: &mut SyscallInvocation,
    exe_ptr: *const u8,
    exe_len: usize,
    dirfd_idx: Option<usize>,
    path_idx: usize,
    flags_idx: usize,
    path_buf: &'a mut [MaybeUninit<u8>; PATH_MAX],
) -> Option<(usize, &'a [u8])> {
    let flags = inv.args[flags_idx] as u32;
    // Preserve symlink semantics for `O_NOFOLLOW` (fails with ELOOP, or opens
    // the symlink itself with `O_PATH | O_NOFOLLOW`).
    if (flags & O_NOFOLLOW) != 0 {
        return None;
    }
    let dirfd = dirfd_idx.map_or(AT_FDCWD, |idx| arg_i32(inv.args[idx]));
    let path_ptr = inv.args[path_idx];
    // SAFETY: Inspects user path pointer up to `PATH_MAX` bytes.
    let (matched, copied) = unsafe { inspect_proc_self_exe(dirfd, path_ptr, path_buf) };
    if !matched {
        return copied.map(|bytes| (path_ptr, bytes));
    }
    if let Some(idx) = dirfd_idx {
        inv.args[idx] = AT_FDCWD.cast_unsigned() as usize;
    }
    inv.args[path_idx] = exe_ptr.addr();
    // SAFETY: `exe_ptr` is permanently retained for `exe_len` bytes.
    Some((exe_ptr.addr(), unsafe { core::slice::from_raw_parts(exe_ptr, exe_len) }))
}

unsafe fn rewrite_openat2<'a>(
    inv: &mut SyscallInvocation,
    exe_ptr: *const u8,
    exe_len: usize,
    path_buf: &'a mut [MaybeUninit<u8>; PATH_MAX],
) -> Prefetched<'a> {
    let dirfd = arg_i32(inv.args[0]);
    let path_ptr = inv.args[1];
    let how_ptr = inv.args[2];
    let size = inv.args[3];

    if how_ptr == 0 || size < size_of::<open_how>() {
        return Prefetched { path: None, open_how: None };
    }
    // SAFETY: `open_how` is a plain `#[repr(C)]` struct of `u64` fields.
    let Ok(how) = (unsafe { usermem::read_user::<open_how>(how_ptr) }) else {
        return Prefetched { path: None, open_how: None };
    };
    let restricted_resolve = u64::from(
        RESOLVE_NO_SYMLINKS
            | RESOLVE_NO_MAGICLINKS
            | RESOLVE_BENEATH
            | RESOLVE_IN_ROOT
            | RESOLVE_NO_XDEV,
    );
    if (how.flags & u64::from(O_NOFOLLOW)) != 0 || (how.resolve & restricted_resolve) != 0 {
        return Prefetched { path: None, open_how: Some(how) };
    }
    // SAFETY: Inspects user path pointer up to `PATH_MAX` bytes.
    let (matched, copied) = unsafe { inspect_proc_self_exe(dirfd, path_ptr, path_buf) };
    if !matched {
        return Prefetched { path: copied.map(|bytes| (path_ptr, bytes)), open_how: Some(how) };
    }
    inv.args[0] = AT_FDCWD.cast_unsigned() as usize;
    inv.args[1] = exe_ptr.addr();
    // SAFETY: `exe_ptr` is permanently retained for `exe_len` bytes.
    let exe_bytes = unsafe { core::slice::from_raw_parts(exe_ptr, exe_len) };
    Prefetched { path: Some((exe_ptr.addr(), exe_bytes)), open_how: Some(how) }
}

#[expect(clippy::cast_possible_truncation, reason = "Linux dirfd/flags are 32-bit")]
unsafe fn rewrite_at_follow<'a>(
    inv: &mut SyscallInvocation,
    exe_ptr: *const u8,
    exe_len: usize,
    dirfd_idx: Option<usize>,
    path_idx: usize,
    flags_idx: Option<usize>,
    path_buf: &'a mut [MaybeUninit<u8>; PATH_MAX],
) -> Option<(usize, &'a [u8])> {
    if let Some(idx) = flags_idx
        && ((inv.args[idx] as u32) & AT_SYMLINK_NOFOLLOW) != 0
    {
        // Do not rewrite no-follow queries: `/proc/self/exe` must remain a
        // symlink (`S_IFLNK`) so `lstat` and `realpath` follow it via `readlink`.
        return None;
    }
    let dirfd = dirfd_idx.map_or(AT_FDCWD, |idx| arg_i32(inv.args[idx]));
    let path_ptr = inv.args[path_idx];
    // SAFETY: Inspects user path pointer up to `PATH_MAX` bytes.
    let (matched, copied) = unsafe { inspect_proc_self_exe(dirfd, path_ptr, path_buf) };
    if !matched {
        return copied.map(|bytes| (path_ptr, bytes));
    }
    if let Some(idx) = dirfd_idx {
        inv.args[idx] = AT_FDCWD.cast_unsigned() as usize;
    }
    inv.args[path_idx] = exe_ptr.addr();
    // SAFETY: `exe_ptr` is permanently retained for `exe_len` bytes.
    Some((exe_ptr.addr(), unsafe { core::slice::from_raw_parts(exe_ptr, exe_len) }))
}

/// Checks whether `(dirfd, path_ptr)` refers to the current process's
/// `/proc/.../exe` magic symlink.
///
/// # Safety
///
/// `path_ptr` must be null or point to a readable NUL-terminated user string.
unsafe fn is_proc_self_exe(dirfd: i32, path_ptr: usize) -> bool {
    let mut stack_buf = [MaybeUninit::<u8>::uninit(); PATH_MAX];
    // SAFETY: delegates to `inspect_proc_self_exe` with a local `PATH_MAX` buffer.
    unsafe { inspect_proc_self_exe(dirfd, path_ptr, &mut stack_buf).0 }
}

/// Fault-safely reads `path_ptr` into `stack_buf` once, returning
/// `(matches_proc_exe, copied_bytes)` so non-matching callers can reuse the
/// already-copied bytes without a second `process_vm_readv`.
///
/// # Safety
///
/// `path_ptr` must be a user pointer from a trapped syscall.
unsafe fn inspect_proc_self_exe(
    dirfd: i32,
    path_ptr: usize,
    stack_buf: &mut [MaybeUninit<u8>; PATH_MAX],
) -> (bool, Option<&[u8]>) {
    let Ok(Some(raw)) = usermem::read_user_cstr(path_ptr, stack_buf) else {
        return (false, None);
    };
    // Fast rejection for 99.99% of paths: any path naming `/proc/.../exe`
    // (without a trailing `/`, which is ENOTDIR on a regular-file symlink)
    // must end with the component `exe`.
    if !ends_with_exe_component(raw) {
        return (false, Some(raw));
    }

    let pid = getpid();
    let tid = gettid();

    if raw.starts_with(b"/") {
        // Fast path for exact canonical forms without allocating.
        let matched = if !raw.contains(&b'.') && !raw.windows(2).any(|w| w == b"//") {
            match_normalized_proc_exe(raw, pid, tid, task_dir_exists)
        } else {
            let bump = fspy_nostd_alloc::pooled_bump();
            let mut normalized = Vec::new_in(&bump);
            normalize_abs_path(raw, &mut normalized)
                && match_normalized_proc_exe(&normalized, pid, tid, task_dir_exists)
        };
        return (matched, Some(raw));
    }

    // Relative path ending in `exe`: resolve `dirfd` (or `cwd` for `AT_FDCWD`)
    // via `fspy_nostd_alloc::fs::fd_path` using a pooled bump arena so stack
    // usage stays tiny.
    // SAFETY: `dirfd` comes from the trapped syscall; `-1` yields `None`, and
    // any invalid fd cleanly returns `Ok(None)` from `fd_path`.
    let Some(borrowed_dirfd) = (unsafe { BorrowedFd::try_borrow_raw(dirfd) }) else {
        return (false, Some(raw));
    };
    let bump = fspy_nostd_alloc::pooled_bump();
    let Ok(Some(base)) = fspy_nostd_alloc::fs::fd_path(&bump, borrowed_dirfd) else {
        return (false, Some(raw));
    };
    let base_bytes = base.as_units();
    if !base_bytes.starts_with(b"/") {
        return (false, Some(raw));
    }

    let mut joined = Vec::new_in(&bump);
    if joined.try_reserve_exact(base_bytes.len() + 1 + raw.len()).is_err() {
        return (false, Some(raw));
    }
    joined.extend_from_slice(base_bytes);
    joined.push(b'/');
    joined.extend_from_slice(raw);

    let mut normalized = Vec::new_in(&bump);
    let matched = normalize_abs_path(&joined, &mut normalized)
        && match_normalized_proc_exe(&normalized, pid, tid, task_dir_exists);
    (matched, Some(raw))
}

fn ends_with_exe_component(path: &[u8]) -> bool {
    path == b"exe" || path.ends_with(b"/exe")
}

/// Lexically normalizes an absolute path (`path` starts with `b'/'`), resolving
/// redundant slashes, `.`, and `..`.
fn normalize_abs_path<A: Allocator>(path: &[u8], out: &mut Vec<u8, A>) -> bool {
    if !path.starts_with(b"/") {
        return false;
    }
    out.clear();
    if out.try_reserve(path.len().max(1)).is_err() {
        return false;
    }
    for segment in path.split(|&b| b == b'/') {
        match segment {
            b"" | b"." => {}
            b".." => {
                while let Some(byte) = out.pop() {
                    if byte == b'/' {
                        break;
                    }
                }
            }
            seg => {
                out.push(b'/');
                out.extend_from_slice(seg);
            }
        }
    }
    if out.is_empty() {
        out.push(b'/');
    }
    true
}

/// Matches a lexically normalized absolute path against:
/// - `/proc/self/exe`
/// - `/proc/thread-self/exe`
/// - `/proc/<pid_or_tid>/exe`
/// - `/proc/<pid_or_self>/task/<tid>/exe`
fn match_normalized_proc_exe(
    norm: &[u8],
    pid: i32,
    tid: i32,
    task_exists: impl FnOnce(i32, i32) -> bool,
) -> bool {
    let Some(rest) = norm.strip_prefix(b"/proc/") else {
        return false;
    };
    let Some(prefix) = rest.strip_suffix(b"/exe") else {
        return false;
    };
    if prefix == b"self" || prefix == b"thread-self" {
        return true;
    }
    if let Some(id) = parse_positive_i32(prefix) {
        return id == pid || id == tid || task_exists(pid, id);
    }
    // Check `<pid_or_self>/task/<task_tid>`
    let Some(slash) = prefix.iter().position(|&b| b == b'/') else {
        return false;
    };
    let (proc_id, after_proc) = prefix.split_at(slash);
    let Some(task_tid_bytes) = after_proc.strip_prefix(b"/task/") else {
        return false;
    };
    let proc_matches = proc_id == b"self" || parse_positive_i32(proc_id) == Some(pid);
    if !proc_matches {
        return false;
    }
    let Some(task_tid) = parse_positive_i32(task_tid_bytes) else {
        return false;
    };
    task_tid == pid || task_tid == tid || task_exists(pid, task_tid)
}

fn parse_positive_i32(bytes: &[u8]) -> Option<i32> {
    if bytes.is_empty() || bytes[0] == b'0' {
        return None;
    }
    let mut value = 0_i32;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(i32::from(b - b'0'))?;
    }
    (value > 0).then_some(value)
}

fn write_u32_decimal(mut value: u32, buf: &mut [u8], start: usize) -> Option<usize> {
    let mut digits = [0_u8; 10];
    let mut count = 0;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    if start + count > buf.len() {
        return None;
    }
    for i in 0..count {
        buf[start + i] = digits[count - 1 - i];
    }
    Some(start + count)
}

fn task_dir_exists(pid: i32, task_tid: i32) -> bool {
    if pid <= 0 || task_tid <= 0 {
        return false;
    }
    let mut buf = [0_u8; 64];
    let prefix = b"/proc/";
    let mut pos = prefix.len();
    buf[..pos].copy_from_slice(prefix);
    let Some(next) = write_u32_decimal(pid.cast_unsigned(), &mut buf, pos) else {
        return false;
    };
    pos = next;
    let middle = b"/task/";
    if pos + middle.len() >= buf.len() {
        return false;
    }
    buf[pos..pos + middle.len()].copy_from_slice(middle);
    pos += middle.len();
    let Some(next) = write_u32_decimal(task_tid.cast_unsigned(), &mut buf, pos) else {
        return false;
    };
    pos = next;
    if pos >= buf.len() {
        return false;
    }
    buf[pos] = 0;

    let Some(path) = CStr::from_units_with_nul(&buf[..=pos]) else {
        return false;
    };
    faccessat(CWD, path, AccessFlags::empty()).is_ok()
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    fn normalize(input: &[u8]) -> std::vec::Vec<u8> {
        let mut out = Vec::new_in(Global);
        assert!(normalize_abs_path(input, &mut out));
        out.as_slice().to_vec()
    }

    #[test]
    fn normalizes_slashes_dot_and_dotdot() {
        assert_eq!(normalize(b"/proc/self/exe"), b"/proc/self/exe");
        assert_eq!(normalize(b"//proc///self/./exe"), b"/proc/self/exe");
        assert_eq!(normalize(b"/proc/123/../self/exe"), b"/proc/self/exe");
        assert_eq!(normalize(b"/proc/self/task/99/../../exe"), b"/proc/self/exe");
        assert_eq!(normalize(b"/../../../proc/self/exe"), b"/proc/self/exe");
        assert_eq!(normalize(b"/"), b"/");
    }

    #[test]
    fn matches_expected_proc_exe_variants() {
        let pid = 1000;
        let tid = 1005;
        let sibling_tid = 1009;
        let exists = |p: i32, t: i32| p == pid && t == sibling_tid;

        assert!(match_normalized_proc_exe(b"/proc/self/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/thread-self/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/1000/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/1005/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/1009/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/1000/task/1000/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/1000/task/1005/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/1000/task/1009/exe", pid, tid, exists));
        assert!(match_normalized_proc_exe(b"/proc/self/task/1009/exe", pid, tid, exists));

        // Non-matching paths.
        assert!(!match_normalized_proc_exe(b"/proc/1/exe", pid, tid, exists));
        assert!(!match_normalized_proc_exe(b"/proc/01000/exe", pid, tid, exists));
        assert!(!match_normalized_proc_exe(b"/proc/1000/task/9999/exe", pid, tid, exists));
        assert!(!match_normalized_proc_exe(b"/proc/9999/task/1005/exe", pid, tid, exists));
        assert!(!match_normalized_proc_exe(b"/proc/self/cmdline", pid, tid, exists));
        assert!(!match_normalized_proc_exe(b"/tmp/proc/self/exe", pid, tid, exists));
        assert!(!match_normalized_proc_exe(b"/proc/self/exe/extra", pid, tid, exists));
    }

    #[test]
    fn fast_exe_suffix_filter() {
        assert!(ends_with_exe_component(b"exe"));
        assert!(ends_with_exe_component(b"/proc/self/exe"));
        assert!(ends_with_exe_component(b"./exe"));
        assert!(!ends_with_exe_component(b""));
        assert!(!ends_with_exe_component(b"/proc/self/exe/"));
        assert!(!ends_with_exe_component(b"/usr/bin/myexe"));
        assert!(!ends_with_exe_component(b"/etc/ld.so.cache"));
    }
}
