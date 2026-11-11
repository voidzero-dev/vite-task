use core::{mem::MaybeUninit, slice};

use crate::{
    BorrowedFd, CStr, CWD, Error, Fat, OwnedFd, Result, Thin,
    fs::{AtFlags, Mode, OFlags, Stat},
};

// Darwin UAPI `MAXPATHLEN`.
pub(super) const PATH_MAX: usize = 1024;
const _: () = assert!(libc::PATH_MAX == 1024);
pub(super) const O_RDONLY: u32 = libc::O_RDONLY.cast_unsigned();
pub(super) const O_RDWR: u32 = libc::O_RDWR.cast_unsigned();
pub(super) const O_CREAT: u32 = libc::O_CREAT.cast_unsigned();
pub(super) const O_EXCL: u32 = libc::O_EXCL.cast_unsigned();
pub(super) const O_CLOEXEC: u32 = libc::O_CLOEXEC.cast_unsigned();
pub(super) const AT_REMOVEDIR: u32 = libc::AT_REMOVEDIR.cast_unsigned();
pub(super) type ModeBits = libc::mode_t;
pub(super) const S_IRUSR: ModeBits = libc::S_IRUSR;
pub(super) const S_IWUSR: ModeBits = libc::S_IWUSR;

#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub(super) fn openat<R>(
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
        return Err(Error::last_os_error());
    }

    // SAFETY: ownership of the newly opened descriptor transfers here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub(super) fn unlinkat<R>(dirfd: BorrowedFd<'_>, path: CStr<'_, R>, flags: AtFlags) -> Result<()> {
    // SAFETY: `dirfd` remains borrowed and `path` is NUL-terminated for the
    // call.
    let result = unsafe {
        libc::unlinkat(dirfd.as_raw_fd(), path.as_ptr().cast(), flags.bits().cast_signed())
    };
    if result == -1 { Err(Error::last_os_error()) } else { Ok(()) }
}

pub(super) fn fstat(fd: BorrowedFd<'_>) -> Result<Stat> {
    let mut raw = core::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `fd` remains borrowed and `raw` is writable for the call.
    if unsafe { libc::fstat(fd.as_raw_fd(), raw.as_mut_ptr()) } == -1 {
        return Err(Error::last_os_error());
    }
    // SAFETY: a successful `fstat` initialized the complete structure.
    let raw = unsafe { raw.assume_init() };
    Ok(Stat { st_size: raw.st_size })
}

pub(super) fn ftruncate(fd: BorrowedFd<'_>, len: u64) -> Result<()> {
    let len = i64::try_from(len).map_err(|_| Error::OVERFLOW)?;
    // SAFETY: `fd` remains borrowed and the length is passed by value.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len) } == -1 {
        Err(Error::last_os_error())
    } else {
        Ok(())
    }
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
        return Err(Error::last_os_error());
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
