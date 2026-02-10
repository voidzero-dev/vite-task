#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "non-vite crate"
)]

pub mod ipc;

#[cfg(windows)]
pub mod windows;
