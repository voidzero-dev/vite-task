use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};

use crate::{
    client::{handle_open_at, handle_open_path},
    macros::intercept,
};

intercept!(access(64): unsafe extern "C" fn(pathname: *const c_char, mode: c_int) -> c_int);
unsafe extern "C" fn access(pathname: *const c_char, mode: c_int) -> c_int {
    // SAFETY: pathname is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(pathname, AccessMode::Read) };
    // SAFETY: calling the original libc access() with forwarded arguments.
    unsafe { access::original()(pathname, mode) }
}

intercept!(faccessat(64): unsafe extern "C" fn(dirfd: c_int, pathname: *const c_char, mode: c_int, flags: c_int) -> c_int);
unsafe extern "C" fn faccessat(
    dirfd: c_int,
    pathname: *const c_char,
    mode: c_int,
    flags: c_int,
) -> c_int {
    // SAFETY: dirfd is -1 or valid, and pathname is a valid C string.
    unsafe { handle_open_at(dirfd, pathname, AccessMode::Read) };
    // SAFETY: calling the original libc faccessat() with forwarded arguments.
    unsafe { faccessat::original()(dirfd, pathname, mode, flags) }
}
