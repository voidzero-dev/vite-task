use fspy_nostd::BorrowedFd;
use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int, c_long};

use crate::{
    client::{convert::PathAt, handle_open},
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
                // SAFETY: dirfd is -1 or a valid file descriptor provided by the statx syscall caller.
                unsafe { handle_open(BorrowedFd::try_borrow_raw(dirfd), AccessMode::Read) };
            }
        } else {
            // SAFETY: dirfd is -1 or a valid file descriptor, and pathname is a non-null C string pointer, provided by the statx syscall caller.
            unsafe {
                handle_open(PathAt::borrow_raw(dirfd, pathname), AccessMode::Read);
            };
        }
    }
    // Do not resolve libc's syscall through LazyLock: concurrent first calls
    // wait through futex, which calls this interposer again before the lock
    // is initialized. Forward directly to the kernel instead.
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "c_long and usize have the same width on supported Linux targets; preserve their bit patterns"
    )]
    // SAFETY: forward the syscall number and arguments supplied by the caller.
    let result = unsafe {
        syscalls::raw::syscall6(
            syscall_no as usize,
            a0 as usize,
            a1 as usize,
            a2 as usize,
            a3 as usize,
            a4 as usize,
            a5 as usize,
        )
    };
    match syscalls::Errno::from_ret(result) {
        #[expect(clippy::cast_possible_wrap, reason = "preserve the syscall result's bit pattern")]
        Ok(value) => value as c_long,
        Err(errno) => {
            // SAFETY: __errno_location returns this thread's errno storage.
            unsafe { *libc::__errno_location() = errno.into_raw() };
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::syscall;

    #[test]
    fn error_sets_errno() {
        // SAFETY: close rejects an invalid descriptor without accessing memory.
        unsafe {
            *libc::__errno_location() = 0;
            assert_eq!(syscall(libc::SYS_close, -1_i64, 0_i64, 0_i64, 0_i64, 0_i64, 0_i64), -1);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
        }
    }
}
