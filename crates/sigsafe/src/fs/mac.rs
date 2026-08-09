use core::{mem::MaybeUninit, slice};

use rustix::{
    fd::{AsFd as _, AsRawFd as _, FromRawFd as _, OwnedFd},
    fs::{Mode, OFlags, fstat, stat},
};

use crate::{BorrowedFd, CStr, Errno, Fat, Result, Thin};

pub(super) const PATH_MAX: usize = libc::PATH_MAX as usize;

#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
fn open<R>(path: CStr<'_, R>, flags: OFlags, mode: Mode) -> Result<OwnedFd> {
    // SAFETY: `path` is NUL-terminated for the call. Variadic arguments use
    // their promoted C types.
    let fd = unsafe {
        libc::open(path.as_ptr(), flags.bits().cast_signed(), libc::c_uint::from(mode.bits()))
    };
    if fd == -1 {
        // SAFETY: libSystem stored this call's error before returning -1.
        return Err(Errno::from_raw_os_error(unsafe { *libc::__error() }));
    }

    // SAFETY: ownership of the newly opened descriptor transfers here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Gets the path associated with `fd`.
///
/// `F_GETPATH` writes a NUL-terminated path into its `MAXPATHLEN` buffer but
/// does not report the length, so this function returns [`CStr<Thin>`] without
/// scanning for the terminator.
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
        return Err(Errno::from_raw_os_error(unsafe { *libc::__error() }));
    }

    // SAFETY: `F_GETPATH` wrote a NUL-terminated pathname into `buf`.
    Ok(unsafe { CStr::from_ptr(buf.as_ptr().cast()) })
}

pub(super) fn getcwd(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    if buf.len() < PATH_MAX {
        return getcwd_small(buf);
    }

    getcwd_full(buf)
}

// Keep the `PATH_MAX` scratch storage in a separate stack frame so the
// large-buffer path does not reserve it. `inline(never)` preserves that
// conditional stack allocation after optimization.
#[inline(never)]
fn getcwd_small(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    let mut scratch = [MaybeUninit::uninit(); PATH_MAX];
    let initialized = getcwd_full(&mut scratch)?.len_with_nul();
    if initialized > buf.len() {
        return Err(Errno::RANGE);
    }

    buf[..initialized].copy_from_slice(&scratch[..initialized]);
    // SAFETY: `getcwd_full` initialized this copied C string prefix.
    let bytes = unsafe { slice::from_raw_parts(buf.as_ptr().cast(), initialized) };
    // SAFETY: upheld by the initialized prefix above.
    Ok(unsafe { CStr::from_bytes_with_nul_unchecked(bytes) })
}

/// The allocation-free fast path from Apple's [`getcwd`]: obtain the path
/// of an open `.` descriptor, then verify that it still names `.`.
///
/// [`getcwd`]: https://github.com/apple-oss-distributions/Libc/blob/Libc-1752.120.2/gen/FreeBSD/getcwd.c#L62-L138
fn getcwd_full(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    let Some(buf) = buf.first_chunk_mut::<PATH_MAX>() else {
        return Err(Errno::RANGE);
    };

    // SAFETY: the byte string contains one trailing NUL.
    let dot_path = unsafe { CStr::<Fat>::from_bytes_with_nul_unchecked(b".\0") };
    let fd = open(dot_path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    let dot = fstat(&fd)?;
    if dot.st_dev == 0 || dot.st_ino == 0 {
        return Err(Errno::INVAL);
    }

    let path = fcntl_getpath(fd.as_fd(), buf)?;
    drop(fd);

    // `F_GETPATH` returns no length, so `getcwd` counts the result just as
    // libSystem's scratch-buffer path does before checking and copying it.
    let path = path.count();

    let pointed_to = stat(path.as_core_c_str())?;
    if dot.st_dev != pointed_to.st_dev || dot.st_ino != pointed_to.st_ino {
        return Err(Errno::INVAL);
    }

    Ok(path)
}
