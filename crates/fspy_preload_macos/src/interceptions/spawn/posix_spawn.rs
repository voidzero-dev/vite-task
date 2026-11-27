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
    // SAFETY: file, argv, and envp are valid pointers forwarded from posix_spawn(p).
    let raw_command = match unsafe {
        client.handle_exec(
            search_path,
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
