use core::{mem::MaybeUninit, slice};

use rustix::{
    fd::{AsFd as _, AsRawFd as _, FromRawFd as _, OwnedFd},
    fs::{Mode, OFlags},
};

use crate::{BorrowedFd, CStr, CWD, Error, Fat, Result, Thin};

pub(super) const PATH_MAX: usize = libc::PATH_MAX as usize;

#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
fn openat<R>(
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, R>,
    flags: OFlags,
    mode: Mode,
) -> Result<OwnedFd> {
    // SAFETY: `dirfd` remains borrowed, `path` is NUL-terminated for the call,
    // and variadic arguments use their promoted C types.
    let fd = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            path.as_ptr().cast(),
            flags.bits().cast_signed(),
            libc::c_uint::from(mode.bits()),
        )
    };
    if fd == -1 {
        // SAFETY: libSystem stored this call's error before returning -1.
        return Err(Error::from_raw_os_error(unsafe { *libc::__error() }));
    }

    // SAFETY: ownership of the newly opened descriptor transfers here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Gets the path associated with `fd`.
///
/// `F_GETPATH` writes a NUL-terminated path into its `MAXPATHLEN` buffer but
/// does not report the length, so this function returns [`CStr<Thin>`] without
/// scanning for the terminator. The returned C string borrows `buf` and starts
/// at the same address as `buf`.
///
/// This function performs one `fcntl` call and does not retry. `F_GETPATH`
/// accepts no buffer length and always uses its fixed `MAXPATHLEN` storage.
///
/// # Errors
///
/// Returns the error reported by `fcntl`.
pub fn fcntl_getpath<'buf>(
    fd: BorrowedFd<'_>,
    buf: &'buf mut [MaybeUninit<u8>; PATH_MAX],
) -> Result<CStr<'buf, Thin>> {
    // SAFETY: `fd` remains borrowed and `buf` has the `MAXPATHLEN` storage
    // required by `F_GETPATH`.
    let result = unsafe {
        libc::fcntl(fd.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr().cast::<libc::c_char>())
    };
    if result == -1 {
        // SAFETY: libSystem stored this call's error before returning -1.
        return Err(Error::from_raw_os_error(unsafe { *libc::__error() }));
    }

    // SAFETY: `F_GETPATH` wrote a NUL-terminated pathname into `buf`.
    Ok(unsafe { CStr::from_ptr(buf.as_ptr().cast()) })
}

pub(super) fn getcwd(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    let (chunks, remainder) = buf.as_chunks_mut::<PATH_MAX>();
    let Some(full) = chunks.first_mut() else {
        return getcwd_small(remainder);
    };
    getcwd_full(full)
}

// Keep the `PATH_MAX` scratch storage in a separate stack frame so the
// large-buffer path does not reserve it. `inline(never)` preserves that
// conditional stack allocation after optimization.
#[inline(never)]
fn getcwd_small(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    let mut scratch = [MaybeUninit::uninit(); PATH_MAX];
    let initialized = getcwd_full(&mut scratch)?.len_with_nul();
    let buf = buf.get_mut(..initialized).ok_or(Error::RANGE)?;

    buf.copy_from_slice(&scratch[..initialized]);
    // SAFETY: `getcwd_full` initialized this copied C string prefix.
    let bytes = unsafe { slice::from_raw_parts(buf.as_ptr().cast(), buf.len()) };
    // SAFETY: upheld by the initialized prefix above.
    Ok(unsafe { CStr::from_units_with_nul_unchecked(bytes) })
}

/// The allocation-free fast path from Apple's [`getcwd`]: ask an open `.`
/// descriptor for its path.
///
/// libSystem additionally `stat`s that path and compares it against the
/// descriptor, falling back to walking up through `..` when they disagree —
/// `F_GETPATH` can name a directory that has since been renamed or removed.
/// This wrapper deliberately stops short of both. It has no slow path to fall
/// back to, so the comparison could only turn a slightly stale answer into a
/// hard error, and its caller records paths that a process accessed rather
/// than resolving them: a stale name is a cosmetic inaccuracy, an error is a
/// lost access. Skipping it also keeps this to two syscalls on the most
/// common resolution there is.
///
/// [`getcwd`]: https://github.com/apple-oss-distributions/Libc/blob/Libc-1752.120.2/gen/FreeBSD/getcwd.c#L62-L138
fn getcwd_full(buf: &mut [MaybeUninit<u8>; PATH_MAX]) -> Result<CStr<'_, Fat>> {
    // SAFETY: the byte string contains one trailing NUL.
    let dot_path = unsafe { CStr::<Fat>::from_units_with_nul_unchecked(b".\0") };
    let fd = openat(CWD, dot_path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;

    let path = fcntl_getpath(fd.as_fd(), buf)?;
    drop(fd);

    // `F_GETPATH` returns no length, so count the result before handing it
    // back with its length retained.
    Ok(path.count())
}
