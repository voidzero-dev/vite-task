use fspy_nostd::{CWD, fs::AtFlags};
use fspy_shared_unix::open_flags::fails_without_reading;
use libc::FILE;

use crate::{
    client::{
        convert::{ModeStr, OpenFlags, PathAt},
        handle_open,
    },
    libc::{c_char, c_int},
    macros::intercept,
};

/// Records an open of `path` with `flags`, unless it fails without reading
/// the path.
///
/// # Safety
///
/// `path` must satisfy the contract of [`PathAt::borrow_raw`].
unsafe fn handle_open_at(path: PathAt<'_, '_>, flags: c_int) {
    let PathAt(dirfd, pathname) = path;
    let skipped = fails_without_reading(flags, |follow_symlinks| {
        let flags = if follow_symlinks { AtFlags::empty() } else { AtFlags::SYMLINK_NOFOLLOW };
        // The kernel ignores the descriptor for an absolute path, and a
        // relative path at `-1` isn't recorded anyway.
        fspy_nostd::fs::fstatat(dirfd.unwrap_or(CWD), pathname, flags)
            .ok()
            .map(|stat| stat.is_dir())
    });
    if !skipped {
        // SAFETY: path is valid, as the caller guarantees.
        unsafe { handle_open(path, OpenFlags(flags)) };
    }
}

const fn has_mode_arg(o_flags: c_int) -> bool {
    if o_flags & libc::O_CREAT != 0 {
        return true;
    }
    #[cfg(target_os = "linux")]
    if o_flags & libc::O_TMPFILE != 0 {
        return true;
    }
    false
}

#[cfg(not(target_os = "macos"))]
type Mode = libc::mode_t;
#[cfg(target_os = "macos")] // https://github.com/tailhook/openat/issues/21#issuecomment-535914957
type Mode = c_int;

intercept!(open(64): unsafe extern "C" fn(*const c_char, c_int, args: ...) -> c_int);
unsafe extern "C" fn open(path: *const c_char, flags: c_int, mut args: ...) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller of the interposed function
    unsafe { handle_open_at(PathAt(Some(CWD), fspy_nostd::CStr::from_ptr(path.cast())), flags) };
    if has_mode_arg(flags) {
        // SAFETY: when O_CREAT or O_TMPFILE is set, a mode_t argument is required by the open() contract
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc open() with the same arguments forwarded from the interposed function
        unsafe { open::original()(path, flags, mode) }
    } else {
        // SAFETY: calling the original libc open() with the same arguments forwarded from the interposed function
        unsafe { open::original()(path, flags) }
    }
}

intercept!(openat(64): unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int);
unsafe extern "C" fn openat(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    // SAFETY: dirfd is -1 or a valid file descriptor, and path is a valid C string, provided by the caller of the interposed function
    unsafe { handle_open_at(PathAt::borrow_raw(dirfd, path), flags) };

    if has_mode_arg(flags) {
        // https://github.com/tailhook/openat/issues/21#issuecomment-535914957
        // SAFETY: when O_CREAT or O_TMPFILE is set, a mode_t argument is required by the openat() contract
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc openat() with the same arguments forwarded from the interposed function
        unsafe { openat::original()(dirfd, path, flags, mode) }
    } else {
        // SAFETY: calling the original libc openat() with the same arguments forwarded from the interposed function
        unsafe { openat::original()(dirfd, path, flags) }
    }
}

#[cfg(target_os = "macos")]
intercept!(open_nocancel: unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int);
#[cfg(target_os = "macos")]
unsafe extern "C" fn open_nocancel(path: *const c_char, flags: c_int, mut args: ...) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller of open$NOCANCEL
    unsafe { handle_open_at(PathAt(Some(CWD), fspy_nostd::CStr::from_ptr(path.cast())), flags) };
    if has_mode_arg(flags) {
        // SAFETY: O_CREAT requires a mode argument, matching the open$NOCANCEL contract
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc open$NOCANCEL() with the same arguments forwarded from the interposed function
        unsafe { open_nocancel::original()(path, flags, mode) }
    } else {
        // SAFETY: calling the original libc open$NOCANCEL() with the same arguments forwarded from the interposed function
        unsafe { open_nocancel::original()(path, flags) }
    }
}

#[cfg(target_os = "macos")]
intercept!(openat_nocancel: unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int);
#[cfg(target_os = "macos")]
unsafe extern "C" fn openat_nocancel(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    // SAFETY: dirfd is -1 or a valid file descriptor, and path is a valid C string, provided by the caller of openat$NOCANCEL
    unsafe { handle_open_at(PathAt::borrow_raw(dirfd, path), flags) };
    if has_mode_arg(flags) {
        // SAFETY: O_CREAT requires a mode argument, matching the openat$NOCANCEL contract
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc openat$NOCANCEL() with the same arguments forwarded from the interposed function
        unsafe { openat_nocancel::original()(dirfd, path, flags, mode) }
    } else {
        // SAFETY: calling the original libc openat$NOCANCEL() with the same arguments forwarded from the interposed function
        unsafe { openat_nocancel::original()(dirfd, path, flags) }
    }
}

intercept!(fopen(64): unsafe extern "C" fn(path: *const c_char, mode: *const c_char) -> *mut FILE);
unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    // SAFETY: path and mode are valid C string pointers provided by the caller of the interposed function
    unsafe { handle_open(fspy_nostd::CStr::from_ptr(path.cast()), ModeStr(mode)) };
    // SAFETY: calling the original libc fopen() with the same arguments forwarded from the interposed function
    unsafe { fopen::original()(path, mode) }
}

intercept!(freopen(64): unsafe extern "C" fn(path: *const c_char, mode: *const c_char, stream: *mut FILE) -> *mut FILE);
unsafe extern "C" fn freopen(
    path: *const c_char,
    mode: *const c_char,
    stream: *mut FILE,
) -> *mut FILE {
    // SAFETY: path and mode are valid C string pointers provided by the caller of the interposed function
    unsafe { handle_open(fspy_nostd::CStr::from_ptr(path.cast()), ModeStr(mode)) };
    // SAFETY: calling the original libc freopen() with the same arguments forwarded from the interposed function
    unsafe { freopen::original()(path, mode, stream) }
}
