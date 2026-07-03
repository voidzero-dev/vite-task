use std::{ffi::CStr, os::unix::ffi::OsStrExt, path::Path};

use fspy_shared_unix::exec::ExecResolveConfig;
use libc::{c_char, c_int};

use crate::{
    client::{global_client, raw_exec::RawExec},
    macros::intercept,
};

type PosixSpawnFn = unsafe extern "C" fn(
    pid: *mut libc::pid_t,
    prog: *const c_char,
    file_actions: *const libc::posix_spawn_file_actions_t,
    attrp: *const libc::posix_spawnattr_t,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> libc::c_int;

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the posix_spawn(3) signature which requires all these parameters"
)]
unsafe fn handle_posix_spawn(
    config: ExecResolveConfig,
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

    // SAFETY: file, argv, and envp are valid pointers forwarded from the interposed posix_spawn(p) function
    let result = unsafe {
        client.handle_exec::<c_int>(
            config,
            RawExec { prog: file, argv: argv.cast(), envp: envp.cast() },
            |raw_command, pre_exec| {
                let call_original = move || {
                    original(
                        pid,
                        raw_command.prog,
                        file_actions,
                        attrp,
                        raw_command.argv.cast(),
                        raw_command.envp.cast(),
                    )
                };
                if pre_exec.is_some() {
                    // Static Linux executables cannot be instrumented through
                    // LD_PRELOAD. Installing a seccomp-unotify listener here
                    // can make libc's posix_spawn fail before the child exists
                    // (reported by Node as spawnSync EINVAL). Let the spawn
                    // proceed, but tell the parent the trace is incomplete so
                    // task caching is skipped.
                    // SAFETY: raw_command.prog is a valid C string produced
                    // from the resolved Exec just above.
                    let program = CStr::from_ptr(raw_command.prog);
                    client.record_untracked_exec(Path::new(std::ffi::OsStr::from_bytes(
                        program.to_bytes(),
                    )));
                }
                Ok(call_original())
            },
        )
    };
    match result {
        Err(errno) => errno as _,
        Ok(ret) => ret,
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
) -> libc::c_int {
    // SAFETY: all arguments are valid pointers forwarded from the interposed posix_spawnp function
    unsafe {
        handle_posix_spawn(
            ExecResolveConfig::search_path_enabled(None),
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
) -> libc::c_int {
    // SAFETY: all arguments are valid pointers forwarded from the interposed posix_spawn function
    unsafe {
        handle_posix_spawn(
            ExecResolveConfig::search_path_disabled(),
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
