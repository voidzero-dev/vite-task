use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int, c_long};

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
            // c-style conversion is expected: (4294967196 -> -100 aka libc::AT_FDCWD)
            let dirfd = a0 as c_int;
            let pathname = a1 as *const c_char;
            unsafe {
                handle_open(PathAt(dirfd, pathname), AccessMode::READ);
            }
        }
        _ => {}
    }
    unsafe { syscall::original()(syscall_no, a0, a1, a2, a3, a4, a5) }
}
