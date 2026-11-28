use fspy_shared::ipc::AccessMode;
use libc::{DIR, c_char, c_int, c_long, c_void};

use crate::{
    client::{handle_open_fd, handle_open_path},
    macros::intercept,
};

intercept!(scandir(64): unsafe extern "C" fn(
    dirname: *const c_char,
    namelist: *mut c_void,
    select: *const c_void,
    compar: *const c_void,
) -> c_int);
unsafe extern "C" fn scandir(
    dirname: *const c_char,
    namelist: *mut c_void,
    select: *const c_void,
    compar: *const c_void,
) -> c_int {
    // SAFETY: dirname is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(dirname, AccessMode::ReadDir) };
    // SAFETY: calling the original libc scandir() with forwarded arguments.
    unsafe { scandir::original()(dirname, namelist, select, compar) }
}

intercept!(scandir_b: unsafe extern "C" fn(
    dirname: *const c_char,
    namelist: *mut c_void,
    select: *const c_void,
    compar: *const c_void,
) -> c_int);
unsafe extern "C" fn scandir_b(
    dirname: *const c_char,
    namelist: *mut c_void,
    select: *const c_void,
    compar: *const c_void,
) -> c_int {
    // SAFETY: dirname is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(dirname, AccessMode::ReadDir) };
    // SAFETY: calling the original libc scandir_b() with forwarded arguments.
    unsafe { scandir_b::original()(dirname, namelist, select, compar) }
}

intercept!(__getdirentries64: unsafe extern "C" fn(c_int, *mut u8, usize, *mut i64) -> isize);
unsafe extern "C" fn __getdirentries64(
    fd: c_int,
    buf: *mut u8,
    buf_len: usize,
    basep: *mut i64,
) -> isize {
    // SAFETY: fd is -1 or a valid file descriptor provided by the caller.
    unsafe { handle_open_fd(fd, AccessMode::ReadDir) };
    // SAFETY: calling the original libc __getdirentries64() with forwarded arguments.
    unsafe { __getdirentries64::original()(fd, buf, buf_len, basep) }
}

intercept!(getdirentries(64): unsafe extern "C" fn(fd: c_int, buf: *mut c_char, nbytes: c_int, basep: *mut c_long) -> c_int);
unsafe extern "C" fn getdirentries(
    fd: c_int,
    buf: *mut c_char,
    nbytes: c_int,
    basep: *mut c_long,
) -> c_int {
    // SAFETY: fd is -1 or a valid file descriptor provided by the caller.
    unsafe { handle_open_fd(fd, AccessMode::ReadDir) };
    // SAFETY: calling the original libc getdirentries() with forwarded arguments.
    unsafe { getdirentries::original()(fd, buf, nbytes, basep) }
}

intercept!(fdopendir(64): unsafe extern "C" fn(fd: c_int) -> *mut DIR);
unsafe extern "C" fn fdopendir(fd: c_int) -> *mut DIR {
    // SAFETY: fd is -1 or a valid file descriptor provided by the caller.
    unsafe { handle_open_fd(fd, AccessMode::ReadDir) };
    // SAFETY: calling the original libc fdopendir() with forwarded arguments.
    unsafe { fdopendir::original()(fd) }
}

intercept!(opendir(64): unsafe extern "C" fn(*const c_char) -> *mut DIR);
unsafe extern "C" fn opendir(dir_name: *const c_char) -> *mut DIR {
    // SAFETY: dir_name is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(dir_name, AccessMode::ReadDir) };
    // SAFETY: calling the original libc opendir() with forwarded arguments.
    unsafe { opendir::original()(dir_name) }
}
