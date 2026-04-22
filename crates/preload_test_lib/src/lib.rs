//! Test-only `LD_PRELOAD` library used by the `preexisting_ld_preload` e2e
//! fixture. Intercepts `open`/`openat` (and their `64` variants) to exercise
//! two behaviours fspy must tolerate when appended to a pre-existing
//! `LD_PRELOAD` list:
//!
//! 1. For paths containing the marker `preload_test_short_circuit`, the
//!    call is short-circuited with `ENOENT` *without* forwarding to the
//!    next preloaded library. Because fspy is appended after this library
//!    in the preload list, fspy never observes the call — exactly the
//!    property we want to verify.
//! 2. For every other path the call is forwarded via
//!    `dlsym(RTLD_NEXT, …)`, so fspy still sees the real accesses and can
//!    track them as cache inputs.
#![cfg(target_os = "linux")]
#![feature(c_variadic)]

use std::{
    ffi::{CStr, c_char, c_int},
    sync::OnceLock,
};

const MARKER: &[u8] = b"preload_test_short_circuit";

fn should_short_circuit(path: *const c_char) -> bool {
    if path.is_null() {
        return false;
    }
    // SAFETY: callers of `open`/`openat` pass a valid NUL-terminated C string
    // (or NULL, handled above).
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    bytes.windows(MARKER.len()).any(|w| w == MARKER)
}

fn fail_with_enoent() -> c_int {
    // SAFETY: `__errno_location` is async-signal-safe and always returns a
    // valid pointer to the per-thread errno.
    unsafe { *libc::__errno_location() = libc::ENOENT };
    -1
}

const fn has_mode_arg(flags: c_int) -> bool {
    flags & libc::O_CREAT != 0 || flags & libc::O_TMPFILE != 0
}

type OpenFn = unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int;
type OpenatFn = unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int;

fn load_next_fn<F: Copy>(name: &CStr) -> F {
    // SAFETY: `dlsym` with `RTLD_NEXT` returns either NULL or a valid
    // function pointer for a symbol that must exist in libc. The cast is
    // valid because the caller supplies a `F` whose layout is a function
    // pointer of the corresponding libc signature.
    let ptr = unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr()) };
    assert!(!ptr.is_null(), "dlsym RTLD_NEXT returned null");
    // SAFETY: see above.
    unsafe { std::mem::transmute_copy(&ptr) }
}

fn next_open() -> OpenFn {
    static S: OnceLock<OpenFn> = OnceLock::new();
    *S.get_or_init(|| load_next_fn(c"open"))
}
fn next_open64() -> OpenFn {
    static S: OnceLock<OpenFn> = OnceLock::new();
    *S.get_or_init(|| load_next_fn(c"open64"))
}
fn next_openat() -> OpenatFn {
    static S: OnceLock<OpenatFn> = OnceLock::new();
    *S.get_or_init(|| load_next_fn(c"openat"))
}
fn next_openat64() -> OpenatFn {
    static S: OnceLock<OpenatFn> = OnceLock::new();
    *S.get_or_init(|| load_next_fn(c"openat64"))
}

/// # Safety
/// Interposer over libc `open(2)`; same contract as the real function. Must
/// only be called by the dynamic loader after installation via `LD_PRELOAD`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open(path: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if should_short_circuit(path) {
        return fail_with_enoent();
    }
    if has_mode_arg(flags) {
        // SAFETY: `O_CREAT`/`O_TMPFILE` guarantees a `mode_t` follows per
        // the `open(2)` contract.
        let mode: libc::mode_t = unsafe { args.arg() };
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_open()(path, flags, mode) }
    } else {
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_open()(path, flags) }
    }
}

/// # Safety
/// Interposer over libc `open64(2)`; same contract as the real function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open64(path: *const c_char, flags: c_int, mut args: ...) -> c_int {
    if should_short_circuit(path) {
        return fail_with_enoent();
    }
    if has_mode_arg(flags) {
        // SAFETY: `O_CREAT`/`O_TMPFILE` guarantees a `mode_t` follows per
        // the `open64(2)` contract.
        let mode: libc::mode_t = unsafe { args.arg() };
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_open64()(path, flags, mode) }
    } else {
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_open64()(path, flags) }
    }
}

/// # Safety
/// Interposer over libc `openat(2)`; same contract as the real function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    if should_short_circuit(path) {
        return fail_with_enoent();
    }
    if has_mode_arg(flags) {
        // SAFETY: `O_CREAT`/`O_TMPFILE` guarantees a `mode_t` follows per
        // the `openat(2)` contract.
        let mode: libc::mode_t = unsafe { args.arg() };
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_openat()(dirfd, path, flags, mode) }
    } else {
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_openat()(dirfd, path, flags) }
    }
}

/// # Safety
/// Interposer over libc `openat64(2)`; same contract as the real function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat64(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    if should_short_circuit(path) {
        return fail_with_enoent();
    }
    if has_mode_arg(flags) {
        // SAFETY: `O_CREAT`/`O_TMPFILE` guarantees a `mode_t` follows per
        // the `openat64(2)` contract.
        let mode: libc::mode_t = unsafe { args.arg() };
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_openat64()(dirfd, path, flags, mode) }
    } else {
        // SAFETY: forwarding the caller's arguments unchanged.
        unsafe { next_openat64()(dirfd, path, flags) }
    }
}
