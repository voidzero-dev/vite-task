use std::{
    cell::UnsafeCell,
    sync::{
        Once,
        atomic::{AtomicUsize, Ordering},
    },
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

/// Initial capacity pre-allocated for [`SPAWN_CHDIRS`] outside the mutex.
const INITIAL_CAPACITY: usize = 64;

/// Number of `posix_spawn_file_actions_t` handles currently tracked in
/// [`SPAWN_CHDIRS`]. Allows `posix_spawn(p)` and `destroy` to skip the mutex
/// completely in the common case when no chdir action is active.
static ACTIVE_CHDIRS: AtomicUsize = AtomicUsize::new(0);

/// Fork-safe table mapping live `posix_spawn_file_actions_t` handle values
/// (`*file_actions as usize`) to their effective absolute working directory
/// from `addchdir_np` / `addfchdir_np`.
///
/// Coordinated across `fork()` via `pthread_atfork`. No heap allocation or
/// deallocation ever runs while holding `mutex`, avoiding lock-order inversion
/// with libmalloc's own `atfork` handlers.
struct SpawnChdirTable {
    mutex: UnsafeCell<libc::pthread_mutex_t>,
    entries: UnsafeCell<Vec<(usize, Box<[u8]>)>>,
}

// SAFETY: `entries` is only accessed while holding `mutex`.
unsafe impl Sync for SpawnChdirTable {}

static SPAWN_CHDIRS: SpawnChdirTable = SpawnChdirTable {
    mutex: UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER),
    entries: UnsafeCell::new(Vec::new()),
};
static ATFORK_INIT: Once = Once::new();

unsafe extern "C" fn atfork_prepare() {
    // SAFETY: `SPAWN_CHDIRS.mutex` is statically initialized.
    unsafe { libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get()) };
}

unsafe extern "C" fn atfork_parent() {
    // SAFETY: `atfork_prepare` acquired `SPAWN_CHDIRS.mutex` on this thread.
    unsafe { libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get()) };
}

unsafe extern "C" fn atfork_child() {
    // SAFETY: `atfork_prepare` acquired `SPAWN_CHDIRS.mutex` on the forking
    // thread, so the single-threaded child owns the lock and can unlock it.
    unsafe { libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get()) };
}

fn ensure_init() {
    ATFORK_INIT.call_once(|| {
        let prealloc = Vec::with_capacity(INITIAL_CAPACITY);
        // SAFETY: `SPAWN_CHDIRS.mutex` is statically initialized; swap the
        // pre-allocated buffer under the lock and drop the empty old `Vec`
        // outside the lock, then register fork handlers.
        let old = unsafe {
            libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
            let old = core::mem::replace(&mut *SPAWN_CHDIRS.entries.get(), prealloc);
            libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get());
            libc::pthread_atfork(Some(atfork_prepare), Some(atfork_parent), Some(atfork_child));
            old
        };
        drop(old);
    });
}

fn set_spawn_chdir(key: usize, path: Box<[u8]>) {
    ensure_init();
    let mut pending = Some(path);
    let mut spare: Option<Vec<(usize, Box<[u8]>)>> = None;
    let mut to_drop_entry: Option<Box<[u8]>> = None;
    let mut to_drop_vec: Option<Vec<(usize, Box<[u8]>)>> = None;

    loop {
        // SAFETY: `SPAWN_CHDIRS.mutex` serializes all access to `entries`, and
        // no allocation or deallocation occurs while the lock is held.
        let need_capacity = unsafe {
            libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
            let map = &mut *SPAWN_CHDIRS.entries.get();

            let result = if let Some((_, existing)) = map.iter_mut().find(|(k, _)| *k == key) {
                let new_path = pending.take().unwrap();
                to_drop_entry = Some(core::mem::replace(existing, new_path));
                None
            } else if map.len() < map.capacity() {
                map.push((key, pending.take().unwrap()));
                ACTIVE_CHDIRS.store(map.len(), Ordering::Release);
                None
            } else if let Some(mut grown) = spare.take()
                && grown.capacity() > map.len()
            {
                grown.append(map);
                grown.push((key, pending.take().unwrap()));
                let old_vec = core::mem::replace(map, grown);
                ACTIVE_CHDIRS.store(map.len(), Ordering::Release);
                to_drop_vec = Some(old_vec);
                None
            } else {
                Some(map.capacity().max(INITIAL_CAPACITY).saturating_mul(2))
            };

            libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get());
            result
        };

        if let Some(cap) = need_capacity {
            // Allocate outside `SPAWN_CHDIRS.mutex` so `malloc` never nests
            // inside our `atfork`-locked mutex.
            spare = Some(Vec::with_capacity(cap));
        } else {
            break;
        }
    }

    drop(to_drop_entry);
    drop(to_drop_vec);
    drop(spare);
}

fn remove_spawn_chdir(key: usize) {
    if ACTIVE_CHDIRS.load(Ordering::Acquire) == 0 {
        return;
    }
    ensure_init();
    // SAFETY: `SPAWN_CHDIRS.mutex` serializes access to `entries`; the removed
    // `Box<[u8]>` is dropped strictly after unlocking.
    let removed = unsafe {
        libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
        let map = &mut *SPAWN_CHDIRS.entries.get();
        let removed = map.iter().position(|(k, _)| *k == key).map(|pos| {
            let entry = map.swap_remove(pos);
            ACTIVE_CHDIRS.store(map.len(), Ordering::Release);
            entry
        });
        libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get());
        removed
    };
    drop(removed);
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
    ensure_init();

    // Pre-allocate outside `SPAWN_CHDIRS.mutex` (macOS PATH_MAX is 1024).
    let mut out = AllocVec::with_capacity_in(1024, allocator);
    loop {
        // SAFETY: `SPAWN_CHDIRS.mutex` serializes access to `entries`, and
        // copying into pre-allocated `out` performs no heap allocation.
        let status = unsafe {
            libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
            let map = &*SPAWN_CHDIRS.entries.get();
            let status = match map.iter().find(|(k, _)| *k == key) {
                None => Ok(false),
                Some((_, bytes)) if bytes.len() <= out.capacity() => {
                    out.clear();
                    out.extend_from_slice(bytes);
                    Ok(true)
                }
                Some((_, bytes)) => Err(bytes.len()),
            };
            libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get());
            status
        };

        match status {
            Ok(true) => return Some(out),
            Ok(false) => return None,
            Err(needed) => {
                out.reserve(needed.saturating_sub(out.len()));
            }
        }
    }
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
                set_spawn_chdir(key, Box::from(raw));
            } else if !raw.is_empty() {
                let prev =
                    get_spawn_chdir_in(file_actions.cast_const(), allocator_api2::alloc::Global);
                let resolved = prev.map_or_else(
                    || {
                        let arena = fspy_nostd_alloc::pooled_bump();
                        resolve_path_at(&arena, Some(CWD), c_path)
                            .ok()
                            .flatten()
                            .map(|p| Box::from(p.as_units()))
                    },
                    |base| {
                        let mut joined = base.to_vec();
                        if joined.last() != Some(&b'/') {
                            joined.push(b'/');
                        }
                        joined.extend_from_slice(raw);
                        Some(joined.into_boxed_slice())
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
                    set_spawn_chdir(key, Box::from(dir.as_units()));
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
