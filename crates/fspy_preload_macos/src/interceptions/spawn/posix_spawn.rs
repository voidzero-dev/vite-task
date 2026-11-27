use std::{
    cell::UnsafeCell,
    sync::{
        Once,
        atomic::{AtomicUsize, Ordering},
    },
};

use allocator_api2::vec::Vec as AllocVec;
use fspy_nostd::{BorrowedFd, CStr, CWD, Thin};
use fspy_nostd_alloc::fs::fd_path;
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

/// Base anchor for deferred `posix_spawn_file_actions_addchdir_np` /
/// `posix_spawn_file_actions_addfchdir_np` resolution.
///
/// Resolved at `posix_spawn(p)` time rather than at `add*chdir_np` time so
/// reusing a `posix_spawn_file_actions_t` across `chdir(2)` or `dup2(2)`
/// matches kernel semantics.
enum ChdirBase {
    Cwd,
    Abs(Box<[u8]>),
    Fd(c_int),
}

struct SpawnChdir {
    base: ChdirBase,
    /// Relative components appended by subsequent relative `addchdir_np`
    /// calls, joined by `/` without lexically collapsing `..`.
    tail: Box<[u8]>,
}

/// Fork-safe table mapping live `posix_spawn_file_actions_t` handle values
/// (`*file_actions as usize`) to their deferred working-directory actions from
/// `addchdir_np` / `addfchdir_np`.
///
/// Coordinated across `fork()` via `pthread_atfork`. No heap allocation or
/// deallocation ever runs while holding `mutex`, avoiding lock-order inversion
/// with libmalloc's own `atfork` handlers.
struct SpawnChdirTable {
    mutex: UnsafeCell<libc::pthread_mutex_t>,
    entries: UnsafeCell<Vec<(usize, SpawnChdir)>>,
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

fn set_spawn_chdir(key: usize, entry: SpawnChdir) {
    ensure_init();
    let mut pending = Some(entry);
    let mut spare: Option<Vec<(usize, SpawnChdir)>> = None;
    let mut to_drop_entry: Option<SpawnChdir> = None;
    let mut to_drop_vec: Option<Vec<(usize, SpawnChdir)>> = None;

    loop {
        // SAFETY: `SPAWN_CHDIRS.mutex` serializes all access to `entries`, and
        // no allocation or deallocation occurs while the lock is held.
        let need_capacity = unsafe {
            libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
            let map = &mut *SPAWN_CHDIRS.entries.get();

            let result = if let Some((_, existing)) = map.iter_mut().find(|(k, _)| *k == key) {
                let new_entry = pending.take().unwrap();
                to_drop_entry = Some(core::mem::replace(existing, new_entry));
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

fn append_relative_chdir(key: usize, rel: &[u8]) {
    ensure_init();
    let mut scratch = Vec::with_capacity(256);
    let mut spare: Option<Vec<(usize, SpawnChdir)>> = None;
    let mut new_entry = Some(SpawnChdir { base: ChdirBase::Cwd, tail: Box::from(rel) });
    let mut new_tail: Option<Box<[u8]>> = None;
    let mut to_drop_tail: Option<Box<[u8]>> = None;
    let mut to_drop_vec: Option<Vec<(usize, SpawnChdir)>> = None;

    loop {
        enum Action {
            Done,
            GrowVec(usize),
            BuildJoinedTail(usize),
        }

        // SAFETY: `SPAWN_CHDIRS.mutex` serializes all access to `entries`, and
        // no allocation or deallocation occurs while the lock is held.
        let action = unsafe {
            libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
            let map = &mut *SPAWN_CHDIRS.entries.get();

            let action = if let Some((_, existing)) = map.iter_mut().find(|(k, _)| *k == key) {
                if new_tail.is_some() && existing.tail.as_ref() == scratch.as_slice() {
                    let joined = new_tail.take().unwrap();
                    to_drop_tail = Some(core::mem::replace(&mut existing.tail, joined));
                    Action::Done
                } else if existing.tail.len() <= scratch.capacity() {
                    scratch.clear();
                    scratch.extend_from_slice(&existing.tail);
                    Action::BuildJoinedTail(0)
                } else {
                    Action::BuildJoinedTail(existing.tail.len())
                }
            } else if map.len() < map.capacity() {
                map.push((key, new_entry.take().unwrap()));
                ACTIVE_CHDIRS.store(map.len(), Ordering::Release);
                Action::Done
            } else if let Some(mut grown) = spare.take()
                && grown.capacity() > map.len()
            {
                grown.append(map);
                grown.push((key, new_entry.take().unwrap()));
                let old_vec = core::mem::replace(map, grown);
                ACTIVE_CHDIRS.store(map.len(), Ordering::Release);
                to_drop_vec = Some(old_vec);
                Action::Done
            } else {
                Action::GrowVec(map.capacity().max(INITIAL_CAPACITY).saturating_mul(2))
            };

            libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get());
            action
        };

        // Drop replaced allocations strictly outside `SPAWN_CHDIRS.mutex`.
        drop(to_drop_tail.take());
        drop(to_drop_vec.take());

        match action {
            Action::Done => break,
            Action::GrowVec(cap) => {
                spare = Some(Vec::with_capacity(cap));
            }
            Action::BuildJoinedTail(min_cap) if min_cap > 0 => {
                scratch.reserve(min_cap.saturating_sub(scratch.len()));
            }
            Action::BuildJoinedTail(_) => {
                let mut joined = Vec::with_capacity(scratch.len() + 1 + rel.len());
                if !scratch.is_empty() {
                    joined.extend_from_slice(&scratch);
                    if joined.last() != Some(&b'/') {
                        joined.push(b'/');
                    }
                }
                joined.extend_from_slice(rel);
                new_tail = Some(joined.into_boxed_slice());
            }
        }
    }

    drop(new_entry);
    drop(new_tail);
    drop(spare);
}

fn rekey_spawn_chdir(old_key: usize, new_key: usize) {
    if old_key == new_key
        || old_key == 0
        || new_key == 0
        || ACTIVE_CHDIRS.load(Ordering::Acquire) == 0
    {
        return;
    }
    ensure_init();
    // SAFETY: `SPAWN_CHDIRS.mutex` serializes access to `entries`, and updating
    // a key in place performs no heap allocation or deallocation.
    unsafe {
        libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
        let map = &mut *SPAWN_CHDIRS.entries.get();
        if let Some(entry) = map.iter_mut().find(|(k, _)| *k == old_key) {
            entry.0 = new_key;
        }
        libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get());
    }
}

fn read_actions_key(file_actions: *const libc::posix_spawn_file_actions_t) -> usize {
    if file_actions.is_null() {
        0
    } else {
        // SAFETY: caller passed a non-null `file_actions` pointer.
        (unsafe { *file_actions }) as usize
    }
}

fn remove_spawn_chdir(key: usize) {
    if ACTIVE_CHDIRS.load(Ordering::Acquire) == 0 {
        return;
    }
    ensure_init();
    // SAFETY: `SPAWN_CHDIRS.mutex` serializes access to `entries`; the removed
    // `SpawnChdir` is dropped strictly after unlocking.
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

enum ResolvedChdir<A: allocator_api2::alloc::Allocator> {
    /// No chdir action is registered on `file_actions`.
    None,
    /// Resolved absolute working directory for the child.
    Dir(AllocVec<u8, A>),
    /// A registered `addfchdir_np(fd)` could not be resolved in the parent
    /// (for example, when an earlier `addopen` creates `fd` only in the child).
    Unresolvable,
}

fn resolve_spawn_chdir_in<A: allocator_api2::alloc::Allocator + Copy>(
    file_actions: *const libc::posix_spawn_file_actions_t,
    allocator: A,
) -> ResolvedChdir<A> {
    enum SnapshotBase {
        Cwd,
        Abs,
        Fd(c_int),
    }
    enum Status {
        Missing,
        Found(SnapshotBase),
        NeedCap { base: usize, tail: usize },
    }

    if file_actions.is_null() || ACTIVE_CHDIRS.load(Ordering::Acquire) == 0 {
        return ResolvedChdir::None;
    }
    // SAFETY: caller passed a non-null `file_actions` pointer to `posix_spawn(p)`.
    let key = (unsafe { *file_actions }) as usize;
    if key == 0 {
        return ResolvedChdir::None;
    }
    ensure_init();

    // Pre-allocate outside `SPAWN_CHDIRS.mutex` (macOS PATH_MAX is 1024).
    let mut base_buf = AllocVec::with_capacity_in(1024, allocator);
    let mut tail_buf = AllocVec::with_capacity_in(256, allocator);

    let snapshot = loop {
        // SAFETY: `SPAWN_CHDIRS.mutex` serializes access to `entries`, and
        // copying into pre-allocated buffers performs no heap allocation.
        let status = unsafe {
            libc::pthread_mutex_lock(SPAWN_CHDIRS.mutex.get());
            let map = &*SPAWN_CHDIRS.entries.get();
            let status = match map.iter().find(|(k, _)| *k == key) {
                None => Status::Missing,
                Some((_, entry)) => {
                    let abs_bytes = match &entry.base {
                        ChdirBase::Abs(b) => Some(b.as_ref()),
                        ChdirBase::Cwd | ChdirBase::Fd(_) => None,
                    };
                    let need_base = abs_bytes.map_or(0, <[u8]>::len);
                    let need_tail = entry.tail.len();
                    if need_base <= base_buf.capacity() && need_tail <= tail_buf.capacity() {
                        base_buf.clear();
                        if let Some(b) = abs_bytes {
                            base_buf.extend_from_slice(b);
                        }
                        tail_buf.clear();
                        tail_buf.extend_from_slice(&entry.tail);
                        let base_kind = match entry.base {
                            ChdirBase::Cwd => SnapshotBase::Cwd,
                            ChdirBase::Abs(_) => SnapshotBase::Abs,
                            ChdirBase::Fd(fd) => SnapshotBase::Fd(fd),
                        };
                        Status::Found(base_kind)
                    } else {
                        Status::NeedCap { base: need_base, tail: need_tail }
                    }
                }
            };
            libc::pthread_mutex_unlock(SPAWN_CHDIRS.mutex.get());
            status
        };

        match status {
            Status::Missing => return ResolvedChdir::None,
            Status::Found(base_kind) => break base_kind,
            Status::NeedCap { base, tail } => {
                base_buf.reserve(base.saturating_sub(base_buf.len()));
                tail_buf.reserve(tail.saturating_sub(tail_buf.len()));
            }
        }
    };

    // Resolve the base anchor outside `SPAWN_CHDIRS.mutex` at `posix_spawn(p)` time.
    // Note: on current XNU, `PSFA_FCHDIR` resolves `fd` against the parent's
    // descriptor table before `addopen`/`adddup2` actions run, matching
    // `fd_path`. If a future kernel executes file actions sequentially and an
    // earlier `addopen`/`adddup2` reuses a parent-open fd number for a
    // different directory, `fd_path` would still see the parent's fd instead
    // of the child's; for a child-only or closed fd, returning `Unresolvable`
    // avoids resolving relative programs against the parent's CWD and preserves
    // both env injection and the kernel's `EBADF`.
    match snapshot {
        SnapshotBase::Abs => {}
        SnapshotBase::Fd(fd) => {
            // SAFETY: `fd` was accepted by `addfchdir_np`; invalid/closed fds yield `None`.
            let Some(borrowed) = (unsafe { BorrowedFd::try_borrow_raw(fd) }) else {
                return ResolvedChdir::Unresolvable;
            };
            let Ok(Some(dir)) = fd_path(allocator, borrowed) else {
                return ResolvedChdir::Unresolvable;
            };
            base_buf.clear();
            base_buf.extend_from_slice(dir.as_units());
        }
        SnapshotBase::Cwd => {
            let Ok(Some(dir)) = fd_path(allocator, CWD) else {
                return ResolvedChdir::None;
            };
            base_buf.clear();
            base_buf.extend_from_slice(dir.as_units());
        }
    }

    if !tail_buf.is_empty() {
        if base_buf.last() != Some(&b'/') {
            base_buf.push(b'/');
        }
        base_buf.extend_from_slice(&tail_buf);
    }
    ResolvedChdir::Dir(base_buf)
}

intercept!(posix_spawn_file_actions_addopen: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
    path: *const c_char,
    oflag: c_int,
    mode: libc::mode_t,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_addopen(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
    path: *const c_char,
    oflag: c_int,
    mode: libc::mode_t,
) -> c_int {
    let old_key = read_actions_key(file_actions);
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret = unsafe {
        posix_spawn_file_actions_addopen::original()(file_actions, filedes, path, oflag, mode)
    };
    if ret == 0 {
        // libsystem may `realloc` `*file_actions` as the action list grows.
        rekey_spawn_chdir(old_key, read_actions_key(file_actions));
    }
    ret
}

intercept!(posix_spawn_file_actions_addclose: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_addclose(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
) -> c_int {
    let old_key = read_actions_key(file_actions);
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret = unsafe { posix_spawn_file_actions_addclose::original()(file_actions, filedes) };
    if ret == 0 {
        rekey_spawn_chdir(old_key, read_actions_key(file_actions));
    }
    ret
}

intercept!(posix_spawn_file_actions_adddup2: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
    newfiledes: c_int,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_adddup2(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
    newfiledes: c_int,
) -> c_int {
    let old_key = read_actions_key(file_actions);
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret =
        unsafe { posix_spawn_file_actions_adddup2::original()(file_actions, filedes, newfiledes) };
    if ret == 0 {
        rekey_spawn_chdir(old_key, read_actions_key(file_actions));
    }
    ret
}

intercept!(posix_spawn_file_actions_addinherit_np: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_addinherit_np(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    filedes: c_int,
) -> c_int {
    let old_key = read_actions_key(file_actions);
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret = unsafe { posix_spawn_file_actions_addinherit_np::original()(file_actions, filedes) };
    if ret == 0 {
        rekey_spawn_chdir(old_key, read_actions_key(file_actions));
    }
    ret
}

intercept!(posix_spawn_file_actions_addchdir_np: unsafe extern "C" fn(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    path: *const c_char,
) -> c_int);
unsafe extern "C" fn posix_spawn_file_actions_addchdir_np(
    file_actions: *mut libc::posix_spawn_file_actions_t,
    path: *const c_char,
) -> c_int {
    let old_key = read_actions_key(file_actions);
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret = unsafe { posix_spawn_file_actions_addchdir_np::original()(file_actions, path) };
    if ret == 0 && !file_actions.is_null() && !path.is_null() {
        let key = read_actions_key(file_actions);
        rekey_spawn_chdir(old_key, key);
        if key != 0 {
            // SAFETY: non-null `path` is a valid NUL-terminated C string.
            let c_path = unsafe { CStr::<Thin>::from_ptr(path.cast()) };
            let raw = c_path.count().as_units();
            if raw.starts_with(b"/") {
                set_spawn_chdir(
                    key,
                    SpawnChdir { base: ChdirBase::Abs(Box::from(raw)), tail: Box::from([]) },
                );
            } else if !raw.is_empty() {
                append_relative_chdir(key, raw);
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
    let old_key = read_actions_key(file_actions);
    // SAFETY: forwarding caller arguments to the original libc function.
    let ret = unsafe { posix_spawn_file_actions_addfchdir_np::original()(file_actions, fd) };
    if ret == 0 && !file_actions.is_null() {
        let key = read_actions_key(file_actions);
        rekey_spawn_chdir(old_key, key);
        if key != 0 {
            set_spawn_chdir(key, SpawnChdir { base: ChdirBase::Fd(fd), tail: Box::from([]) });
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
    let raw_in = RawExec { prog: file, argv: argv.cast(), envp: envp.cast() };
    let raw_command = match resolve_spawn_chdir_in(file_actions, &arena) {
        ResolvedChdir::Unresolvable
            if file.addr() >= 4096 && {
                // SAFETY: `file` is non-null and outside page 0, forwarded from `posix_spawn(p)`.
                let file_bytes = unsafe { CStr::<Thin>::from_ptr(file.cast()) }.count().as_units();
                !file_bytes.starts_with(b"/") && (!search_path || file_bytes.contains(&b'/'))
            } =>
        {
            // SAFETY: file, argv, and envp are valid pointers forwarded from posix_spawn(p).
            let mut exec = match unsafe { raw_in.to_exec(&arena) } {
                Ok(exec) => exec,
                Err(err) => return err.raw_os_error(),
            };
            if let Err(err) = client.inject_env(&mut exec) {
                return err.raw_os_error();
            }
            RawExec::from_exec(exec)
        }
        resolved => {
            let spawn_cwd = match resolved {
                ResolvedChdir::Dir(dir) => Some(dir),
                ResolvedChdir::None | ResolvedChdir::Unresolvable => None,
            };
            // SAFETY: file, argv, and envp are valid pointers forwarded from posix_spawn(p).
            match unsafe {
                client.handle_exec_in_dir(search_path, spawn_cwd.as_deref(), raw_in, &arena)
            } {
                Ok(raw) => raw,
                Err(err) => return err.raw_os_error(),
            }
        }
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
