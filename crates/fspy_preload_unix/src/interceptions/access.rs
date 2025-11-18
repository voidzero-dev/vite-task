use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};

use crate::{
    client::{convert::PathAt, handle_open},
    macros::intercept,
};

intercept!(access(64): unsafe extern "C" fn(pathname: *const c_char, mode: c_int) -> c_int);
unsafe extern "C" fn access(pathname: *const c_char, mode: c_int) -> c_int {
    unsafe {
        handle_open(pathname, AccessMode::READ);
    }
    unsafe { access::original()(pathname, mode) }
}

intercept!(faccessat(64): unsafe extern "C" fn(dirfd: c_int, pathname: *const c_char, mode: c_int, flags: c_int) -> c_int);
unsafe extern "C" fn faccessat(
    dirfd: c_int,
    pathname: *const c_char,
    mode: c_int,
    flags: c_int,
) -> c_int {
    unsafe {
        handle_open(PathAt(dirfd, pathname), AccessMode::READ);
    }
    unsafe { faccessat::original()(dirfd, pathname, mode, flags) }
}
