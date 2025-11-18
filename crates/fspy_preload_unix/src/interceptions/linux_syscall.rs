use std::ffi::{c_char, c_long};

use fspy_shared::ipc::AccessMode;

use crate::{
    client::{convert::PathAt, handle_open},
    macros::intercept,
};

intercept!(syscall(64): unsafe extern "C" fn(c_long, args: ...) -> c_long);
unsafe extern "C" fn syscall(syscall_no: c_long, mut args: ...) -> c_long {
    // https://github.com/bminor/glibc/blob/efc8642051e6c4fe5165e8986c1338ba2c180de6/sysdeps/unix/sysv/linux/syscall.c#L23
    let a0 = unsafe { args.arg::<c_long>() };
    let a1 = unsafe { args.arg::<c_long>() };
    let a2 = unsafe { args.arg::<c_long>() };
    let a3 = unsafe { args.arg::<c_long>() };
    let a4 = unsafe { args.arg::<c_long>() };
    let a5 = unsafe { args.arg::<c_long>() };

    match syscall_no {
        libc::SYS_statx => {
            if let Ok(dirfd) = i32::try_from(a0) {
                let pathname = a1 as *const c_char;
                unsafe {
                    handle_open(PathAt(dirfd, pathname), AccessMode::Read);
                }
            }
        }
        _ => {}
    }
    unsafe { syscall::original()(syscall_no, a0, a1, a2, a3, a4, a5) }
}
