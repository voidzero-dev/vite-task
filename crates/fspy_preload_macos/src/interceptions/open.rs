use libc::FILE;

use crate::{
    client::{handle_open_at, handle_open_path, mode_str_to_access_mode, open_flags_mode},
    libc::{c_char, c_int},
    macros::intercept,
};

const fn has_mode_arg(o_flags: c_int) -> bool {
    o_flags & libc::O_CREAT != 0
}

// https://github.com/tailhook/openat/issues/21#issuecomment-535914957
type Mode = c_int;

intercept!(open(64): unsafe extern "C" fn(*const c_char, c_int, args: ...) -> c_int);
unsafe extern "C" fn open(path: *const c_char, flags: c_int, mut args: ...) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(path, open_flags_mode(flags)) };
    if has_mode_arg(flags) {
        // SAFETY: O_CREAT requires a mode argument.
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc open() with forwarded arguments.
        unsafe { open::original()(path, flags, mode) }
    } else {
        // SAFETY: calling the original libc open() with forwarded arguments.
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
    // SAFETY: dirfd is -1 or valid, and path is a valid C string.
    unsafe { handle_open_at(dirfd, path, open_flags_mode(flags)) };
    if has_mode_arg(flags) {
        // SAFETY: O_CREAT requires a mode argument.
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc openat() with forwarded arguments.
        unsafe { openat::original()(dirfd, path, flags, mode) }
    } else {
        // SAFETY: calling the original libc openat() with forwarded arguments.
        unsafe { openat::original()(dirfd, path, flags) }
    }
}

intercept!(open_nocancel: unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int);
unsafe extern "C" fn open_nocancel(path: *const c_char, flags: c_int, mut args: ...) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(path, open_flags_mode(flags)) };
    if has_mode_arg(flags) {
        // SAFETY: O_CREAT requires a mode argument.
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc open$NOCANCEL() with forwarded arguments.
        unsafe { open_nocancel::original()(path, flags, mode) }
    } else {
        // SAFETY: calling the original libc open$NOCANCEL() with forwarded arguments.
        unsafe { open_nocancel::original()(path, flags) }
    }
}

intercept!(openat_nocancel: unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int);
unsafe extern "C" fn openat_nocancel(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    // SAFETY: dirfd is -1 or valid, and path is a valid C string.
    unsafe { handle_open_at(dirfd, path, open_flags_mode(flags)) };
    if has_mode_arg(flags) {
        // SAFETY: O_CREAT requires a mode argument.
        let mode: Mode = unsafe { args.next_arg() };
        // SAFETY: calling the original libc openat$NOCANCEL() with forwarded arguments.
        unsafe { openat_nocancel::original()(dirfd, path, flags, mode) }
    } else {
        // SAFETY: calling the original libc openat$NOCANCEL() with forwarded arguments.
        unsafe { openat_nocancel::original()(dirfd, path, flags) }
    }
}

intercept!(fopen(64): unsafe extern "C" fn(path: *const c_char, mode: *const c_char) -> *mut FILE);
unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    // SAFETY: path and mode are valid C string pointers provided by the caller.
    unsafe { handle_open_path(path, mode_str_to_access_mode(mode)) };
    // SAFETY: calling the original libc fopen() with forwarded arguments.
    unsafe { fopen::original()(path, mode) }
}

intercept!(freopen(64): unsafe extern "C" fn(path: *const c_char, mode: *const c_char, stream: *mut FILE) -> *mut FILE);
unsafe extern "C" fn freopen(
    path: *const c_char,
    mode: *const c_char,
    stream: *mut FILE,
) -> *mut FILE {
    // SAFETY: path and mode are valid C string pointers provided by the caller.
    unsafe { handle_open_path(path, mode_str_to_access_mode(mode)) };
    // SAFETY: calling the original libc freopen() with forwarded arguments.
    unsafe { freopen::original()(path, mode, stream) }
}
