//! Low-level operations for fspy code that cannot use the normal process
//! runtime.
//!
//! This includes Unix signal handlers and post-`fork()` children, Windows
//! loader callbacks, process startup, and the injected Linux runtime. See
//! README.md for the platform-specific guarantees.

#![cfg_attr(not(test), no_std)]

mod c_str;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub mod env;
#[cfg(any(unix, windows))]
pub mod fs;
#[cfg(any(unix, windows))]
pub mod mm;
#[cfg(unix)]
pub mod param;

pub use c_str::{CStr, CStrUnit, Fat, Thin, Units, WideCStr};
#[cfg(windows)]
pub use windows::{
    AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle, RawHandle, SecurityAttributes,
    get_module_handle,
};

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Error(u32);

#[cfg(windows)]
impl Error {
    /// Creates an error from a raw Windows error code.
    #[must_use]
    pub const fn from_raw_os_error(code: u32) -> Self {
        Self(code)
    }

    /// Returns the raw Windows error code.
    #[must_use]
    pub const fn raw_os_error(self) -> u32 {
        self.0
    }
}

pub type Result<T> = core::result::Result<T, Error>;

#[cfg(windows)]
#[doc(hidden)]
pub use windows_sys::w as __wide_cstr_literal;

/// Creates a static [`WideCStr`] from a UTF-8 string literal.
#[cfg(windows)]
#[macro_export]
macro_rules! wide_cstr {
    ($literal:literal) => {{
        // SAFETY: `windows-sys` transcodes the literal into static UTF-16
        // storage and appends its NUL terminator.
        unsafe {
            $crate::WideCStr::<$crate::Thin>::from_ptr($crate::__wide_cstr_literal!($literal))
        }
    }};
}

#[cfg(unix)]
pub use rustix::{
    fd::{AsRawFd, BorrowedFd, OwnedFd},
    fs::CWD,
    io::Errno as Error,
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
