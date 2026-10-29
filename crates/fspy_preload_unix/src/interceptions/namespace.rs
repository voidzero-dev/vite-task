//! Calls that put an existing file or folder at a new path without opening
//! it: rename and hard link. The new path is recorded before the call, so a
//! failed call is recorded too: as a rename target for a rename, which may
//! move a whole folder, and as a write for a hard link, always to a file. The
//! source of a rename needs no record.

use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};

use crate::{
    client::{convert::PathAt, handle_open},
    macros::intercept,
};

intercept!(rename: unsafe extern "C" fn(old: *const c_char, new: *const c_char) -> c_int);
unsafe extern "C" fn rename(old: *const c_char, new: *const c_char) -> c_int {
    // SAFETY: new is a valid C string pointer provided by the caller of the interposed function
    unsafe { handle_open(fspy_nostd::CStr::from_ptr(new.cast()), AccessMode::RenameTarget) };
    // SAFETY: calling the original libc rename() with the same arguments forwarded from the interposed function
    unsafe { rename::original()(old, new) }
}

intercept!(renameat: unsafe extern "C" fn(olddirfd: c_int, old: *const c_char, newdirfd: c_int, new: *const c_char) -> c_int);
unsafe extern "C" fn renameat(
    olddirfd: c_int,
    old: *const c_char,
    newdirfd: c_int,
    new: *const c_char,
) -> c_int {
    // SAFETY: newdirfd is -1 or a valid file descriptor, and new is a valid C string, provided by the caller of the interposed function
    unsafe { handle_open(PathAt::borrow_raw(newdirfd, new), AccessMode::RenameTarget) };
    // SAFETY: calling the original libc renameat() with the same arguments forwarded from the interposed function
    unsafe { renameat::original()(olddirfd, old, newdirfd, new) }
}

#[cfg(target_os = "linux")]
intercept!(renameat2: unsafe extern "C" fn(olddirfd: c_int, old: *const c_char, newdirfd: c_int, new: *const c_char, flags: libc::c_uint) -> c_int);
/// With `RENAME_EXCHANGE` both paths change; only the destination is
/// recorded, as for a plain rename.
#[cfg(target_os = "linux")]
unsafe extern "C" fn renameat2(
    olddirfd: c_int,
    old: *const c_char,
    newdirfd: c_int,
    new: *const c_char,
    flags: libc::c_uint,
) -> c_int {
    let Some(original) = renameat2::try_original() else {
        // glibc added renameat2 in 2.28. Callers that look it up at run time
        // treat ENOSYS as unsupported, as when the kernel lacks it.
        // SAFETY: __errno_location returns the calling thread's errno storage on Linux.
        unsafe { *libc::__errno_location() = libc::ENOSYS };
        return -1;
    };
    // SAFETY: newdirfd is -1 or a valid file descriptor, and new is a valid C string, provided by the caller of the interposed function
    unsafe { handle_open(PathAt::borrow_raw(newdirfd, new), AccessMode::RenameTarget) };
    // SAFETY: calling the original libc renameat2() with the same arguments forwarded from the interposed function
    unsafe { original(olddirfd, old, newdirfd, new, flags) }
}

#[cfg(target_os = "macos")]
intercept!(renamex_np: unsafe extern "C" fn(old: *const c_char, new: *const c_char, flags: libc::c_uint) -> c_int);
#[cfg(target_os = "macos")]
unsafe extern "C" fn renamex_np(
    old: *const c_char,
    new: *const c_char,
    flags: libc::c_uint,
) -> c_int {
    // SAFETY: new is a valid C string pointer provided by the caller of the interposed function
    unsafe { handle_open(fspy_nostd::CStr::from_ptr(new.cast()), AccessMode::RenameTarget) };
    // SAFETY: calling the original libc renamex_np() with the same arguments forwarded from the interposed function
    unsafe { renamex_np::original()(old, new, flags) }
}

#[cfg(target_os = "macos")]
intercept!(renameatx_np: unsafe extern "C" fn(olddirfd: c_int, old: *const c_char, newdirfd: c_int, new: *const c_char, flags: libc::c_uint) -> c_int);
#[cfg(target_os = "macos")]
unsafe extern "C" fn renameatx_np(
    olddirfd: c_int,
    old: *const c_char,
    newdirfd: c_int,
    new: *const c_char,
    flags: libc::c_uint,
) -> c_int {
    // SAFETY: newdirfd is -1 or a valid file descriptor, and new is a valid C string, provided by the caller of the interposed function
    unsafe { handle_open(PathAt::borrow_raw(newdirfd, new), AccessMode::RenameTarget) };
    // SAFETY: calling the original libc renameatx_np() with the same arguments forwarded from the interposed function
    unsafe { renameatx_np::original()(olddirfd, old, newdirfd, new, flags) }
}

intercept!(link: unsafe extern "C" fn(old: *const c_char, new: *const c_char) -> c_int);
unsafe extern "C" fn link(old: *const c_char, new: *const c_char) -> c_int {
    // SAFETY: new is a valid C string pointer provided by the caller of the interposed function
    unsafe { handle_open(fspy_nostd::CStr::from_ptr(new.cast()), AccessMode::Write) };
    // SAFETY: calling the original libc link() with the same arguments forwarded from the interposed function
    unsafe { link::original()(old, new) }
}

intercept!(linkat: unsafe extern "C" fn(olddirfd: c_int, old: *const c_char, newdirfd: c_int, new: *const c_char, flags: c_int) -> c_int);
unsafe extern "C" fn linkat(
    olddirfd: c_int,
    old: *const c_char,
    newdirfd: c_int,
    new: *const c_char,
    flags: c_int,
) -> c_int {
    // SAFETY: newdirfd is -1 or a valid file descriptor, and new is a valid C string, provided by the caller of the interposed function
    unsafe { handle_open(PathAt::borrow_raw(newdirfd, new), AccessMode::Write) };
    // SAFETY: calling the original libc linkat() with the same arguments forwarded from the interposed function
    unsafe { linkat::original()(olddirfd, old, newdirfd, new, flags) }
}
