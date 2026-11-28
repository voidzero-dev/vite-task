use std::{ffi::VaList, ptr::null};

use allocator_api2::{alloc::Allocator, vec::Vec};
use fspy_shared_unix::RawExec;
use libc::{c_char, c_int};

use crate::{
    client::{global_client, set_errno},
    macros::intercept,
};

unsafe fn environ() -> *const *const c_char {
    // SAFETY: `_NSGetEnviron()` always returns a valid pointer to the process's `environ`.
    unsafe { *(libc::_NSGetEnviron().cast()) }
}

/// Collects `arg` and the variadic arguments after it into a NULL-terminated
/// argv array in `allocator` (never touching libc malloc).
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
        // SAFETY: caller guarantees arguments continue up to a NULL pointer.
        arg = unsafe { va.next_arg() };
    }
    argv.push(null());
    argv
}

fn handle_exec(
    allocator: impl Allocator,
    search_path: bool,
    prog: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    let client =
        global_client().expect("exec unexpectedly called before client initialized in ctor");
    // SAFETY: prog, argv, and envp are forwarded from the interposed exec call.
    let raw_command = match unsafe {
        client.handle_exec(search_path, RawExec { prog, argv, envp }, &allocator)
    } {
        Ok(raw) => raw,
        Err(err) => {
            set_errno(err);
            return -1;
        }
    };
    // SAFETY: `raw_command` points into `allocator`, which outlives this call.
    unsafe { execve::original()(raw_command.prog, raw_command.argv, raw_command.envp) }
}

intercept!(execve(64): unsafe extern "C" fn(
    prog: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int);
unsafe extern "C" fn execve(
    prog: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    handle_exec(fspy_nostd_alloc::pooled_bump(), false, prog, argv, envp)
}

intercept!(execl(64): unsafe extern "C" fn(path: *const c_char, arg0: *const c_char, ...) -> c_int);
unsafe extern "C" fn execl(path: *const c_char, arg: *const c_char, mut valist: ...) -> c_int {
    let arena = fspy_nostd_alloc::pooled_bump();
    // SAFETY: valist holds execl's NULL-terminated argument list after arg.
    let (argv, envp) = unsafe { (collect_argv(&arena, arg, &mut valist), environ()) };
    handle_exec(&arena, false, path, argv.as_ptr(), envp)
}

intercept!(execlp(64): unsafe extern "C" fn(path: *const c_char, arg0: *const c_char, ...) -> c_int);
unsafe extern "C" fn execlp(path: *const c_char, arg: *const c_char, mut valist: ...) -> c_int {
    let arena = fspy_nostd_alloc::pooled_bump();
    // SAFETY: valist holds execlp's NULL-terminated argument list after arg.
    let (argv, envp) = unsafe { (collect_argv(&arena, arg, &mut valist), environ()) };
    handle_exec(&arena, true, path, argv.as_ptr(), envp)
}

intercept!(execle(64): unsafe extern "C" fn(path: *const c_char, arg0: *const c_char, ...) -> c_int);
unsafe extern "C" fn execle(path: *const c_char, arg: *const c_char, mut valist: ...) -> c_int {
    let arena = fspy_nostd_alloc::pooled_bump();
    // SAFETY: valist holds execle's NULL-terminated argument list followed by envp.
    let (argv, envp) = unsafe {
        let argv = collect_argv(&arena, arg, &mut valist);
        (argv, valist.next_arg::<*const *const c_char>())
    };
    handle_exec(&arena, false, path, argv.as_ptr(), envp)
}

intercept!(execv(64): unsafe extern "C" fn(path: *const c_char, argv: *const *const c_char) -> c_int);
unsafe extern "C" fn execv(path: *const c_char, argv: *const *const c_char) -> c_int {
    // SAFETY: `environ()` returns the valid process environment pointer.
    handle_exec(fspy_nostd_alloc::pooled_bump(), false, path, argv, unsafe { environ() })
}

intercept!(execvp(64): unsafe extern "C" fn(
    prog: *const c_char,
    argv: *const *const c_char,
) -> c_int);
unsafe extern "C" fn execvp(prog: *const c_char, argv: *const *const c_char) -> c_int {
    // SAFETY: `environ()` returns the valid process environment pointer.
    handle_exec(fspy_nostd_alloc::pooled_bump(), true, prog, argv, unsafe { environ() })
}
