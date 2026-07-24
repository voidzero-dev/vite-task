#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub(super) use linux::{ForkGeneration, ForkTracker};
#[cfg(target_os = "macos")]
pub(super) use macos::{ForkGeneration, ForkTracker};
