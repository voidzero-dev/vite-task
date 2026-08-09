//! Unix syscall wrappers that are safe to call where libc is not: in signal
//! handlers, in the child of `fork()` in a multithreaded process, and before
//! libc has finished initializing.
//!
//! The fspy preload library interposes libc functions that POSIX declares
//! async-signal-safe (`open`, `stat`, `execve`, ...), so its code runs in all
//! of those places, where libc's own machinery — locks, lazy initialization,
//! malloc — is off limits. Everything this crate exposes follows three rules:
//! kernel calls bypass libc on Linux, operations use no locks or hidden state,
//! and nothing allocates globally. See README.md for the full approach.

// Compile as an empty crate on non-unix targets: the crate backs the unix
// preload library.
#![cfg(unix)]
#![cfg_attr(not(test), no_std)]

mod c_str;
pub mod fs;
pub mod mm;
pub mod param;

pub use c_str::{CStr, Fat, Thin};
pub use rustix::{
    fd::{AsRawFd, BorrowedFd},
    fs::CWD,
    io::{Errno, Result},
};

// Compile-time proof that rustix uses its raw-syscall backend (`linux_raw`)
// on Linux — and with it, that rustix calls do not go through libc there.
// `rustix::runtime` is gated on that backend (`#[cfg(linux_raw)]`), so this
// reference fails to resolve, failing the whole build, whenever anything
// selects the libc backend instead: the `rustix/use-libc` feature (which any
// crate in the dependency graph can enable through feature unification, where
// no build script could ever see it), `RUSTFLAGS=--cfg=rustix_use_libc`, or a
// target rustix has no raw backend for. Miri also forces the libc backend, so
// it is exempted: it type-checks rather than ships code.
//
// The module's contents are not covered by rustix's stability promise, so a
// rustix upgrade may break this line. If that happens, re-point it at any
// other `rustix::runtime` item — do not delete it: it is the only enforcement
// of the raw-rustix rule above.
#[cfg(all(target_os = "linux", not(miri)))]
const _: () = {
    let _ = rustix::runtime::exit_group;
};
