use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use allocator_api2::vec::Vec as AllocVec;
use fspy_nostd::{BorrowedFd, CStr, CWD, Thin};
use fspy_nostd_alloc::fs::{fd_path, resolve_path_at};
use fspy_shared_unix::RawExec;
use libc::{c_char, c_int};

use crate::{client::global_client, macros::intercept};

type PosixSpawnFn = unsafe extern "C" fn(
    pid: *mut libc::pid_t,
    prog: *const c_char,
    file_actions: *const libc::posix_spawn_file_actions_t,
    attrp: *const libc::posix_spawnattr_t,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int;

/// Number of `posix_spawn_file_actions_t` handles currently tracked in
/// [`SPAWN_CHDIRS`]. Allows `posix_spawn(p)` and `destroy` to skip the mutex
/// completely in the common case when no chdir action is active.
static ACTIVE_CHDIRS: AtomicUsize = AtomicUsize::new(0);

/// Maps live `posix_spawn_file_actions_t` handle values (`*file_actions as usize`)
/// to their effective absolute working directory from `addchdir_np` / `addfchdir_np`.
static SPAWN_CHDIRS: Mutex<Vec<(usize, Vec<u8>)>> = Mutex::new(Vec::new());

fn set_spawn_chdir(key: usize, path: Vec<u8>) {
    let mut map = SPAWN_CHDIRS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, existing)) = map.iter_mut().find(|(k, _)| *k == key) {
        *existing = path;
    } else {
        map.push((key, path));
        ACTIVE_CHDIRS.store(map.len(), Ordering::Release);
    }
}

fn remove_spawn_chdir(key: usize) {
    if ACTIVE_CHDIRS.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut map = SPAWN_CHDIRS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(pos) = map.iter().position(|(k, _)| *k == key) {
        map.swap_remove(pos);
        ACTIVE_CHDIRS.store(map.len(), Ordering::Release);
    }
}

fn get_spawn_chdir_in<A: allocator_api2::alloc::Allocator>(
    file_actions: *const libc::posix_spawn_file_actions_t,
    allocator: A,
) -> Option<AllocVec<u8, A>> {
    if file_actions.is_null() || ACTIVE_CHDIRS.load(Ordering::Acquire) == 0 {
        return None;
    }
    // SAFETY: caller passed a non-null `file_actions` pointer to `posix_spawn(p)`.
    let key = (unsafe { *file_actions }) as usize;
    if key == 0 {
        return None;
    }
    let map = SPAWN_CHDIRS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let out = map.iter().find(|(k, _)| *k == key).map(|(_, bytes)| {
        let mut out = AllocVec::with_capacity_in(bytes.len(), allocator);
        out.extend_from_slice(bytes);
        out
    });
    drop(map);
    out
}

