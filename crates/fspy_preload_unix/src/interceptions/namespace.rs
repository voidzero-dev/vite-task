//! Calls that make an entry at a path without opening it: mkdir, symlink,
//! and rename and hard link, which put an existing file or folder at a new
//! path. The path is recorded before the call, so a failed call is recorded
//! too: as a mkdir for a mkdir, whose folder may be there already; as a
//! rename target for a rename, which may move a whole folder; and as a write
//! for a symlink, and for a hard link, always to a file. The source of a
//! rename needs no record, and neither does the target of a symlink, which
//! is stored as text, not accessed.

use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int, mode_t};

use crate::{
    client::{convert::PathAt, handle_open},
    macros::intercept,
};

intercept!(mkdir: unsafe extern "C" fn(path: *const c_char, mode: mode_t) -> c_int);
unsafe extern "C" fn mkdir(path: *const c_char, mode: mode_t) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller of the interposed function
    unsafe { handle_open(fspy_nostd::CStr::from_ptr(path.cast()), AccessMode::Mkdir) };
    // SAFETY: calling the original libc mkdir() with the same arguments forwarded from the interposed function
    unsafe { mkdir::original()(path, mode) }
}

intercept!(mkdirat: unsafe extern "C" fn(dirfd: c_int, path: *const c_char, mode: mode_t) -> c_int);
unsafe extern "C" fn mkdirat(dirfd: c_int, path: *const c_char, mode: mode_t) -> c_int {
    // SAFETY: dirfd is -1 or a valid file descriptor, and path is a valid C string, provided by the caller of the interposed function
    unsafe { handle_open(PathAt::borrow_raw(dirfd, path), AccessMode::Mkdir) };
    // SAFETY: calling the original libc mkdirat() with the same arguments forwarded from the interposed function
    unsafe { mkdirat::original()(dirfd, path, mode) }
}

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

intercept!(symlink: unsafe extern "C" fn(target: *const c_char, link: *const c_char) -> c_int);
unsafe extern "C" fn symlink(target: *const c_char, link: *const c_char) -> c_int {
    // SAFETY: link is a valid C string pointer provided by the caller of the interposed function
    unsafe { handle_open(fspy_nostd::CStr::from_ptr(link.cast()), AccessMode::Write) };
    // SAFETY: calling the original libc symlink() with the same arguments forwarded from the interposed function
    unsafe { symlink::original()(target, link) }
}

intercept!(symlinkat: unsafe extern "C" fn(target: *const c_char, newdirfd: c_int, link: *const c_char) -> c_int);
unsafe extern "C" fn symlinkat(
    target: *const c_char,
    newdirfd: c_int,
    link: *const c_char,
) -> c_int {
    // SAFETY: newdirfd is -1 or a valid file descriptor, and link is a valid C string, provided by the caller of the interposed function
    unsafe { handle_open(PathAt::borrow_raw(newdirfd, link), AccessMode::Write) };
    // SAFETY: calling the original libc symlinkat() with the same arguments forwarded from the interposed function
    unsafe { symlinkat::original()(target, newdirfd, link) }
}
