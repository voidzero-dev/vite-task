#![cfg(windows)]
#![feature(sync_unsafe_cell)]
#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "non-vite crate"
)]

pub mod windows;
