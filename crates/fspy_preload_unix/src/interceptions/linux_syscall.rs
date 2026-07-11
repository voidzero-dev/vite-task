use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int, c_long};

use crate::{
    client::{
        convert::{Fd, PathAt},
        handle_open,
    },
    macros::intercept,
};

intercept!(syscall(64): unsafe extern "C" fn(c_long, args: ...) -> c_long);
unsafe extern "C" fn syscall(syscall_no: c_long, mut args: ...) -> c_long {
    // https://github.com/bminor/glibc/blob/efc8642051e6c4fe5165e8986c1338ba2c180de6/sysdeps/unix/sysv/linux/syscall.c#L23
    // SAFETY: extracting variadic arguments matching the syscall ABI; the caller passes at least 6 c_long arguments
    let a0 = unsafe { args.next_arg::<c_long>() };
    // SAFETY: extracting variadic arguments matching the syscall ABI
    let a1 = unsafe { args.next_arg::<c_long>() };
    // SAFETY: extracting variadic arguments matching the syscall ABI
    let a2 = unsafe { args.next_arg::<c_long>() };
    // SAFETY: extracting variadic arguments matching the syscall ABI
    let a3 = unsafe { args.next_arg::<c_long>() };
    // SAFETY: extracting variadic arguments matching the syscall ABI
    let a4 = unsafe { args.next_arg::<c_long>() };
    // SAFETY: extracting variadic arguments matching the syscall ABI
    let a5 = unsafe { args.next_arg::<c_long>() };

    if syscall_no == libc::SYS_statx {
        // C-style conversions are expected for the variadic syscall arguments.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "C-style conversion from c_long syscall arguments to c_int"
        )]
        let dirfd = a0 as c_int;
        let pathname = a1 as *const c_char;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "C-style conversion from c_long syscall arguments to c_int"
        )]
        let flags = a2 as c_int;
        if pathname.is_null() {
            if flags & libc::AT_EMPTY_PATH != 0 {
                // SAFETY: dirfd is provided by the statx syscall caller.
                unsafe { handle_open(Fd(dirfd), AccessMode::READ) };
            }
        } else {
            // SAFETY: pathname is a non-null C string pointer provided by the statx syscall caller.
            unsafe { handle_open(PathAt(dirfd, pathname), AccessMode::READ) };
        }
    }
    // SAFETY: forwarding the syscall to the original libc syscall function with the extracted arguments
    unsafe { syscall::original()(syscall_no, a0, a1, a2, a3, a4, a5) }
}
