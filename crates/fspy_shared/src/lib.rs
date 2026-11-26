#![cfg_attr(target_os = "none", no_std)]

pub mod ipc;

#[cfg(windows)]
pub mod windows;
