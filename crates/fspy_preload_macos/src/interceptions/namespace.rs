//! Calls that make an entry at a path without opening it: mkdir, symlink,
//! and rename and hard link, which put an existing file or folder at a new
//! path.

use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int, mode_t};

use crate::{
    client::{handle_open_at, handle_open_path},
    macros::intercept,
};

intercept!(mkdir: unsafe extern "C" fn(path: *const c_char, mode: mode_t) -> c_int);
unsafe extern "C" fn mkdir(path: *const c_char, mode: mode_t) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(path, AccessMode::Mkdir) };
    // SAFETY: calling the original libc mkdir() with forwarded arguments.
    unsafe { mkdir::original()(path, mode) }
}

intercept!(mkdirat: unsafe extern "C" fn(dirfd: c_int, path: *const c_char, mode: mode_t) -> c_int);
unsafe extern "C" fn mkdirat(dirfd: c_int, path: *const c_char, mode: mode_t) -> c_int {
    // SAFETY: dirfd is -1 or valid, and path is a valid C string.
    unsafe { handle_open_at(dirfd, path, AccessMode::Mkdir) };
    // SAFETY: calling the original libc mkdirat() with forwarded arguments.
    unsafe { mkdirat::original()(dirfd, path, mode) }
}

intercept!(rename: unsafe extern "C" fn(old: *const c_char, new: *const c_char) -> c_int);
unsafe extern "C" fn rename(old: *const c_char, new: *const c_char) -> c_int {
    // SAFETY: new is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(new, AccessMode::RenameTarget) };
    // SAFETY: calling the original libc rename() with forwarded arguments.
    unsafe { rename::original()(old, new) }
}

intercept!(renameat: unsafe extern "C" fn(olddirfd: c_int, old: *const c_char, newdirfd: c_int, new: *const c_char) -> c_int);
unsafe extern "C" fn renameat(
    olddirfd: c_int,
    old: *const c_char,
    newdirfd: c_int,
    new: *const c_char,
) -> c_int {
    // SAFETY: newdirfd is -1 or valid, and new is a valid C string.
    unsafe { handle_open_at(newdirfd, new, AccessMode::RenameTarget) };
    // SAFETY: calling the original libc renameat() with forwarded arguments.
    unsafe { renameat::original()(olddirfd, old, newdirfd, new) }
}

intercept!(renamex_np: unsafe extern "C" fn(old: *const c_char, new: *const c_char, flags: libc::c_uint) -> c_int);
unsafe extern "C" fn renamex_np(
    old: *const c_char,
    new: *const c_char,
    flags: libc::c_uint,
) -> c_int {
    if flags & libc::RENAME_SWAP != 0 {
        // SAFETY: old is a valid C string pointer provided by the caller.
        unsafe { handle_open_path(old, AccessMode::RenameTarget) };
    }
    // SAFETY: new is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(new, AccessMode::RenameTarget) };
    // SAFETY: calling the original libc renamex_np() with forwarded arguments.
    unsafe { renamex_np::original()(old, new, flags) }
}

intercept!(renameatx_np: unsafe extern "C" fn(olddirfd: c_int, old: *const c_char, newdirfd: c_int, new: *const c_char, flags: libc::c_uint) -> c_int);
unsafe extern "C" fn renameatx_np(
    olddirfd: c_int,
    old: *const c_char,
    newdirfd: c_int,
    new: *const c_char,
    flags: libc::c_uint,
) -> c_int {
    if flags & libc::RENAME_SWAP != 0 {
        // SAFETY: olddirfd is -1 or valid, and old is a valid C string.
        unsafe { handle_open_at(olddirfd, old, AccessMode::RenameTarget) };
    }
    // SAFETY: newdirfd is -1 or valid, and new is a valid C string.
    unsafe { handle_open_at(newdirfd, new, AccessMode::RenameTarget) };
    // SAFETY: calling the original libc renameatx_np() with forwarded arguments.
    unsafe { renameatx_np::original()(olddirfd, old, newdirfd, new, flags) }
}

intercept!(link: unsafe extern "C" fn(old: *const c_char, new: *const c_char) -> c_int);
unsafe extern "C" fn link(old: *const c_char, new: *const c_char) -> c_int {
    // SAFETY: new is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(new, AccessMode::Write) };
    // SAFETY: calling the original libc link() with forwarded arguments.
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
    // SAFETY: newdirfd is -1 or valid, and new is a valid C string.
    unsafe { handle_open_at(newdirfd, new, AccessMode::Write) };
    // SAFETY: calling the original libc linkat() with forwarded arguments.
    unsafe { linkat::original()(olddirfd, old, newdirfd, new, flags) }
}

intercept!(symlink: unsafe extern "C" fn(target: *const c_char, link: *const c_char) -> c_int);
unsafe extern "C" fn symlink(target: *const c_char, link: *const c_char) -> c_int {
    // SAFETY: link is a valid C string pointer provided by the caller.
    unsafe { handle_open_path(link, AccessMode::Write) };
    // SAFETY: calling the original libc symlink() with forwarded arguments.
    unsafe { symlink::original()(target, link) }
}

intercept!(symlinkat: unsafe extern "C" fn(target: *const c_char, newdirfd: c_int, link: *const c_char) -> c_int);
unsafe extern "C" fn symlinkat(
    target: *const c_char,
    newdirfd: c_int,
    link: *const c_char,
) -> c_int {
    // SAFETY: newdirfd is -1 or valid, and link is a valid C string.
    unsafe { handle_open_at(newdirfd, link, AccessMode::Write) };
    // SAFETY: calling the original libc symlinkat() with forwarded arguments.
    unsafe { symlinkat::original()(target, newdirfd, link) }
}
