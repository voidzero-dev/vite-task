use std::thread;

use fspy_shared_unix::exec::ExecResolveConfig;
use libc::{c_char, c_int};

use crate::{
    client::{ExecInjectionError, global_client, raw_exec::RawExec},
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
    struct AssertSend<T>(T);
    #[expect(
        clippy::non_send_fields_in_send_ty,
        reason = "the closure captures raw pointers that are valid for the duration of the thread::scope call, so sending them to the scoped thread is safe"
    )]
    // SAFETY: the raw pointers captured inside T are valid for the duration of the thread::scope call, so sending them to the scoped thread is safe
    unsafe impl<T> Send for AssertSend<T> {}

    let Some(client) = global_client() else {
        // The ctor left the client unset (no readable environment, or no
        // valid payload): spawn untracked by forwarding to the real
        // posix_spawn(p).
        // SAFETY: all arguments are valid pointers forwarded from the
        // interposed posix_spawn(p) function.
        return unsafe { original(pid, file, file_actions, attrp, argv, envp) };
    };

    // SAFETY: file, argv, and envp are valid pointers forwarded from the interposed posix_spawn(p) function
    let result = unsafe {
        client.handle_exec::<c_int>(
            config,
            RawExec { prog: file, argv: argv.cast(), envp: envp.cast() },
            fspy_nostd_alloc::pooled_bump(),
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
                if let Some(pre_exec) = pre_exec {
                    thread::scope(move |s| {
                        let call_original = AssertSend(call_original);
                        s.spawn(move || {
                            let call_original = call_original;
                            pre_exec.run()?;

                            nix::Result::Ok((call_original.0)())
                        })
                        .join()
                        .unwrap()
                    })
                } else {
                    Ok(call_original())
                }
            },
        )
    };
    match result {
        Ok(ret) => ret,
        Err(ExecInjectionError::Resolution(errno)) => {
            // Resolution failed the way the real spawn would have;
            // posix_spawn returns the errno code rather than -1.
            errno as _
        }
        Err(ExecInjectionError::Injection(_)) => {
            // The injection machinery failed. Mark the run's trace incomplete
            // so it is not cached, then spawn untracked with the original
            // arguments.
            client.report_loss();
            // SAFETY: all arguments are the interposed posix_spawn(p)
            // function's own valid arguments.
            unsafe { original(pid, file, file_actions, attrp, argv, envp) }
        }
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