intercept!(posix_spawn_file_actions_addchdir_np: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    path: *const c_char,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_addchdir_np(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    path: *const c_char,
) -> c_int {
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret = unsafe { posix_spawn_file_actions_addchdir_np::original()(file_actions, path) };
    if ret == 0 && !file_actions.is_null() && !path.is_null() {
        // SAFETY: `original` succeeded on non-null `file_actions`.
        let key = (unsafe { *file_actions }) as usize;
        if key != 0 {
            // SAFETY: non-null `path` is a valid NUL-terminated C string.
            let c_path = unsafe { CStr::<Thin>::from_ptr(path.cast()) };
            let raw = c_path.count().as_units();
            if raw.starts_with(b"/") {
                set_spawn_chdir(key, raw.to_vec());
            } else if !raw.is_empty() {
                let prev =
                    get_spawn_chdir_in(file_actions.cast_const(), allocator_api2::alloc::Global);
                let resolved = prev.map_or_else(
                    || {
                        let arena = fspy_nostd_alloc::pooled_bump();
                        resolve_path_at(&arena, Some(CWD), c_path)
                            .ok()
                            .flatten()
                            .map(|p| p.as_units().to_vec())
                    },
                    |base| {
                        let mut joined = base.to_vec();
                        if joined.last() != Some(&b'/') {
                            joined.push(b'/');
                        }
                        joined.extend_from_slice(raw);
                        Some(joined)
                    },
                );
                if let Some(dir) = resolved {
                    set_spawn_chdir(key, dir);
                }
            }
        }
    }
    ret
}

intercept!(posix_spawn_file_actions_addfchdir_np: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    fd: c_int,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_addfchdir_np(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    fd: c_int,
) -> c_int {
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret = unsafe { posix_spawn_file_actions_addfchdir_np::original()(file_actions, fd) };
    if ret == 0 && !file_actions.is_null() {
        // SAFETY: `original` succeeded on non-null `file_actions`.
        let key = (unsafe { *file_actions }) as usize;
        if key != 0 {
            // SAFETY: `fd` was accepted by `addfchdir_np`.
            let borrowed = unsafe { BorrowedFd::try_borrow_raw(fd) };
            if let Some(borrowed) = borrowed {
                let arena = fspy_nostd_alloc::pooled_bump();
                if let Ok(Some(dir)) = fd_path(&arena, borrowed) {
                    set_spawn_chdir(key, dir.as_units().to_vec());
                }
            }
        }
    }
    ret
}

intercept!(posix_spawn_file_actions_destroy: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_destroy(
    file_actions: *mut libc::posix_spawn_file_actions_t,
) -> c_int {
    if !file_actions.is_null() {
        // Remove before freeing `*file_actions` to prevent an ABA race if
        // another thread immediately allocates the same address in `init`.
        // SAFETY: caller passed a non-null `file_actions` pointer.
        let key = (unsafe { *file_actions }) as usize;
        if key != 0 {
            remove_spawn_chdir(key);
        }
    }
    // SAFETY: forwarding caller argument to the original libc function.
    unsafe { posix_spawn_file_actions_destroy::original()(file_actions) }
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the posix_spawn(3) signature which requires all these parameters"
)]
unsafe fn handle_posix_spawn(
    search_path: bool,
    original: PosixSpawnFn,
    pid: *mut libc::pid_t,
    file: *const c_char,
    file_actions: *const libc::posix_spawn_file_actions_t,
    attrp: *const libc::posix_spawnattr_t,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    let client = global_client()
        .expect("posix_spawn(p) unexpectedly called before client initialized in ctor");

    let arena = fspy_nostd_alloc::pooled_bump();
    let spawn_cwd = get_spawn_chdir_in(file_actions, &arena);
    // SAFETY: file, argv, and envp are valid pointers forwarded from posix_spawn(p).
    let raw_command = match unsafe {
        client.handle_exec_in_dir(
            search_path,
            spawn_cwd.as_deref(),
            RawExec { prog: file, argv: argv.cast(), envp: envp.cast() },
            &arena,
        )
    } {
        Ok(raw) => raw,
        Err(err) => return err.raw_os_error(),
    };
    // SAFETY: caller's pointers are forwarded unchanged, and `raw_command`'s
    // strings and arrays live in `arena`, which outlives this call.
    unsafe {
        original(
            pid,
            raw_command.prog,
            file_actions,
            attrp,
            raw_command.argv.cast(),
            raw_command.envp.cast(),
        )
    }
}

intercept!(posix_spawnp(64): PosixSpawnFn);
unsafe extern "C" fn posix_spawnp(
    pid: *mut libc::pid_t,
    file: *const c_char,
    file_actions: *const libc::posix_spawn_file_actions_t,
    attrp: *const libc::posix_spawnattr_t,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    // SAFETY: all arguments are valid pointers forwarded from posix_spawnp.
    unsafe {
        handle_posix_spawn(
            true,
            posix_spawnp::original(),
            pid,
            file,
            file_actions,
            attrp,
            argv,
            envp,
        )
    }
}

intercept!(posix_spawn(64): PosixSpawnFn);
unsafe extern "C" fn posix_spawn(
    pid: *mut libc::pid_t,
    file: *const c_char,
    file_actions: *const libc::posix_spawn_file_actions_t,
    attrp: *const libc::posix_spawnattr_t,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    // SAFETY: all arguments are valid pointers forwarded from posix_spawn.
    unsafe {
        handle_posix_spawn(
            false,
            posix_spawn::original(),
            pid,
            file,
            file_actions,
            attrp,
            argv,
            envp,
        )
    }
}
