//! Interceptions for calls that change the shape of the tree rather than the
//! contents of a file: rename, unlink, rmdir and mkdir.
//!
//! Without these, a tool that publishes results atomically is invisible. It
//! writes a temporary and renames it over the destination, so the destination
//! never appears in a write event and the temporary — which no longer exists —
//! does. Directory renames are worse: a build that stages into `dist.tmp` and
//! swaps it into place produces no write event for any of its real outputs.
//!
//! These report before the real call, like every other interception, and record
//! only what the arguments say. Nothing here depends on whether the call
//! succeeded: the Linux seccomp supervisor cannot observe outcomes at all, so an
//! outcome-dependent rule would hold on some backends and not others.
//!
//! `mkdir` is intentionally not intercepted. Its argument alone cannot say
//! whether this run created the directory, and callers overwhelmingly use it as
//! "ensure this exists" and ignore `EEXIST`, so recording it would claim every
//! pre-existing directory.

use fspy_shared::ipc::AccessMode;

use crate::{
    client::{convert::PathAt, handle_open},
    libc::{c_char, c_int},
    macros::intercept,
};

/// Whether a path is a directory, for tagging rename and removal events.
///
/// A consumer needs this to re-attribute writes recorded beneath a staging
/// directory to the published location. Inspecting the source is not the same as
/// inspecting the rename's result: it describes an argument, and it happens
/// before the call while the source is still there.
unsafe fn is_directory_at(dirfd: c_int, path: *const c_char) -> bool {
    // SAFETY: an all-zero stat is a valid initial value; fstatat overwrites it
    let mut stat_buf: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: path is a valid C string pointer and stat_buf is a valid, owned stat struct
    let result =
        unsafe { libc::fstatat(dirfd, path, &raw mut stat_buf, libc::AT_SYMLINK_NOFOLLOW) };
    result == 0 && (stat_buf.st_mode & libc::S_IFMT) == libc::S_IFDIR
}

/// Report both halves of a rename: the source is being retired, the destination
/// is being replaced by whatever the source held.
unsafe fn report_rename(
    old_dirfd: c_int,
    old_path: *const c_char,
    new_dirfd: c_int,
    new_path: *const c_char,
    is_directory: bool,
) {
    let dir_flag = if is_directory { AccessMode::IS_DIR } else { AccessMode::empty() };
    // SAFETY: old_path and new_path are valid C string pointers from the caller
    unsafe {
        handle_open(
            PathAt(old_dirfd, old_path),
            AccessMode::RENAME_FROM | AccessMode::DELETED | dir_flag,
        );
        handle_open(
            PathAt(new_dirfd, new_path),
            AccessMode::RENAME_TO | AccessMode::WRITE | dir_flag,
        );
    }
}

intercept!(rename: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int);
unsafe extern "C" fn rename(old_path: *const c_char, new_path: *const c_char) -> c_int {
    // SAFETY: old_path is a valid C string pointer provided by the caller
    let is_directory = unsafe { is_directory_at(libc::AT_FDCWD, old_path) };
    // SAFETY: both paths are valid C string pointers provided by the caller
    unsafe {
        report_rename(libc::AT_FDCWD, old_path, libc::AT_FDCWD, new_path, is_directory);
    }
    // SAFETY: forwarding the caller's arguments to the original libc rename()
    unsafe { rename::original()(old_path, new_path) }
}

intercept!(renameat: unsafe extern "C" fn(c_int, *const c_char, c_int, *const c_char) -> c_int);
unsafe extern "C" fn renameat(
    old_dirfd: c_int,
    old_path: *const c_char,
    new_dirfd: c_int,
    new_path: *const c_char,
) -> c_int {
    // SAFETY: old_dirfd and old_path are valid arguments provided by the caller
    let is_directory = unsafe { is_directory_at(old_dirfd, old_path) };
    // SAFETY: both paths are valid C string pointers provided by the caller
    unsafe { report_rename(old_dirfd, old_path, new_dirfd, new_path, is_directory) };
    // SAFETY: forwarding the caller's arguments to the original libc renameat()
    unsafe { renameat::original()(old_dirfd, old_path, new_dirfd, new_path) }
}

