use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int, stat as stat_struct};

use crate::{
    client::{handle_open_at, handle_open_path},
    macros::intercept,
};

intercept!(stat(64): unsafe extern "C" fn(path: *const c_char, buf: *mut stat_struct) -> c_int);
unsafe extern "C" fn stat(path: *const c_char, buf: *mut stat_struct) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(path, AccessMode::Read) };
    // SAFETY: calling the original libc stat() with forwarded arguments.
    unsafe { stat::original()(path, buf) }
}

intercept!(lstat(64): unsafe extern "C" fn(path: *const c_char, buf: *mut stat_struct) -> c_int);
unsafe extern "C" fn lstat(path: *const c_char, buf: *mut stat_struct) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(path, AccessMode::Read) };
    // SAFETY: calling the original libc lstat() with forwarded arguments.
    unsafe { lstat::original()(path, buf) }
}

intercept!(fstatat(64): unsafe extern "C" fn(dirfd: c_int, pathname: *const c_char, buf: *mut stat_struct, flags: c_int) -> c_int);
unsafe extern "C" fn fstatat(
    dirfd: c_int,
    pathname: *const c_char,
    buf: *mut stat_struct,
    flags: c_int,
) -> c_int {
    // SAFETY: dirfd is -1 or valid, and pathname is a valid C string.
    unsafe { handle_open_at(dirfd, pathname, AccessMode::Read) };
    // SAFETY: calling the original libc fstatat() with forwarded arguments.
    unsafe { fstatat::original()(dirfd, pathname, buf, flags) }
}
