use std::{ffi::VaList, ptr::null};

use allocator_api2::{alloc::Allocator, vec::Vec};
use fspy_shared_unix::exec::ExecResolveConfig;
use libc::{c_char, c_int};

use crate::{
    client::{ExecInjectionError, global_client, raw_exec::RawExec},
    macros::intercept,
};

#[cfg(target_os = "macos")]
pub unsafe fn environ() -> *const *const c_char {
    // SAFETY: _NSGetEnviron() always returns a valid pointer to the process's environ on macOS
    unsafe { *(libc::_NSGetEnviron().cast()) }
}

#[cfg(target_os = "linux")]
pub unsafe fn environ() -> *const *const c_char {
    unsafe extern "C" {
        static environ: *const *const c_char;
    }
    // SAFETY: environ is a valid global pointer to the process environment, as defined by POSIX
    unsafe { environ }
}

/// Collects `arg` and the variadic arguments after it into the
/// NULL-terminated argv array that `execl`, `execlp` and `execle` take as a
/// list. The array lives in `allocator`, not libc's heap: exec may run in
/// the child of `fork()`, where malloc's lock may be held by a thread that
/// no longer exists. `va` is left past the terminator, where `execle` finds
/// `envp`.
///
/// # Safety
///
/// Unless `arg` is NULL, `va` must hold `*const c_char` arguments up to a
/// NULL pointer.
unsafe fn collect_argv<A: Allocator>(
    allocator: A,
    mut arg: *const c_char,
    va: &mut VaList<'_>,
) -> Vec<*const c_char, A> {
    let mut argv = Vec::new_in(allocator);
    while !arg.is_null() {
        argv.push(arg);
        // SAFETY: the caller guarantees the arguments continue up to a NULL pointer.
        arg = unsafe { va.next_arg() };
    }
    argv.push(null());
    argv
}

/// Resolves, reports, and performs a tracked exec.
///
/// `untracked` performs the interposed call as the caller made it, through
/// that function's own original: `execvp` keeps its `PATH` search,
/// `execveat` its `dirfd` and flags, `fexecve` its descriptor. It runs when
/// this process has no tracking client or the injection machinery fails, so
/// the process still execs, untracked.
fn handle_exec(
    allocator: impl Allocator,
    config: ExecResolveConfig,
    prog: *const libc::c_char,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
    untracked: impl FnOnce() -> libc::c_int,
) -> libc::c_int {
    let Some(client) = global_client() else {
        // The ctor left the client unset (no readable environment, or no
        // valid payload): run untracked.
        return untracked();
    };
    // The injection machinery failed (e.g. the seccomp filter cannot be
    // installed under a restrictive sandbox). Mark the run's trace incomplete
    // so it is not cached, then run the interposed call untracked. Its envp
    // still carries LD_PRELOAD/FSPY_PAYLOAD, so each generation independently
    // attempts tracking and independently degrades.
    let run_untracked = || {
        client.report_loss();
        untracked()
    };
    // SAFETY: prog, argv, and envp are valid pointers to C strings/arrays forwarded from the interposed exec function
    let handled = unsafe { client.handle_exec(config, RawExec { prog, argv, envp }, &allocator) };
    let (raw_command, pre_exec) = match handled {
        Ok(handled) => handled,
        Err(ExecInjectionError::Resolution(errno)) => {
            // Resolution failed the way the real exec would have; the errno
            // is authentic.
            errno.set();
            return -1;
        }
        Err(ExecInjectionError::Injection(_)) => return run_untracked(),
    };
    if let Some(pre_exec) = pre_exec
        && pre_exec.run().is_err()
    {
        return run_untracked();
    }
    // SAFETY: raw_command's strings and arrays live in `allocator`, which outlives the call
    unsafe { execve::original()(raw_command.prog, raw_command.argv, raw_command.envp) }
}

intercept!(execve(64): unsafe extern "C" fn(
    prog: *const libc::c_char,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
) -> libc::c_int);
unsafe extern "C" fn execve(
    prog: *const libc::c_char,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
) -> libc::c_int {
    handle_exec(
        fspy_nostd_alloc::pooled_bump(),
        ExecResolveConfig::search_path_disabled(),
        prog,
        argv,
        envp,
        // SAFETY: the interposed execve's own arguments.
        || unsafe { execve::original()(prog, argv, envp) },
    )
}

