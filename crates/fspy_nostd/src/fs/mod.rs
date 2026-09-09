//! Filesystem calls with caller-owned storage.

#[cfg(any(target_os = "linux", target_os = "none"))]
mod linux;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
mod mac;
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos", target_os = "freebsd"))]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(any(
    target_os = "linux",
    target_os = "none",
    target_os = "macos",
    target_os = "freebsd"
))]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;

#[cfg(all(test, any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
mod tests;
