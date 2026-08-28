//! Filesystem calls with caller-owned storage.

#[cfg(any(target_os = "linux", target_os = "none"))]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(any(target_os = "linux", target_os = "none", target_os = "macos"))]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;