intercept!(execl(64): unsafe extern "C" fn(path: *const c_char, arg0: *const c_char, ...) -> c_int);
unsafe extern "C" fn execl(path: *const c_char, arg: *const c_char, mut valist: ...) -> c_int {
    #[expect(
        clippy::no_effect_underscore_binding,
        reason = "suppresses unused warning on *::original"
    )]
    let _unused = execl::original;
    let arena = fspy_nostd_alloc::pooled_bump();
    // SAFETY: valist holds execl's NULL-terminated argument list after arg,
    // and environ() returns the valid process environment pointer
    let (argv, envp) = unsafe { (collect_argv(&arena, arg, &mut valist), environ()) };
    handle_exec(
        &arena,
        ExecResolveConfig::search_path_disabled(),
        path,
        argv.as_ptr(),
        envp,
        // A variadic original cannot be forwarded a collected argv;
        // execl is execv over the same argv.
        // SAFETY: argv is the NULL-terminated argument list execl was given.
        || unsafe { execv::original()(path, argv.as_ptr()) },
    )
}

intercept!(execlp(64): unsafe extern "C" fn(path: *const c_char, arg0: *const c_char, ...) -> c_int);
unsafe extern "C" fn execlp(path: *const c_char, arg: *const c_char, mut valist: ...) -> c_int {
    #[expect(
        clippy::no_effect_underscore_binding,
        reason = "suppresses unused warning on *::original"
    )]
    let _unused = execlp::original;
    let arena = fspy_nostd_alloc::pooled_bump();
    // SAFETY: valist holds execlp's NULL-terminated argument list after arg,
    // and environ() returns the valid process environment pointer
    let (argv, envp) = unsafe { (collect_argv(&arena, arg, &mut valist), environ()) };
    handle_exec(
        &arena,
        ExecResolveConfig::search_path_enabled(None),
        path,
        argv.as_ptr(),
        envp,
        // execlp is execvp over the same argv, PATH search included.
        // SAFETY: argv is the NULL-terminated argument list execlp was given.
        || unsafe { execvp::original()(path, argv.as_ptr()) },
    )
}

intercept!(execle(64): unsafe extern "C" fn(path: *const c_char, arg0: *const c_char, ...) -> c_int);
unsafe extern "C" fn execle(path: *const c_char, arg: *const c_char, mut valist: ...) -> c_int {
    #[expect(
        clippy::no_effect_underscore_binding,
        reason = "suppresses unused warning on *::original"
    )]
    let _unused = execle::original;
    let arena = fspy_nostd_alloc::pooled_bump();
    // SAFETY: valist holds execle's NULL-terminated argument list after arg,
    // followed by envp
    let (argv, envp) = unsafe {
        let argv = collect_argv(&arena, arg, &mut valist);
        (argv, valist.next_arg::<*const *const c_char>())
    };
    handle_exec(
        &arena,
        ExecResolveConfig::search_path_disabled(),
        path,
        argv.as_ptr(),
        envp,
        // execle is execve over the same argv and envp.
        // SAFETY: argv is the NULL-terminated argument list execle was given,
        // and envp is the environment it was given after the list.
        || unsafe { execve::original()(path, argv.as_ptr(), envp) },
    )
}

intercept!(execv(64): unsafe extern "C" fn(path: *const c_char, argv: *const *const c_char) -> c_int);
unsafe extern "C" fn execv(path: *const c_char, argv: *const *const c_char) -> c_int {
    // SAFETY: path, argv are valid pointers forwarded from the interposed function; environ() returns the process environment
    unsafe {
        handle_exec(
            fspy_nostd_alloc::pooled_bump(),
            ExecResolveConfig::search_path_disabled(),
            path,
            argv,
            environ(),
            || execv::original()(path, argv),
        )
    }
}

intercept!(execvp(64): unsafe extern "C" fn(
    prog: *const libc::c_char,
    argv: *const *const libc::c_char,
) -> c_int);
unsafe extern "C" fn execvp(prog: *const c_char, argv: *const *const c_char) -> c_int {
    handle_exec(
        fspy_nostd_alloc::pooled_bump(),
        ExecResolveConfig::search_path_enabled(None),
        prog,
        argv,
        // SAFETY: environ() returns the valid process environment pointer
        unsafe { environ() },
        // SAFETY: the interposed execvp's own arguments.
        || unsafe { execvp::original()(prog, argv) },
    )
}

