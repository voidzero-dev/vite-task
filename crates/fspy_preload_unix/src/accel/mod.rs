//! Integration between filesystem access recording and syscall interception.
//!
//! `fspy_syscall_intercept` owns executable-memory management, assembly, and
//! text rewriting. This module supplies only the callback that understands
//! filesystem syscall arguments and records their paths before allowing the
//! intercepted syscall to use the seccomp-exempt gate.

#![cfg_attr(
    test,
    expect(
        dead_code,
        reason = "the constructor is disabled in unit tests, so the registered callback is not reachable"
    )
)]

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
mod dispatcher;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub use fspy_syscall_intercept::open_control_file;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub fn init(post_syscall_pc: u64) {
    fspy_syscall_intercept::init(post_syscall_pc, dispatcher::try_dispatch);
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub const fn init(_post_syscall_pc: u64) {}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub fn open_control_file(_path: &std::ffi::CStr) -> std::io::Result<Option<std::fs::File>> {
    Ok(None)
}
