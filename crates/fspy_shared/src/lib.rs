#![cfg_attr(target_os = "none", no_std)]

pub mod ipc;
pub mod probe;

#[cfg(windows)]
pub mod windows;