#[cfg(target_os = "linux")]
mod linux_only {
    #[expect(
        clippy::useless_attribute,
        reason = "allow_attributes on use items is flagged as useless but needed here"
    )]
    #[expect(
        clippy::allow_attributes,
        reason = "using allow because wildcard_imports may or may not fire depending on build target"
    )]
    #[allow(
        clippy::wildcard_imports,
        reason = "macro-generated code requires types from parent scope"
    )]
    use super::*;
    use crate::client::convert::{PROC_FD_PATH_CAPACITY, PathAt, ToAbsolutePath, proc_fd_path};

    intercept!(execvpe(64): unsafe extern "C" fn(
        prog: *const libc::c_char,
        argv: *const *const libc::c_char,
        envp: *const *const libc::c_char,
    ) -> libc::c_int);
    unsafe extern "C" fn execvpe(
        file: *const c_char,
        argv: *const *const libc::c_char,
        envp: *const *const libc::c_char,
    ) -> c_int {
        handle_exec(
            fspy_nostd_alloc::pooled_bump(),
            ExecResolveConfig::search_path_enabled(None),
            file,
            argv,
            envp,
            // SAFETY: the interposed execvpe's own arguments.
            || unsafe { execvpe::original()(file, argv, envp) },
        )
    }
    intercept!(execveat(64): unsafe extern "C" fn(
        dirfd: c_int,
        prog: *const libc::c_char,
        argv: *const *mut libc::c_char,
        envp: *const *mut libc::c_char,
        flags: c_int
    ) -> libc::c_int);
    unsafe extern "C" fn execveat(
        dirfd: c_int,
        pathname: *const libc::c_char,
        argv: *const *mut libc::c_char,
        envp: *const *mut libc::c_char,
        flags: c_int, // TODO: conform to semantics of flags
    ) -> libc::c_int {
        // Without `AT_EMPTY_PATH`, an empty pathname fails with `ENOENT`
        // before the kernel looks anything up, while resolving it against
        // `dirfd` would run whatever `dirfd` refers to. Let the real call
        // fail.
        // SAFETY: pathname is a valid C string from the interposed execveat call.
        if unsafe { *pathname } == 0 && flags & libc::AT_EMPTY_PATH == 0 {
            // SAFETY: forwarding the original arguments to the real execveat syscall
            return unsafe { execveat::original()(dirfd, pathname, argv, envp, flags) };
        }
        let arena = fspy_nostd_alloc::pooled_bump();

        // SAFETY: dirfd is -1 or a valid file descriptor, and pathname is a valid C string, from the interposed execveat call.
        let path = unsafe { PathAt::borrow_raw(dirfd, pathname) };
        let abs_path = match path.to_absolute_path(&arena) {
            Ok(None) => {
                // SAFETY: forwarding the original arguments to the real execveat syscall
                return unsafe { execveat::original()(dirfd, pathname, argv, envp, flags) };
            }
            Ok(Some(path)) => path,
            Err(errno) => {
                errno.set();
                return -1;
            }
        };

        // `abs_path` is a C string, so the exec receives a terminated
        // pointer by construction rather than by convention.
        handle_exec(
            &arena,
            ExecResolveConfig::search_path_disabled(),
            abs_path.as_ptr().cast(),
            argv.cast(),
            envp.cast(),
            // SAFETY: the interposed execveat's own arguments.
            || unsafe { execveat::original()(dirfd, pathname, argv, envp, flags) },
        )
    }

    intercept!(fexecve(64): unsafe extern "C" fn(
        fd: c_int,
        argv: *const *const libc::c_char,
        envp: *const *const libc::c_char,
    ) -> libc::c_int);
    unsafe extern "C" fn fexecve(
        fd: c_int,
        argv: *const *const libc::c_char,
        envp: *const *const libc::c_char,
    ) -> libc::c_int {
        let mut prog = [0; PROC_FD_PATH_CAPACITY];
        let prog = proc_fd_path(fd, &mut prog);
        handle_exec(
            fspy_nostd_alloc::pooled_bump(),
            ExecResolveConfig::search_path_disabled(),
            prog.as_ptr().cast(),
            argv,
            envp,
            // The descriptor itself, not its /proc path: a sandbox without
            // /proc can still fexecve.
            // SAFETY: the interposed fexecve's own arguments.
            || unsafe { fexecve::original()(fd, argv, envp) },
        )
    }
}