// macOS publishes atomic swaps through renameatx_np with RENAME_SWAP, which
// mutates both paths. Vite's dependency optimizer and rustup both reach it.
#[cfg(target_os = "macos")]
intercept!(renameatx_np: unsafe extern "C" fn(c_int, *const c_char, c_int, *const c_char, libc::c_uint) -> c_int);
#[cfg(target_os = "macos")]
unsafe extern "C" fn renameatx_np(
    old_dirfd: c_int,
    old_path: *const c_char,
    new_dirfd: c_int,
    new_path: *const c_char,
    flags: libc::c_uint,
) -> c_int {
    // SAFETY: old_dirfd and old_path are valid arguments provided by the caller
    let is_directory = unsafe { is_directory_at(old_dirfd, old_path) };
    // SAFETY: both paths are valid C string pointers provided by the caller
    unsafe { report_rename(old_dirfd, old_path, new_dirfd, new_path, is_directory) };
    // RENAME_SWAP mutates both sides, so the source is replaced too rather than
    // simply retired.
    if flags & libc::RENAME_SWAP != 0 {
        // SAFETY: old_path is a valid C string pointer provided by the caller
        unsafe {
            handle_open(PathAt(old_dirfd, old_path), AccessMode::RENAME_TO | AccessMode::WRITE);
        }
    }
    // SAFETY: forwarding the caller's arguments to the original renameatx_np()
    unsafe { renameatx_np::original()(old_dirfd, old_path, new_dirfd, new_path, flags) }
}

// Linux's renameat2 can also exchange two paths, in which case both sides are
// mutated rather than one replacing the other.
#[cfg(target_os = "linux")]
intercept!(renameat2: unsafe extern "C" fn(c_int, *const c_char, c_int, *const c_char, libc::c_uint) -> c_int);
#[cfg(target_os = "linux")]
unsafe extern "C" fn renameat2(
    old_dirfd: c_int,
    old_path: *const c_char,
    new_dirfd: c_int,
    new_path: *const c_char,
    flags: libc::c_uint,
) -> c_int {
    // SAFETY: old_dirfd and old_path are valid arguments provided by the caller
    let is_directory = unsafe { is_directory_at(old_dirfd, old_path) };
    // SAFETY: both paths are valid C string pointers provided by the caller
    unsafe { report_rename(old_dirfd, old_path, new_dirfd, new_path, is_directory) };
    // RENAME_EXCHANGE mutates both sides, so the source is replaced too rather
    // than simply retired.
    if flags & libc::RENAME_EXCHANGE != 0 {
        // SAFETY: old_path is a valid C string pointer provided by the caller
        unsafe {
            handle_open(PathAt(old_dirfd, old_path), AccessMode::RENAME_TO | AccessMode::WRITE);
        }
    }
    // SAFETY: forwarding the caller's arguments to the original renameat2()
    unsafe { renameat2::original()(old_dirfd, old_path, new_dirfd, new_path, flags) }
}

intercept!(unlink: unsafe extern "C" fn(*const c_char) -> c_int);
unsafe extern "C" fn unlink(path: *const c_char) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller
    unsafe { handle_open(path, AccessMode::DELETED) };
    // SAFETY: forwarding the caller's argument to the original libc unlink()
    unsafe { unlink::original()(path) }
}

intercept!(unlinkat: unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int);
unsafe extern "C" fn unlinkat(dirfd: c_int, path: *const c_char, flags: c_int) -> c_int {
    let dir_flag =
        if flags & libc::AT_REMOVEDIR != 0 { AccessMode::IS_DIR } else { AccessMode::empty() };
    // SAFETY: dirfd and path are valid arguments provided by the caller
    unsafe { handle_open(PathAt(dirfd, path), AccessMode::DELETED | dir_flag) };
    // SAFETY: forwarding the caller's arguments to the original libc unlinkat()
    unsafe { unlinkat::original()(dirfd, path, flags) }
}

intercept!(remove: unsafe extern "C" fn(*const c_char) -> c_int);
unsafe extern "C" fn remove(path: *const c_char) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller
    unsafe { handle_open(path, AccessMode::DELETED) };
    // SAFETY: forwarding the caller's argument to the original libc remove()
    unsafe { remove::original()(path) }
}

intercept!(rmdir: unsafe extern "C" fn(*const c_char) -> c_int);
unsafe extern "C" fn rmdir(path: *const c_char) -> c_int {
    // SAFETY: path is a valid C string pointer provided by the caller
    unsafe { handle_open(path, AccessMode::DELETED | AccessMode::IS_DIR) };
    // SAFETY: forwarding the caller's argument to the original libc rmdir()
    unsafe { rmdir::original()(path) }
}
