//! Low-level operations for fspy code that cannot use the normal process
//! runtime.
//!
//! This includes Unix signal handlers and post-`fork()` children, Windows
//! loader callbacks, process startup, and the injected Linux runtime. See
//! README.md for the platform-specific guarantees.

#![cfg_attr(not(test), no_std)]

mod c_str;
mod error;
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
mod fd;
#[cfg(windows)]
mod windows;

#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
pub mod env;
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos", windows))]
pub mod fs;
#[cfg(any(target_os = "linux", target_os = "none"))]
pub mod io;
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos", windows))]
pub mod mm;
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
pub mod param;

pub use c_str::{CStr, CStrUnit, Fat, OsCStr, Thin, Units, WideCStr};
pub use error::{Error, Result};
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
pub use fd::{BorrowedFd, CWD, OwnedFd, RawFd};
#[cfg(windows)]
pub use windows::{
    BorrowedHandle, OwnedHandle, RawHandle, SecurityAttributes, bool_result, get_module_handle,
};
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
